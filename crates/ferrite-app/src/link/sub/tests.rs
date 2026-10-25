//! APRS over the link and its events, against the sample store: what Ferrite Console's APRS section and Live View
//! see.

use super::*;
use crate::data::Data;
use crate::sample::Sample;
use ferrite_core::link::what;
use ferrite_core::keys::Key;
use ferrite_core::liveview::{self, Lines};
use std::vec::Vec;

struct Wire(Vec<u8>);

impl Sink for Wire {
    fn room(&self) -> usize {
        4096
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// A PC on a session, with the Live View's tracker the ports would give it.
struct Pc {
    app: App,
    st: Sample,
    link: Session,
    ui: Track,
    now: u32,
    seq: u8,
    dev: Device,
    live: LiveAll,
}

impl Pc {
    fn new(trusted: bool) -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        let dev = Device { tx_built: true, ..Device::default() };
        Pc { app, st: Sample::new(), link: Session::new(trusted), ui: Track::new(), now: 1000, seq: 0, dev, live: LiveAll::default() }
    }

    fn ask(&mut self, ty: u8, payload: &[u8]) -> (u8, Vec<u8>) {
        self.seq = self.seq.wrapping_add(1);
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, self.seq, payload, &mut w).unwrap();
        for &c in &w[..n] {
            self.link.feed(c, self.now);
        }
        let mut out = Wire(Vec::new());
        for _ in 0..4 {
            self.now += 10;
            self.link.poll(&mut self.app, &self.st, &self.dev, &self.live, self.now, &mut out);
            while let Some(wr) = self.app.pop_write() {
                self.st.apply(wr);
            }
            if !out.0.is_empty() {
                break;
            }
        }
        let k = out.0.len() - 1;
        let f = wire::parse(&mut out.0[..k]).unwrap();
        assert_eq!((f.ty, f.seq), (ty | wire::ANSWER, self.seq));
        (f.payload[0], f.payload[1..].to_vec())
    }

    /// Time passes (`ms`), the session pushing events: each event's payload (kind first).
    fn events(&mut self, ms: u32) -> Vec<Vec<u8>> {
        let mut out = Wire(Vec::new());
        let end = self.now + ms;
        while self.now < end {
            self.now += 50;
            self.link.events(&mut self.app, &self.st, &self.dev, &self.live, self.now, Some(&mut self.ui), &mut out);
        }
        out.0
            .split(|&b| b == 0)
            .filter(|f| !f.is_empty())
            .map(|f| {
                let mut f = f.to_vec();
                let p = wire::parse(&mut f).unwrap();
                assert_eq!(p.ty, msg::EVENT);
                p.payload.to_vec()
            })
            .collect()
    }

    /// A second of the sample's APRS traffic (APRS mode on).
    fn aprs_second(&mut self) {
        self.now += 1000;
        self.st.aprs_tick(self.now / 1000, true, true);
    }

    fn aprs_rec(&mut self) -> Vec<u8> {
        let (s, a) = self.ask(msg::READ, &[what::APRS, 0, 0, 0, 0, 200]);
        assert_eq!(s, 0);
        assert_eq!(u16::from_le_bytes([a[0], a[1]]) as usize, APRS_REC);
        a[2..].to_vec()
    }

    fn aprs_write(&mut self, rec: &[u8]) -> (u8, Vec<u8>) {
        let mut p = std::vec![what::APRS, 0, 0, 0, 0];
        p.extend_from_slice(&(rec.len() as u16).to_le_bytes());
        p.extend_from_slice(rec);
        self.ask(msg::WRITE, &p)
    }
}

