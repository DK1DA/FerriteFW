//! The two remote ports (roadmap 3d): the terminal and the link on Bluetooth, the cable's priority for the
//! terminal, a link session on each port at once (LINK-28), KISS beside them, and the PIN Bluetooth gives
//! (TERM-25, LINK-7, LINK-30).

use crate::ports::{Port, Ports, To, BT_IDLE_MS};
use crate::sample::Sample;
use crate::term::Sink;
use crate::{App, Screen};
use ferrite_core::link::{self as wire, msg, Status, MAX_WIRE};
use ferrite_core::msg::{Device, LiveAll};
use std::vec::Vec;

#[derive(Default)]
struct Out(Vec<u8>);

impl Sink for Out {
    fn room(&self) -> usize {
        8192
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

struct Rig {
    app: App,
    st: Sample,
    p: Ports,
    now: u32,
    cable: Out,
    bt: Out,
}

impl Rig {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        app.set.bt = true;
        Rig { app, st: Sample::new(), p: Ports::new(), now: 1000, cable: Out::default(), bt: Out::default() }
    }

    /// Bytes from a port, each routed and fed; then a few polls. What each byte was routed to.
    fn send(&mut self, port: Port, bytes: &[u8]) -> Vec<To> {
        let mut to = Vec::new();
        for &b in bytes {
            let t = self.p.route(&self.app, port, b, self.now);
            to.push(t);
            self.p.feed(&mut self.app, &self.st, t, b, self.now);
            while let Some(w) = self.app.pop_write() {
                self.st.apply(w);
            }
            self.poll();
        }
        for _ in 0..3 {
            self.poll();
        }
        to
    }

    fn poll(&mut self) {
        self.now += 10;
        self.p.poll(&mut self.app, &self.st, &Device::default(), &LiveAll::default(), self.now, &mut self.cable, &mut self.bt);
        while let Some(w) = self.app.pop_write() {
            self.st.apply(w);
        }
    }

    /// One link request from `port`: its answer's status.
    fn ask(&mut self, port: Port, ty: u8, payload: &[u8]) -> u8 {
        self.ask_full(port, ty, payload).0
    }

    /// One link request from `port`: its answer's status and the rest of the payload; nothing reaches the other port.
    fn ask_full(&mut self, port: Port, ty: u8, payload: &[u8]) -> (u8, Vec<u8>) {
        let before = if port == Port::Bt { self.cable.0.len() } else { self.bt.0.len() };
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, 1, payload, &mut w).unwrap();
        let mut f = std::vec![0u8];
        f.extend_from_slice(&w[..n]);
        self.send(port, &f);
        let out = if port == Port::Bt { &mut self.bt } else { &mut self.cable };
        let mut a = core::mem::take(&mut out.0);
        assert_eq!(a.last(), Some(&0), "one delimited answer");
        let k = a.len() - 1;
        let p = wire::parse(&mut a[..k]).unwrap().payload.to_vec();
        let after = if port == Port::Bt { self.cable.0.len() } else { self.bt.0.len() };
        assert_eq!(before, after, "the answer went to the asking port only");
        (p[0], p[1..].to_vec())
    }
}

/// TERM-25: the terminal opens on Bluetooth with Enter and draws there, not on the cable; with no PIN lock it
/// reads and writes at once, with no prompt on the radio (LINK-7, decision 104).
#[test]
fn bluetooth_terminal_without_pin() {
    let mut r = Rig::new();
    r.send(Port::Bt, b"\r");
    assert_eq!(r.p.owner(), Port::Bt);
    assert!(r.p.term.on());
    assert!(!r.bt.0.is_empty() && r.cable.0.is_empty(), "drawn on Bluetooth only");
    r.send(Port::Bt, b":home 3\r");
    assert_eq!(r.app.set.home_ch, Some(2), "written, no approval");
    assert!(!matches!(r.app.screen(), Screen::Link(_)), "no prompt on the radio");
    assert!(crate::term::cmd::writes(b"home 3") && !crate::term::cmd::writes(b"get 0 9 2 1") && !crate::term::cmd::writes(b"ch 12"));
    assert!(crate::term::cmd::writes(b"bk 0 30 0") && crate::term::cmd::writes(b"ch 12 \"X\" 146.52"));
}

