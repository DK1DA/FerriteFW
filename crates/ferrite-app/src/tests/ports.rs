//! The two remote ports (roadmap 3d): the terminal and the link on Bluetooth, the cable's priority, KISS beside
//! them, and the approval Bluetooth shares between the terminal and the link (TERM-25, LINK-7).

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
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, 1, payload, &mut w).unwrap();
        let mut f = std::vec![0u8];
        f.extend_from_slice(&w[..n]);
        self.send(port, &f);
        let out = if port == Port::Bt { &mut self.bt } else { &mut self.cable };
        let mut a = core::mem::take(&mut out.0);
        assert_eq!(a.last(), Some(&0), "one delimited answer");
        let k = a.len() - 1;
        wire::parse(&mut a[..k]).unwrap().payload[0]
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

/// The cable always takes the terminal and the link (physical access, LINK-7); while it has them, Bluetooth's
/// terminal and link bytes are dropped, and Bluetooth gets them back once the cable's terminal has closed.
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

/// KISS-6: with 0 5 9 1 on Bluetooth, Bluetooth's bytes are KISS while the cable keeps its terminal; on Cable,
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

/// LINK-29: a Bluetooth connection quiet for BT_IDLE_MS is over: its terminal closes, its approval ends, the
/// cable has the port again, and turning Bluetooth off ends it at once; LINK-15: `BAUD` never changes
/// Bluetooth's rate (LINK-25).
#[test]
fn bluetooth_goes_quiet() {
    let mut r = Rig::new();
    assert_eq!(r.ask(Port::Bt, msg::BAUD, &921_600u32.to_le_bytes()), Status::Refused as u8, "115200 only");
    assert_eq!(r.ask(Port::Bt, msg::BAUD, &115_200u32.to_le_bytes()), 0);
    assert_eq!(r.p.take_baud(), None, "Bluetooth's rate never changes");
    r.send(Port::Bt, b"\r");
    assert!(r.p.bt_connected(&r.app, r.now));
    r.now += BT_IDLE_MS;
    r.poll();
    assert_eq!(r.p.owner(), Port::Cable);
    assert!(!r.p.term.on() && !r.p.bt_connected(&r.app, r.now));
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
