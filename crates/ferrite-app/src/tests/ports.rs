//! The two remote ports (roadmap 3d): the terminal and the link on Bluetooth, the cable's priority, KISS beside
//! them, and the approval Bluetooth shares between the terminal and the link (TERM-25, LINK-7).

use crate::ports::{Port, Ports, To, BT_IDLE_MS};
use crate::sample::Sample;
use crate::screens::link::LinkPage;
use crate::term::Sink;
use crate::{App, Screen};
use ferrite_core::keys::Key;
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

    fn key(&mut self, k: Key) {
        self.app.key_down(k, self.now, &self.st);
        self.now += 100;
        self.app.key_up(k, self.now, &self.st);
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

/// TERM-25: the terminal opens on Bluetooth with Enter and draws there, not on the cable; reading works at
/// once; a write asks the owner on the radio's screen and isn't made; after OK it is, and the approval makes
/// no device token (the terminal has no HELLO to present one).
#[test]
fn bluetooth_terminal_reads_then_asks() {
    let mut r = Rig::new();
    r.send(Port::Bt, b"\r");
    assert_eq!(r.p.owner(), Port::Bt);
    assert!(r.p.term.on());
    assert!(!r.bt.0.is_empty() && r.cable.0.is_empty(), "drawn on Bluetooth only");
    let tokens = r.app.set.link_tokens;
    let home = r.app.set.home_ch;
    r.send(Port::Bt, b":home 3\r");
    assert_eq!(r.app.set.home_ch, home, "not written before the approval");
    assert!(matches!(r.app.screen(), Screen::Link(l) if l.page == LinkPage::Approve), "{:?}", r.app.screen());
    r.key(Key::Menu); // OK allows
    r.poll();
    r.send(Port::Bt, b":home 3\r");
    assert_eq!(r.app.set.home_ch, Some(2), "written once approved");
    assert_eq!(r.app.set.link_tokens, tokens, "no token for a terminal");
    assert!(crate::term::cmd::writes(b"home 3") && !crate::term::cmd::writes(b"get 0 9 2 1") && !crate::term::cmd::writes(b"ch 12"));
    assert!(crate::term::cmd::writes(b"bk 0 30 0") && crate::term::cmd::writes(b"ch 12 \"X\" 146.52"));
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