/// LINK-43: the APRS settings read and written as one record, each value checked as MENU 0 5's editors check it;
/// a refused record changes nothing; the region's frequency is kept as "the region's".
#[test]
fn aprs_settings_record() {
    let mut pc = Pc::new(true);
    let rec = pc.aprs_rec();
    let c = pc.app.aprs_cfg();
    assert_eq!(&rec[..8], &[0, 0, c.beacon, c.power, c.path, c.digi, 0, c.ssid]);
    let mut new = rec.clone();
    new[8..15].copy_from_slice(b"WIDE2-2");
    new[4] = crate::aprs::PATH_CUSTOM;
    (new[7], new[3], new[5]) = (9, 2, 0x31); // SSID 9, power high, digipeater on, 3 s
    assert_eq!(pc.aprs_write(&new).0, 0);
    let c = pc.app.aprs_cfg();
    assert_eq!((c.ssid, c.power, c.digi_on(), c.digi_wait_s(), c.custom_text(), c.path), (9, 2, true, 3, &b"WIDE2-2"[..], crate::aprs::PATH_CUSTOM));
    assert_eq!(pc.aprs_rec(), new);
    // APRS-45: FX.25 on transmit, the last byte (16 by default); a record without it keeps the radio's.
    assert_eq!(rec[APRS_REC - 1], crate::aprs::FX25_DEFAULT);
    new[APRS_REC - 1] = 64;
    assert_eq!(pc.aprs_write(&new).0, 0);
    assert_eq!(pc.app.aprs_cfg().fx25, 64);
    new[APRS_REC - 1] = 32;
    assert_eq!(pc.aprs_write(&new[..APRS_REC - 1]).0, 0);
    assert_eq!(pc.app.aprs_cfg().fx25, 64, "a short record keeps it");
    new[APRS_REC - 1] = 64;
    let bad: [(usize, u8, &str); 7] = [(7, 16, "SSID"), (3, 3, "power"), (2, 40, "beacon"), (5, 0x61, "digipeater"), (8, b'w', "custom"), (4, 9, "path"), (APRS_REC - 1, 20, "FX.25")];
    for (at, v, why) in bad {
        let mut b = new.clone();
        b[at] = v;
        let (s, text) = pc.aprs_write(&b);
        assert_eq!(s, Status::Invalid as u8, "{why}");
        assert!(core::str::from_utf8(&text).unwrap().contains(why) || why == "beacon" || why == "path", "{why}: {text:?}");
        assert_eq!(pc.aprs_rec(), new, "unchanged after {why}");
    }
    // 144.390 (the US region's) is kept as the region's; 300.005 MHz is past the range
    let mut b = new.clone();
    b[..2].copy_from_slice(&(14_439_000u32 / 500).to_le_bytes()[..2]);
    pc.app.set.region = 0; // US
    assert_eq!(pc.aprs_write(&b).0, 0);
    assert_eq!(pc.app.aprs_cfg().f5k, 0);
    b[..2].copy_from_slice(&(30_000_500u32 / 500).to_le_bytes()[..2]);
    assert_eq!(pc.aprs_write(&b).0, Status::Invalid as u8);
    assert_eq!(pc.aprs_write(&b[..20]).0, Status::BadArg as u8, "the whole record");
}

/// LINK-44: the heard list and the messages read in answers that hold whole records, from any index.
#[test]
fn heard_and_messages() {
    let mut pc = Pc::new(false);
    let (s, a) = pc.ask(msg::HEARD, &[0]);
    assert_eq!(s, 0);
    let n = a[0] as u16;
    assert_eq!(n, pc.st.stations());
    let first = pc.st.station(0).unwrap();
    assert_eq!(a[1] & 1, first.pos.is_some() as u8);
    assert_eq!(&a[3..3 + a[2] as usize], first.call.as_bytes());
    let (_, b) = pc.ask(msg::HEARD, &[1]);
    let second = pc.st.station(1).unwrap();
    assert_eq!(&b[3..3 + b[2] as usize], second.call.as_bytes());
    let (s, m) = pc.ask(msg::MSGS, &[0, 0]);
    assert_eq!(s, 0);
    assert_eq!(u16::from_le_bytes([m[0], m[1]]), pc.st.messages());
    let newest = pc.st.message(0).unwrap();
    assert_eq!(m[2], newest.inbound as u8);
    let peer = &m[10..10 + m[9] as usize];
    assert_eq!(peer, newest.peer.as_bytes());
    // past the end: the count, nothing more
    assert_eq!(pc.ask(msg::MSGS, &[200, 0]).1.len(), 2);
}

fn send_payload(to: &[u8], text: &[u8]) -> Vec<u8> {
    let mut p = std::vec![to.len() as u8];
    p.extend_from_slice(to);
    p.push(text.len() as u8);
    p.extend_from_slice(text);
    p
}