/// LINK-30, LINK-31: with the PIN lock on, a Bluetooth terminal asks for the radio's PIN before anything else
/// (shown as *, nothing else read); wrong ones count toward the radio's own wait (PIN-3); the cable never asks.
#[test]
fn bluetooth_terminal_asks_for_the_pin() {
    let mut r = Rig::new();
    (r.app.set.pin, r.app.set.pin_len, r.app.set.pin_mode) = (2468, 4, 1);
    r.send(Port::Bt, b"\r");
    assert!(r.p.term.on());
    let shown = std::string::String::from_utf8_lossy(&r.bt.0).into_owned();
    assert!(shown.contains("PIN"), "the prompt: {shown}");
    let home = r.app.set.home_ch;
    r.send(Port::Bt, b":home 3\r");
    assert_eq!(r.app.set.home_ch, home, "nothing runs before the PIN");
    assert_eq!(r.app.set.pin_misses, 1, "only its digit was read, as a wrong PIN");
    r.send(Port::Bt, b"1111\r");
    assert_eq!(r.app.set.pin_misses, 2, "a wrong PIN counts on the radio");
    r.bt.0.clear();
    r.send(Port::Bt, b"2468\r");
    assert_eq!(r.app.set.pin_misses, 0);
    r.send(Port::Bt, b":home 3\r");
    assert_eq!(r.app.set.home_ch, Some(2), "runs after the PIN");
    // the cable takes over: never asked
    let mut c = Rig::new();
    (c.app.set.pin, c.app.set.pin_len, c.app.set.pin_mode) = (2468, 4, 1);
    c.send(Port::Cable, b"\r");
    c.send(Port::Cable, b":home 3\r");
    assert_eq!(c.app.set.home_ch, Some(2), "the cable is trusted");
}

/// The cable always takes the terminal (physical access, LINK-7); while it has it, Bluetooth's terminal bytes are
/// dropped, and Bluetooth gets it back once the cable's terminal has closed (LINK-28).
#[test]
fn the_cable_wins() {
    let mut r = Rig::new();
    r.send(Port::Bt, b"\r");
    assert_eq!(r.p.owner(), Port::Bt);
    r.send(Port::Cable, b"\r");
    assert_eq!(r.p.owner(), Port::Cable, "the cable took it");
    assert!(r.p.term.on() && !r.cable.0.is_empty());
    let to = r.send(Port::Bt, b"x");
    assert_eq!(to, [To::Drop]);
    assert_eq!(r.p.owner(), Port::Cable);
    r.send(Port::Cable, &[0x03]); // Ctrl-C closes the cable's terminal
    r.poll();
    assert!(!r.p.term.on());
    r.send(Port::Bt, b"\r");
    assert_eq!(r.p.owner(), Port::Bt, "free again: Bluetooth takes it");
}

/// KISS-6: with 0 5 8 1 on Bluetooth, Bluetooth's bytes are KISS while the cable keeps its terminal; on Cable,
/// the cable's are.
#[test]
fn kiss_beside_the_terminal() {
    let mut r = Rig::new();
    r.app.set.kiss_port = 2;
    r.send(Port::Cable, b"\r");
    assert!(r.p.term.on());
    assert_eq!(r.send(Port::Bt, &[0xC0, 0x00, 0x82]), [To::Kiss; 3]);
    assert!(r.p.term.on() && r.p.owner() == Port::Cable, "the cable's terminal stays");
    r.app.set.kiss_port = 1;
    assert_eq!(r.send(Port::Cable, &[0xC0]), [To::Kiss]);
}