/// LINK-45: a message sent from the PC is queued as compose queues it (the radio's own draft kept), says whether
/// APRS mode will send it now; refused on a receive-only build, without a callsign, or with a bad address or
/// text.
#[test]
fn message_send() {
    let mut pc = Pc::new(false);
    let draft = Draft { to: Buf::from(b"VE7ABC"), text: Buf::from(b"half written") };
    pc.st.request(Op::Draft(draft));
    let n = pc.st.messages();
    let (s, a) = pc.ask(msg::MSG_SEND, &send_payload(b"ve3kpw-9", b"On my way"));
    assert_eq!(s, 0, "{a:?}");
    assert_eq!(a, [pc.app.aprs_on() as u8]);
    assert_eq!(pc.st.messages(), n + 1);
    let m = pc.st.message(0).unwrap();
    assert_eq!((m.inbound, m.peer.as_bytes(), m.text.as_bytes()), (false, &b"VE3KPW-9"[..], &b"On my way"[..]));
    assert!(pc.st.draft() == draft, "the radio's draft kept");
    for (to, text) in [(&b""[..], &b"hi"[..]), (b"VE3KPW-10X", b"hi"), (b"VE3 KPW", b"hi"), (b"VE3KPW", b""), (b"VE3KPW", b"a {1"), (b"VE3KPW", &[b'x'; 68][..])] {
        assert_eq!(pc.ask(msg::MSG_SEND, &send_payload(to, text)).0, Status::Invalid as u8, "{to:?} {text:?}");
    }
    assert_eq!(pc.ask(msg::MSG_SEND, &[3, b'A']).0, Status::BadArg as u8);
    pc.st.call.set(None);
    let (s, why) = pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW", b"hi"));
    assert_eq!((s, &why[..12]), (Status::Invalid as u8, &b"No callsign:"[..]));
    pc.dev.tx_built = false;
    let (s, why) = pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW", b"hi"));
    assert_eq!((s, &why[..12]), (Status::Invalid as u8, &b"receive-only"[..]));
    assert_eq!(pc.st.messages(), n + 1, "nothing more queued");
}

/// LINK-46: subscribed, the session is told of stations heard, messages in and their states as they happen,
/// nothing before them; unsubscribed or quiet for 10 s, nothing.
#[test]
fn aprs_events() {
    let mut pc = Pc::new(false);
    let (s, a) = pc.ask(msg::SUBSCRIBE, &[ev::SUB_HEARD | ev::SUB_MSGS]);
    assert_eq!((s, a), (0, std::vec![ev::SUB_HEARD | ev::SUB_MSGS, DICT_VERSION]));
    assert!(pc.events(500).is_empty(), "nothing has happened yet");
    let (mut heard, mut ins, mut states) = (0, 0, 0);
    for _ in 0..45 {
        pc.aprs_second(); // the sample hears a station at 8 s, a message comes at 40 s
        // the PC keeps the session alive (any request does)
        pc.ask(msg::SUBSCRIBE, &[ev::SUB_HEARD | ev::SUB_MSGS, 0, ev::RENEW]);
        for e in pc.events(300) {
            match e[0] {
                ev::HEARD => {
                    heard += 1;
                    let s = pc.st.station(0).unwrap();
                    assert_eq!(e[1] as u16, pc.st.stations());
                    assert_eq!(&e[4..4 + e[3] as usize], s.call.as_bytes());
                }
                ev::MSG => {
                    let m = pc.st.message(0).unwrap();
                    assert_eq!((e[3], &e[11..11 + e[10] as usize]), (m.inbound as u8, m.peer.as_bytes()));
                    ins += 1;
                }
                ev::MSG_STATE => states += 1,
                k => panic!("kind {k}"),
            }
        }
    }
    assert!(heard >= 1 && ins >= 1, "heard {heard}, messages {ins}, states {states}");
    // our own message, sent from the PC: a MSG event for it
    pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW", b"Test"));
    let e = pc.events(200);
    assert!(e.iter().any(|e| e[0] == ev::MSG && e[3] == 0), "{e:?}");
    // the PC goes quiet: events stop
    pc.now += ev::QUIET_MS;
    pc.aprs_second();
    pc.aprs_second();
    assert!(pc.events(2000).is_empty());
    assert_eq!(pc.ask(msg::SUBSCRIBE, &[0x08]).0, Status::BadArg as u8);
}