/// LINK-29: a Bluetooth connection quiet for BT_IDLE_MS is over: its terminal closes, its PIN ends, the cable has
/// the terminal again, and turning Bluetooth off ends it at once; LINK-15: `BAUD` never applies to Bluetooth, whose
/// rate is the module's 9600 (LINK-25).
#[test]
fn bluetooth_goes_quiet() {
    let mut r = Rig::new();
    assert_eq!(crate::ports::BT_BAUD, 9_600, "the module's resting rate (LINK-25)");
    for rate in ferrite_core::link::RATES {
        assert_eq!(r.ask(Port::Bt, msg::BAUD, &rate.to_le_bytes()), Status::Refused as u8, "{rate}: never on Bluetooth");
    }
    assert_eq!(r.p.take_baud(), None, "Bluetooth's rate never changes");
    (r.app.set.pin, r.app.set.pin_len, r.app.set.pin_mode) = (2468, 4, 1);
    assert_eq!(r.ask(Port::Bt, msg::AUTH, b"2468"), 0);
    assert_eq!(r.ask(Port::Bt, msg::INFO, &[]), 0, "past the PIN");
    r.send(Port::Bt, b"\r");
    assert!(r.p.bt_connected(&r.app, r.now));
    r.now += BT_IDLE_MS;
    r.poll();
    assert_eq!(r.p.owner(), Port::Cable);
    assert!(!r.p.term.on() && !r.p.bt_connected(&r.app, r.now));
    assert_eq!(r.ask(Port::Bt, msg::INFO, &[]), Status::NotAuthorised as u8, "a new connection gives the PIN again");
    (r.app.set.pin, r.app.set.pin_len, r.app.set.pin_mode) = (0, 0, 0);
    // Bluetooth off ends it at once
    r.send(Port::Bt, b"\r");
    r.app.set.bt = false;
    r.poll();
    assert_eq!(r.p.owner(), Port::Cable);
}

/// LINK-14: Bluetooth is off by default.
#[test]
fn bluetooth_off_by_default() {
    let set = core::hint::black_box(crate::settings::Settings::DEFAULT);
    assert!(!set.bt, "Bluetooth off by default");
}

/// LINK-28 (decision 152): a link session on the cable and one on Bluetooth at the same time, each answered on its
/// own port; a Bluetooth link session neither takes nor is blocked by the cable's terminal, and the cable's link
/// frames leave a Bluetooth terminal open.
#[test]
fn a_session_on_each_port() {
    let mut r = Rig::new();
    let hello = [ferrite_core::link::PROTOCOL];
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &hello), 0);
    assert_eq!(r.ask(Port::Bt, msg::HELLO, &hello), 0);
    assert!(r.p.link_on(Port::Cable, r.now) && r.p.link_on(Port::Bt, r.now), "both running");
    assert_eq!(r.ask(Port::Cable, msg::INFO, &[]), 0);
    assert_eq!(r.ask(Port::Bt, msg::INFO, &[]), 0);
    // the cable's terminal open: Bluetooth's link goes on
    let mut c = Rig::new();
    c.app.set.cable_port = 2; // Terminal: the cable's bytes are the terminal's
    c.send(Port::Cable, b"\r");
    assert!(c.p.term.on() && c.p.owner() == Port::Cable);
    assert_eq!(c.ask(Port::Bt, msg::HELLO, &hello), 0, "Bluetooth's session beside the cable's terminal");
    assert!(c.p.term.on() && c.p.owner() == Port::Cable, "the terminal stays the cable's");
    // a Bluetooth terminal stays open through the cable's link frames
    let mut b = Rig::new();
    b.send(Port::Bt, b"\r");
    assert!(b.p.term.on() && b.p.owner() == Port::Bt);
    assert_eq!(b.ask(Port::Cable, msg::HELLO, &hello), 0);
    assert!(b.p.term.on() && b.p.owner() == Port::Bt, "the cable's session took nothing");
    // a CMD on each, at once: each gets its own output
    let (s, a) = b.ask_full(Port::Cable, msg::CMD, b"get 0 9 2 1");
    assert_eq!(s, 0);
    assert!(std::string::String::from_utf8_lossy(&a).contains("Bluetooth"), "{:?}", a);
}

/// LINK-28: writes go one port at a time. While the cable's write session runs (BEGIN .. END), or a record is half
/// written, Bluetooth's `BEGIN`, `WRITE` and `DELETE` answer `busy`, and the reverse; reads go on.
#[test]
fn writes_one_port_at_a_time() {
    let mut r = Rig::new();
    let hello = [ferrite_core::link::PROTOCOL];
    assert_eq!(r.ask(Port::Cable, msg::HELLO, &hello), 0);
    assert_eq!(r.ask(Port::Bt, msg::HELLO, &hello), 0);
    assert_eq!(r.ask(Port::Cable, msg::BEGIN, &1u16.to_le_bytes()), 0);
    assert_eq!(r.ask(Port::Bt, msg::BEGIN, &1u16.to_le_bytes()), Status::Busy as u8);
    assert_eq!(r.ask(Port::Bt, msg::DELETE, &[ferrite_core::link::what::CHANNEL, 0, 0]), Status::Busy as u8);
    let read = [ferrite_core::link::what::SETTINGS, 0, 0, 0, 0, 100];
    assert_eq!(r.ask(Port::Bt, msg::READ, &read), 0, "reads go on");
    assert_eq!(r.ask(Port::Cable, msg::END, &[]), 0);
    assert_eq!(r.ask(Port::Bt, msg::BEGIN, &1u16.to_le_bytes()), 0, "free again");
    assert_eq!(r.ask(Port::Cable, msg::BEGIN, &1u16.to_le_bytes()), Status::Busy as u8, "and the other way");
    assert_eq!(r.ask(Port::Bt, msg::END, &[]), 0);
    // a record half written counts too
    let mut w = std::vec![ferrite_core::link::what::SETTINGS, 0, 0, 0, 0];
    w.extend_from_slice(&300u16.to_le_bytes());
    w.extend_from_slice(&[0u8; 100]);
    assert_eq!(r.ask(Port::Cable, msg::WRITE, &w), 0, "the first part of three");
    assert_eq!(r.ask(Port::Bt, msg::WRITE, &w), Status::Busy as u8);
}

/// LINK-20.5, LINK-20.6, LINK-28: one keyer at a time. The cable's session keys; Bluetooth's key-up and keepalives are
/// refused and don't hold it; Bluetooth's session starting over leaves the cable's transmission alone; `KEY` off from
/// either port unkeys.
#[test]
fn one_remote_keyer() {
    use ferrite_core::link::{key, Ptt};
    let mut r = Rig::new();
    r.app.radio.tx_built = true;
    r.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
    while r.app.pop_cmd().is_some() {}
    let ptt = |p: Ptt| {
        let mut b = [0u8; 3];
        let n = p.encode(&mut b);
        b[..n].to_vec()
    };
    assert_eq!(r.ask(Port::Cable, msg::KEY, &ptt(Ptt::On(key::ACTIVE))), 0);
    assert!(r.app.remote_keyed());
    let (s, a) = r.ask_full(Port::Bt, msg::KEY, &ptt(Ptt::On(key::ACTIVE)));
    assert_eq!((s, a.as_slice()), (Status::Invalid as u8, crate::remote::OTHER_PORT.as_bytes()));
    let (s, a) = r.ask_full(Port::Bt, msg::KEY, &ptt(Ptt::Alive));
    assert_eq!((s, a.as_slice()), (Status::Invalid as u8, crate::remote::OTHER_PORT.as_bytes()));
    assert_eq!(r.ask(Port::Cable, msg::KEY, &ptt(Ptt::Alive)), 0, "the keyer's own keepalive");
    // Bluetooth's connection ends: the cable's transmission goes on
    r.app.set.bt = false;
    r.poll();
    r.app.set.bt = true;
    assert!(r.app.remote_keyed(), "not Bluetooth's to end");
    // keepalives are the keyer's: Bluetooth's alone don't hold it past 2 s
    for _ in 0..25 {
        r.now += 100;
        let _ = r.ask(Port::Bt, msg::KEY, &ptt(Ptt::Alive));
        r.app.tick(r.now, &LiveAll::default(), &r.st);
    }
    assert!(!r.app.remote_keyed(), "the cable's keepalives stopped: unkeyed");
    // KEY off from either port unkeys
    assert_eq!(r.ask(Port::Cable, msg::KEY, &ptt(Ptt::On(key::ACTIVE))), 0);
    assert_eq!(r.ask(Port::Bt, msg::KEY, &ptt(Ptt::Off)), 0);
    assert!(!r.app.remote_keyed(), "stopping is always allowed");
}