#[derive(Default)]
struct Screen(Vec<Vec<u8>>, Option<u8>);

impl Lines for Screen {
    fn count(&self) -> usize {
        self.0.len()
    }
    fn set_count(&mut self, n: usize) {
        self.0.resize(n, Vec::new());
    }
    fn line(&self, i: usize) -> &[u8] {
        &self.0[i]
    }
    fn set_line(&mut self, i: usize, t: &[u8]) {
        self.0[i] = t.to_vec();
    }
}

impl Screen {
    fn take(&mut self, e: &[u8]) {
        assert_eq!(e[0], ev::UI);
        self.1 = Some(liveview::apply(self, &e[1..], self.1).unwrap());
    }
}

/// The readback now, as the compact form has it.
fn compacted(pc: &Pc) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    let mut l = Line::new();
    while crate::uistate::line(&pc.app, &pc.st, &pc.dev, &pc.live, v.len() as u16, &mut l) {
        let mut b = [0u8; 256];
        let n = liveview::compact(l.as_bytes(), &mut b);
        v.push(b[..n].to_vec());
    }
    v
}

/// LINK-47, LINK-48: subscribed to the UI, the session gets the whole compact state, then a delta when a key
/// changes the screen (at once, not waiting for the gap), each applying exactly; the cable's rate cap holds.
#[test]
fn live_view_events() {
    let mut pc = Pc::new(true);
    pc.ask(msg::SUBSCRIBE, &[ev::SUB_UI, 5]);
    let mut scr = Screen::default();
    for e in pc.events(200) {
        scr.take(&e);
    }
    assert_eq!(scr.0, compacted(&pc));
    // nothing changes: nothing sent
    assert!(pc.events(1000).is_empty());
    // a key: the delta within 100 ms
    pc.app.key_down(Key::Down, pc.now, &pc.st);
    pc.app.key_up(Key::Down, pc.now + 50, &pc.st);
    pc.ask(msg::SUBSCRIBE, &[ev::SUB_UI, 5, ev::RENEW]);
    let e = pc.events(150);
    assert!(!e.is_empty(), "a delta after the key");
    for e in e {
        scr.take(&e);
    }
    assert_eq!(scr.0, compacted(&pc));
    // MENU: a whole new screen, in more than one event within the cap
    pc.app.key_down(Key::Menu, pc.now, &pc.st);
    pc.app.key_up(Key::Menu, pc.now + 50, &pc.st);
    for e in pc.events(1000) {
        scr.take(&e);
    }
    assert_eq!(scr.0, compacted(&pc));
}

/// APRS-34, LINK-44: a rejected message reads as state 6, a value of its own (0-5 keep their meaning, so an
/// older reader shows it as an unknown state rather than another one).
#[test]
fn rejected_state_byte() {
    assert_eq!(state_byte(MsgState::Rejected), (6, 0));
    assert_eq!(state_byte(MsgState::Failed), (5, 0));
}

/// LINK-49, APRS-13.1: `MSG_STOP` stops one of ours still being tried, as the radio's Stop sending does: kept as
/// cancelled; `invalid` when none to that station with that text is being tried; `bad argument` when malformed.
#[test]
fn message_stop() {
    let mut pc = Pc::new(false);
    assert_eq!(pc.ask(msg::MSG_SEND, &send_payload(b"VE3KPW-9", b"On my way")).0, 0);
    let (s, a) = pc.ask(msg::MSG_STOP, &send_payload(b"ve3kpw-9", b"On my way"));
    assert_eq!(s, 0, "{a:?}");
    assert_eq!(pc.st.message(0).map(|m| m.state), Some(MsgState::Cancelled));
    let (s, why) = pc.ask(msg::MSG_STOP, &send_payload(b"VE3KPW-9", b"On my way"));
    assert_eq!((s, &why[..]), (Status::Invalid as u8, &b"not being sent"[..]));
    assert_eq!(pc.ask(msg::MSG_STOP, &[3, b'A']).0, Status::BadArg as u8);
}
