//! The link session against the sample store: what a PC tool sees on the cable.

use super::*;
use crate::keys::Press;
use crate::sample::Sample;
use crate::screens::link::LinkPage;
use crate::Screen;
use ferrite_core::keys::Key;
use std::vec::Vec;

/// The cable's other end: frames out, answers back.
struct Pc {
    app: App,
    st: Sample,
    link: Session,
    now: u32,
    seq: u8,
}

struct Wire(Vec<u8>);

impl Sink for Wire {
    fn room(&self) -> usize {
        4096
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

impl Pc {
    fn new(trusted: bool) -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        Pc { app, st: Sample::new(), link: Session::new(trusted), now: 1000, seq: 0 }
    }

    /// Sends one request, polls until its answer: (status, the rest of the payload).
    fn ask(&mut self, ty: u8, payload: &[u8]) -> (u8, Vec<u8>) {
        self.seq = self.seq.wrapping_add(1);
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, self.seq, payload, &mut w).unwrap();
        self.raw(&w[..n]);
        let mut out = Wire(Vec::new());
        for _ in 0..4 {
            self.now += 10;
            self.link.poll(&mut self.app, &self.st, self.now, &mut out);
            while let Some(wr) = self.app.pop_write() {
                self.st.apply(wr);
            }
            if !out.0.is_empty() {
                break;
            }
        }
        assert_eq!(out.0.last(), Some(&0), "one delimited answer");
        let k = out.0.len() - 1;
        let f = wire::parse(&mut out.0[..k]).unwrap();
        assert_eq!((f.ty, f.seq), (ty | wire::ANSWER, self.seq));
        (f.payload[0], f.payload[1..].to_vec())
    }

    fn raw(&mut self, b: &[u8]) {
        for &c in b {
            assert!(self.link.ready());
            self.link.feed(c, self.now);
        }
    }

    /// A whole record, read in parts.
    fn read(&mut self, w: u8, idx: u16) -> Result<Vec<u8>, u8> {
        let mut rec = Vec::new();
        loop {
            let off = rec.len() as u16;
            let mut p = std::vec![w];
            p.extend_from_slice(&idx.to_le_bytes());
            p.extend_from_slice(&off.to_le_bytes());
            p.push(200);
            let (s, a) = self.ask(msg::READ, &p);
            if s != 0 {
                return Err(s);
            }
            let total = u16::from_le_bytes([a[0], a[1]]) as usize;
            rec.extend_from_slice(&a[2..]);
            if rec.len() >= total {
                return Ok(rec);
            }
        }
    }

    /// A whole record, written in parts of `part` bytes; the last part's answer.
    fn write(&mut self, w: u8, idx: u16, rec: &[u8], part: usize) -> (u8, Vec<u8>) {
        let mut last = (0, Vec::new());
        for (k, chunk) in rec.chunks(part).enumerate() {
            let mut p = std::vec![w];
            p.extend_from_slice(&idx.to_le_bytes());
            p.extend_from_slice(&((k * part) as u16).to_le_bytes());
            p.extend_from_slice(&(rec.len() as u16).to_le_bytes());
            p.extend_from_slice(chunk);
            last = self.ask(msg::WRITE, &p);
            if last.0 != 0 {
                return last;
            }
        }
        last
    }

    fn key(&mut self, k: Key) {
        self.app.key_down(k, self.now, &self.st);
        self.now += 100;
        self.app.key_up(k, self.now, &self.st);
        let _ = Press::Short;
    }
}

/// LINK-1, LINK-2: HELLO and INFO answer; every frame is acknowledged with its type and seq.
#[test]
fn hello_and_info() {
    let mut pc = Pc::new(true);
    let (s, a) = pc.ask(msg::HELLO, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0, b'p', b'c']);
    assert_eq!(s, Status::Ok as u8);
    assert_eq!(&a[..3], &[PROTOCOL, MAX_PAYLOAD as u8, 1], "protocol, max payload, may write");
    assert_eq!(&a[4..4 + a[3] as usize], VERSION.as_bytes());
    let (s, a) = pc.ask(msg::INFO, &[]);
    assert_eq!(s, 0);
    let v = a[0] as usize;
    assert_eq!(u16::from_le_bytes([a[1 + v], a[2 + v]]), pc.st.count());
    assert_eq!(pc.link.hellos(), 1);
    assert!(pc.link.active(pc.now));
    assert!(!pc.link.active(pc.now + ACTIVE_MS));
}

/// A damaged frame, an unknown type and a short payload are answered, not dropped.
#[test]
fn errors_are_answered() {
    let mut pc = Pc::new(true);
    assert_eq!(pc.ask(0x55, &[]).0, Status::UnknownType as u8);
    assert_eq!(pc.ask(msg::READ, &[2]).0, Status::BadArg as u8);
    let mut w = [0u8; MAX_WIRE];
    let n = wire::build(msg::INFO, 9, &[1, 2, 3], &mut w).unwrap();
    w[4] ^= 0x10;
    pc.raw(&w[..n]);
    let mut out = Wire(Vec::new());
    pc.link.poll(&mut pc.app, &pc.st, 2000, &mut out);
    let k = out.0.len() - 1;
    let f = wire::parse(&mut out.0[..k]).unwrap();
    assert_eq!((f.ty, f.seq, f.payload[0]), (msg::INFO | wire::ANSWER, 9, Status::BadFrame as u8));
}

/// LINK-2, LINK-5: a channel reads, writes back changed, and a bad one is refused with the reason.
#[test]
fn channel_round_trip_and_checks() {
    let mut pc = Pc::new(true);
    let c = pc.st.by_number(0).unwrap();
    let mut rec = pc.read(what::CHANNEL, c.num).unwrap();
    assert_eq!(&rec[7..7 + c.name().len()], c.name());
    rec[7] = b'Z';
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, 0);
    assert_eq!(pc.st.get(c.num).unwrap().name[0], b'Z');
    let mut bad = rec.clone();
    bad[3..7].copy_from_slice(&70_000_000u32.to_le_bytes()); // 700 MHz: no chip receives it
    let (s, why) = pc.write(what::CHANNEL, c.num, &bad, 240);
    assert_eq!(s, Status::Invalid as u8);
    assert!(std::str::from_utf8(&why).unwrap().contains("756-1300 MHz"), "{why:?}");
    bad[3..7].copy_from_slice(&1_000_000u32.to_le_bytes()); // 10 MHz: an HF channel, but NFM isn't HF mode's
    let (s, why) = pc.write(what::CHANNEL, c.num, &bad, 240);
    assert_eq!(s, Status::Invalid as u8);
    assert!(std::str::from_utf8(&why).unwrap().contains("an HF channel is AM"), "{why:?}");
    assert_eq!(pc.read(what::CHANNEL, 999).unwrap_err(), Status::NotFound as u8);
    assert_eq!(pc.ask(msg::DELETE, &[what::CHANNEL, (c.num & 255) as u8, (c.num >> 8) as u8]).0, 0);
    assert!(pc.st.get(c.num).is_none());
}

/// LINK-3: nothing but the four records is written; there is no address-based write at all.
#[test]
fn only_records_are_written() {
    let mut pc = Pc::new(true);
    let (s, _) = pc.write(9, 0, &[0; 8], 240);
    assert_eq!(s, Status::Refused as u8);
}

/// LINK-5, LINK-7: the settings read without the PIN, write back in parts, and the protected ones
/// (region, Extend TX, PIN, cable port) keep the radio's values; the callsign is written, in capitals (LINK-26).
#[test]
fn settings_round_trip_keeps_protected() {
    let mut pc = Pc::new(true);
    pc.app.set.pin = 1234;
    pc.app.set.pin_len = 4;
    let rec = pc.read(what::SETTINGS, 0).unwrap();
    assert!(rec.len() <= RECORD_MAX, "{} bytes", rec.len());
    let mut s = Settings::DEFAULT;
    s.load(&mut Rd { b: &rec, n: 0 });
    assert_eq!(s.pin, 0, "masked");
    s.theme = 2;
    s.region = 3;
    s.extend_tx = true;
    s.cable_port = 2;
    s.callsign = crate::settings::Text::EMPTY;
    for &b in b"va7fer" {
        s.callsign.push(b);
    }
    let mut out = std::vec![0u8; RECORD_MAX];
    let mut w = Window { b: &mut out, from: 0, n: 0 };
    s.save(&mut w);
    let n = w.n;
    let region = pc.app.set.region;
    assert_eq!(pc.write(what::SETTINGS, 0, &out[..n], 100).0, 0);
    assert_eq!(pc.app.set.theme, 2, "changed");
    assert_eq!(pc.app.set.callsign.as_bytes(), b"VA7FER", "the callsign is written, in capitals (LINK-26)");
    assert_eq!((pc.app.set.region, pc.app.set.extend_tx, pc.app.set.cable_port), (region, false, 0), "kept");
    assert_eq!(pc.app.set.pin, 1234, "kept, not zeroed by the masked read");
}

/// LINK-7, LINK-30, LINK-31: with no PIN lock, a Bluetooth session reads and writes like the cable, with no
/// prompt on the radio. With the PIN lock on, HELLO says AUTH first and every other request is refused until
/// AUTH gives the radio's PIN; wrong PINs count toward the radio's own 30 s wait (PIN-3), shown on its screen.
#[test]
fn bluetooth_pin_only() {
    let mut pc = Pc::new(false);
    let c = pc.st.by_number(0).unwrap();
    let rec = pc.read(what::CHANNEL, c.num).unwrap();
    assert_eq!(pc.ask(msg::HELLO, &[PROTOCOL]).1[2], 1, "no PIN lock: may go on");
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, 0, "no PIN lock: writes go ahead");
    assert!(!matches!(pc.app.screen(), Screen::Link(_)), "no prompt on the radio");
    // the PIN lock on
    let mut bt = Pc::new(false);
    (bt.app.set.pin, bt.app.set.pin_len, bt.app.set.pin_mode) = (1234, 4, 1);
    assert_eq!(bt.ask(msg::HELLO, &[PROTOCOL]).1[2], 0, "AUTH first");
    assert_eq!(bt.read(what::CHANNEL, c.num), Err(Status::NotAuthorised as u8), "not even reads");
    assert_eq!(bt.ask(msg::INFO, &[]).0, Status::NotAuthorised as u8);
    assert_eq!(bt.ask(msg::AUTH, b"0000").0, Status::Refused as u8, "wrong");
    assert_eq!(bt.app.set.pin_misses, 1, "the radio's own counter");
    assert_eq!(bt.ask(msg::AUTH, b"01234").0, Status::Refused as u8, "length counts");
    assert_eq!(bt.ask(msg::AUTH, b"9999").0, Status::Refused as u8, "third miss");
    assert!(bt.app.set.pin_wait != 0, "the radio's 30 s wait started");
    assert_eq!(bt.ask(msg::AUTH, b"1234").0, Status::Busy as u8, "even the right PIN waits");
    bt.app.set.pin_wait = 0;
    assert_eq!(bt.ask(msg::AUTH, b"1234").0, 0);
    assert_eq!(bt.app.set.pin_misses, 0, "a right PIN clears the count");
    assert_eq!(bt.ask(msg::HELLO, &[PROTOCOL]).1[2], 1);
    assert_eq!(bt.write(what::CHANNEL, c.num, &rec, 240).0, 0, "writes after AUTH, no prompt");
    // a new connection asks again
    bt.link.set_port(false, 115_200);
    assert_eq!(bt.ask(msg::INFO, &[]).0, Status::NotAuthorised as u8, "a new connection gives it again");
    // the cable never asks
    let mut cable = Pc::new(true);
    (cable.app.set.pin, cable.app.set.pin_len, cable.app.set.pin_mode) = (1234, 4, 1);
    assert_eq!(cable.ask(msg::HELLO, &[PROTOCOL]).1[2], 1);
    assert_eq!(cable.write(what::CHANNEL, c.num, &rec, 240).0, 0, "the cable is trusted");
}

/// LINK-9: BEGIN shows progress; BACK cancels the session's writes until END.
#[test]
fn write_session_progress_and_cancel() {
    let mut pc = Pc::new(true);
    let c = pc.st.by_number(0).unwrap();
    let rec = pc.read(what::CHANNEL, c.num).unwrap();
    assert_eq!(pc.ask(msg::BEGIN, &2u16.to_le_bytes()).0, 0);
    assert!(matches!(pc.app.screen(), Screen::Link(l) if l.page == LinkPage::Progress));
    assert_eq!(pc.link.progress(), Some(0));
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, 0);
    assert_eq!(pc.link.progress(), Some(50));
    pc.key(Key::Exit);
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, Status::Cancelled as u8);
    assert_eq!(pc.ask(msg::END, &[]).0, 0);
    assert_eq!(pc.link.progress(), None);
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, 0, "a new session writes again");
}

/// LINK-11: BAUD takes only a listed rate the UART reaches.
#[test]
fn baud_within_the_uart() {
    let mut pc = Pc::new(true);
    pc.link.set_max_baud(230_400);
    assert_eq!(pc.ask(msg::BAUD, &921_600u32.to_le_bytes()).0, Status::Refused as u8);
    assert_eq!(pc.ask(msg::BAUD, &100_000u32.to_le_bytes()).0, Status::Refused as u8);
    assert_eq!(pc.ask(msg::BAUD, &230_400u32.to_le_bytes()).0, 0);
    assert_eq!(pc.link.take_baud(), Some(230_400));
    assert_eq!(pc.link.take_baud(), None);
}

/// LINK-24: REBOOT on the cable is answered, then handed to the caller once; never while transmitting; on
/// Bluetooth with the PIN lock on, only after AUTH (LINK-30).
#[test]
fn reboot_to_the_bootloader() {
    let mut pc = Pc::new(true);
    pc.app.radio.keyed = Some(0);
    assert_eq!(pc.ask(msg::REBOOT, &[]).0, Status::Busy as u8, "not while transmitting");
    assert!(!pc.link.take_reboot());
    pc.app.radio.keyed = None;
    assert_eq!(pc.ask(msg::REBOOT, &[]).0, 0);
    assert!(pc.link.take_reboot());
    assert!(!pc.link.take_reboot(), "once");
    let mut bt = Pc::new(false);
    (bt.app.set.pin, bt.app.set.pin_len, bt.app.set.pin_mode) = (1234, 4, 1);
    assert_eq!(bt.ask(msg::REBOOT, &[]).0, Status::NotAuthorised as u8);
    assert!(!bt.link.take_reboot());
    assert_eq!(bt.ask(msg::AUTH, b"1234").0, 0);
    assert_eq!(bt.ask(msg::REBOOT, &[]).0, 0);
    assert!(bt.link.take_reboot());
}

/// TERM-2: the cable port setting and the first bytes decide.
#[test]
fn cable_routing() {
    assert_eq!(route(0, false, b'\r'), Route::Term);
    assert_eq!(route(0, false, 0), Route::Link);
    assert_eq!(route(0, true, b'\r'), Route::Link, "a link session keeps the port");
    assert_eq!(route(1, false, b'\r'), Route::Link);
    assert_eq!(route(2, true, 0), Route::Term);
    assert_eq!(route(3, false, 0), Route::Drop);
}

// ---- Remote PTT (LINK-20) ----

use crate::remote::RemoteEnd;
use ferrite_core::keys::{PTT_A, PTT_B};
use ferrite_core::link::{key, Ptt};
use ferrite_core::msg::{LiveAll, RadioCmd};

impl Pc {
    /// A transmit build with a region set, as a person's PTT would key.
    fn tx() -> Self {
        let mut pc = Pc::new(true);
        pc.app.radio.tx_built = true;
        pc.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
        while pc.app.pop_cmd().is_some() {}
        pc
    }

    fn ptt(&mut self, p: Ptt) -> (u8, Vec<u8>) {
        let mut b = [0u8; 3];
        let n = p.encode(&mut b);
        self.ask(msg::KEY, &b[..n])
    }

    /// Time passes on the radio: the App's ticks, and the session polled.
    fn pass(&mut self, ms: u32, live: &LiveAll) {
        let end = self.now + ms;
        while self.now < end {
            self.now += 50;
            self.app.tick(self.now, live, &self.st);
            self.link.poll(&mut self.app, &self.st, self.now, &mut Wire(Vec::new()));
        }
    }

    fn cmds(&mut self) -> Vec<RadioCmd> {
        core::iter::from_fn(|| self.app.pop_cmd()).collect()
    }
}

fn keyed(c: &[RadioCmd]) -> bool {
    c.iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. }))
}

fn unkeyed(c: &[RadioCmd]) -> bool {
    c.iter().any(|c| matches!(c, RadioCmd::Transmit { on: false, .. }))
}

fn text(a: &[u8]) -> &str {
    core::str::from_utf8(a).unwrap()
}

/// LINK-20: KEY on keys the active slot through the PTT path (the Transmit screen, marked Remote), keepalives
/// hold it, KEY off unkeys; then a keepalive says it isn't keyed, and KEY off is always ok.
#[test]
fn remote_ptt_keys_and_unkeys() {
    let mut pc = Pc::tx();
    let (s, a) = pc.ptt(Ptt::On(key::ACTIVE));
    assert_eq!((s, a.as_slice()), (Status::Ok as u8, &[0u8][..]), "the active slot, A, keyed");
    let c = pc.cmds();
    assert!(c.iter().any(|c| matches!(c, RadioCmd::TxPlan { confirmed: false, .. })), "never a confirmed emergency plan");
    assert!(c.contains(&RadioCmd::Transmit { slot: 0, on: true }));
    assert!(matches!(pc.app.screen(), Screen::Transmit(_)) && pc.app.remote_keyed());
    for _ in 0..6 {
        pc.pass(500, &LiveAll::default());
        assert_eq!(pc.ptt(Ptt::Alive).0, Status::Ok as u8, "keepalives hold it");
    }
    assert!(!unkeyed(&pc.cmds()), "3 s keyed with keepalives");
    assert_eq!(pc.ptt(Ptt::Off).0, Status::Ok as u8);
    assert!(pc.cmds().contains(&RadioCmd::Transmit { slot: 0, on: false }));
    assert_eq!(pc.app.screen(), Screen::Home);
    let (s, a) = pc.ptt(Ptt::Alive);
    assert_eq!((s, text(&a)), (Status::Invalid as u8, "not keyed"));
    assert_eq!(pc.ptt(Ptt::Off).0, Status::Ok as u8, "off again is fine");
    // PTT B's slot; C only while it is the active slot.
    assert_eq!(pc.ptt(Ptt::On(1)), (0, std::vec![1]));
    assert!(pc.cmds().contains(&RadioCmd::Transmit { slot: 1, on: true }));
    assert_eq!(pc.ptt(Ptt::On(1)).0, 0, "a repeated key-up is a keepalive");
    assert_eq!(pc.ptt(Ptt::On(0)).0, Status::Invalid as u8, "one transmission at a time");
    pc.ptt(Ptt::Off);
    let (s, a) = pc.ptt(Ptt::On(2));
    assert_eq!(s, Status::Invalid as u8, "C isn't active: {}", text(&a));
    assert!(!keyed(&pc.cmds()));
    assert_eq!(pc.ask(msg::KEY, &[key::PTT, key::ON]).0, Status::BadArg as u8, "ON needs its slot");
    assert_eq!(pc.ask(msg::KEY, &[0x10, 1]).0, Status::BadArg as u8, "other keys: LINK-19, not built");
}

/// LINK-20: no keepalive for 2 s, no transmit; the PC hears why.
#[test]
fn remote_ptt_keepalive_timeout() {
    let mut pc = Pc::tx();
    assert_eq!(pc.ptt(Ptt::On(key::ACTIVE)).0, 0);
    pc.cmds();
    pc.pass(1800, &LiveAll::default());
    assert!(pc.app.remote_keyed() && !unkeyed(&pc.cmds()), "1.8 s: still keyed");
    pc.pass(300, &LiveAll::default());
    assert!(!pc.app.remote_keyed());
    assert!(unkeyed(&pc.cmds()), "past 2 s: unkeyed");
    assert_eq!(pc.app.screen(), Screen::Home);
    let (s, a) = pc.ptt(Ptt::Alive);
    assert_eq!((s, text(&a)), (Status::Invalid as u8, RemoteEnd::Keepalive.text()));
}

/// LINK-20, LINK-28: the session closing or moving port ends a remote transmission.
#[test]
fn remote_ptt_ends_with_the_session() {
    let mut pc = Pc::tx();
    assert_eq!(pc.ptt(Ptt::On(key::ACTIVE)).0, 0);
    pc.cmds();
    pc.link.set_port(false, 115_200); // Bluetooth took the session, or the cable took it back
    pc.pass(50, &LiveAll::default());
    assert!(unkeyed(&pc.cmds()) && !pc.app.remote_keyed());
    assert_eq!(text(&pc.ptt(Ptt::Alive).1), RemoteEnd::Session.text());
}

/// LINK-20: the radio's own refusals come back as `invalid` with the reason; the Radio task's stops too.
#[test]
fn remote_ptt_refusals_pass_through() {
    let mut pc = Pc::new(true); // a receive-only build
    let (s, a) = pc.ptt(Ptt::On(key::ACTIVE));
    assert_eq!((s, text(&a)), (Status::Invalid as u8, "receive-only build"));
    assert!(!keyed(&pc.cmds()) && !pc.app.remote_keyed());
    // outside the band plan
    let mut pc = Pc::tx();
    let a0 = pc.app.active as usize;
    (pc.app.slots[a0].vfo, pc.app.slots[a0].vfo_mode) = (ferrite_core::freq::mhz(160), true);
    let (s, a) = pc.ptt(Ptt::On(key::ACTIVE));
    assert_eq!((s, text(&a)), (Status::Invalid as u8, "outside the band plan"));
    assert!(matches!(pc.app.screen(), Screen::TxRefused(_)), "the radio shows it as for PTT");
    // keyed, then the Radio task stops it (the transmitter too hot): the next keepalive says so
    let mut pc = Pc::tx();
    assert_eq!(pc.ptt(Ptt::On(key::ACTIVE)).0, 0);
    let mut live = LiveAll::default();
    live.tx.refused = Some(ferrite_core::txplan::Refusal::Hot);
    pc.pass(100, &live);
    assert!(!pc.app.remote_keyed());
    assert_eq!(text(&pc.ptt(Ptt::Alive).1), "transmitter cooling");
    // the transmit timeout (TX-30)
    let mut pc = Pc::tx();
    assert_eq!(pc.ptt(Ptt::On(key::ACTIVE)).0, 0);
    let limit = pc.app.set.tx_timeout_s() * 1000;
    let mut t = 0;
    while t < limit + 500 {
        pc.pass(500, &LiveAll::default());
        pc.ptt(Ptt::Alive);
        t += 500;
    }
    assert!(!pc.app.remote_keyed());
    assert_eq!(text(&pc.ptt(Ptt::Alive).1), RemoteEnd::Timeout.text());
}

/// LINK-20, TX-35: a remote key-up never transmits on an emergency frequency, never opens the second-PTT prompt,
/// and never answers one a person opened.
#[test]
fn remote_ptt_never_on_an_emergency_frequency() {
    let mut pc = Pc::tx();
    pc.app.set_region(Some(ferrite_core::txplan::Region::Us), true);
    let a0 = pc.app.active as usize;
    (pc.app.slots[a0].vfo, pc.app.slots[a0].vfo_mode) = (ferrite_core::freq::mhz(406) + 2_500, true);
    pc.cmds();
    let (s, a) = pc.ptt(Ptt::On(key::ACTIVE));
    assert_eq!(s, Status::Invalid as u8);
    assert!(text(&a).starts_with("emergency frequency"));
    assert!(!pc.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. } | RadioCmd::TxPlan { .. })));
    assert!(!matches!(pc.app.screen(), Screen::Emergency(_)), "no prompt");
    // A person's first PTT opens the prompt; a remote KEY on slot B doesn't confirm it.
    pc.app.key_down(Key::Ptt(PTT_A), pc.now, &pc.st);
    pc.app.key_up(Key::Ptt(PTT_A), pc.now + 100, &pc.st);
    assert!(matches!(pc.app.screen(), Screen::Emergency(_)));
    assert_eq!(pc.ptt(Ptt::On(1)).0, Status::Invalid as u8);
    assert!(!pc.cmds().iter().any(|c| matches!(c, RadioCmd::Transmit { on: true, .. } | RadioCmd::TxPlan { .. })));
}

/// LINK-20: the radio's operator wins. A local PTT during a remote transmission ends it and keys nothing until let
/// go; a remote key-up is refused while a local PTT is held; after, PTT keys as usual.
#[test]
fn remote_ptt_local_ptt_wins() {
    let mut pc = Pc::tx();
    assert_eq!(pc.ptt(Ptt::On(key::ACTIVE)).0, 0);
    pc.cmds();
    pc.app.key_down(Key::PTT, pc.now, &pc.st); // the side connector's, the same key the remote pressed
    let c = pc.cmds();
    assert!(unkeyed(&c) && !keyed(&c), "unkeyed, and the press keys nothing");
    assert!(!pc.app.remote_keyed());
    assert_eq!(text(&pc.ptt(Ptt::Alive).1), RemoteEnd::LocalPtt.text());
    let (s, a) = pc.ptt(Ptt::On(key::ACTIVE));
    assert_eq!((s, text(&a)), (Status::Invalid as u8, "PTT held on the radio"));
    pc.app.key_up(Key::PTT, pc.now + 200, &pc.st);
    assert!(pc.cmds().is_empty(), "its release does nothing either");
    // PTT B held: the remote waits; once let go, a person's PTT keys as usual.
    pc.app.key_down(Key::Ptt(PTT_B), pc.now, &pc.st);
    assert!(keyed(&pc.cmds()));
    assert_eq!(pc.ptt(Ptt::On(key::ACTIVE)).0, Status::Invalid as u8);
    pc.app.key_up(Key::Ptt(PTT_B), pc.now + 300, &pc.st);
    assert!(unkeyed(&pc.cmds()));
    assert_eq!(pc.ptt(Ptt::On(key::ACTIVE)).0, 0, "the remote again");
    pc.ptt(Ptt::Off);
}

/// LINK-20, TX-43: `KEY` is answered without room for the largest answer and with the store busy (on the cable its
/// answers wait in the send ring while keyed), so keepalives keep coming in.
#[test]
fn remote_ptt_answered_with_a_full_ring() {
    struct Small(Vec<u8>);
    impl Sink for Small {
        fn room(&self) -> usize {
            16usize.saturating_sub(self.0.len())
        }
        fn put(&mut self, b: &[u8]) {
            assert!(b.len() <= self.room(), "never cut");
            self.0.extend_from_slice(b);
        }
    }
    let mut pc = Pc::tx();
    let mut b = [0u8; 3];
    let n = Ptt::On(key::ACTIVE).encode(&mut b);
    let mut w = [0u8; MAX_WIRE];
    let k = wire::build(msg::KEY, 1, &b[..n], &mut w).unwrap();
    pc.raw(&w[..k]);
    let mut out = Small(Vec::new());
    pc.link.poll(&mut pc.app, &pc.st, pc.now, &mut out);
    assert!(pc.app.remote_keyed() && !out.0.is_empty(), "answered with 16 bytes of room");
    // a full ring: the keepalive still counts, its answer is dropped whole
    let n = Ptt::Alive.encode(&mut b);
    let k = wire::build(msg::KEY, 2, &b[..n], &mut w).unwrap();
    pc.now += 1500;
    pc.raw(&w[..k]);
    pc.link.poll(&mut pc.app, &pc.st, pc.now, &mut Small(std::vec![0; 16]));
    assert!(pc.link.ready(), "handled");
    pc.now += 1000;
    pc.app.tick(pc.now, &LiveAll::default(), &pc.st);
    assert!(pc.app.remote_keyed(), "the keepalive counted: 1 s since it");
}

/// LINK-20, LINK-30: on Bluetooth with a PIN lock, `KEY` only after `AUTH`.
#[test]
fn remote_ptt_needs_the_pin_on_bluetooth() {
    let mut bt = Pc::tx();
    bt.link = Session::new(false);
    (bt.app.set.pin, bt.app.set.pin_len, bt.app.set.pin_mode) = (1234, 4, 1);
    assert_eq!(bt.ptt(Ptt::On(key::ACTIVE)).0, Status::NotAuthorised as u8);
    assert!(!keyed(&bt.cmds()));
    assert_eq!(bt.ask(msg::AUTH, b"1234").0, 0);
    assert_eq!(bt.ptt(Ptt::On(key::ACTIVE)).0, 0);
    assert!(keyed(&bt.cmds()));
}

// ---- SLOT (LINK-33) ----

/// LINK-33: `SLOT` reads a slot, changes it through the keypad's paths (as the terminal's `slot`, TERM-35) and
/// answers it as it is after; a refusal is `invalid` with the reason and changes nothing.
#[test]
fn slot_read_and_set() {
    use ferrite_core::link::slot as m;
    let mut pc = Pc::new(true);
    let (s, a) = pc.ask(msg::SLOT, &[1]);
    assert_eq!((s, a.len(), a[0], a[1]), (0, 19, 1, 0), "slot B, not active");
    assert_eq!(u16::from_le_bytes([a[2], a[3]]), 0xFFFF, "on its VFO");
    let mut p = std::vec![1, m::FREQ | m::MODE | m::POWER | m::TX_TONE | m::ACTIVE];
    p.extend_from_slice(&(ferrite_core::freq::mhz(146) + 94_000).to_le_bytes());
    p.extend_from_slice(&[1, 0]);
    p.extend_from_slice(&1000u16.to_le_bytes());
    let (s, a) = pc.ask(msg::SLOT, &p);
    assert_eq!(s, 0);
    assert_eq!(u32::from_le_bytes([a[4], a[5], a[6], a[7]]), ferrite_core::freq::mhz(146) + 94_000);
    assert_eq!((a[1], a[8], a[9], u16::from_le_bytes([a[12], a[13]])), (1, 1, 0, 1000), "active, NFM, low, tone sent");
    assert_eq!(pc.app.active(), 1);
    // refused: nothing changes
    let mut p = std::vec![1, m::FREQ | m::MODE];
    p.extend_from_slice(&ferrite_core::freq::mhz(7).to_le_bytes());
    p.push(0);
    let (s, a) = pc.ask(msg::SLOT, &p);
    assert_eq!(s, Status::Invalid as u8);
    assert!(text(&a).contains("HF mode"));
    assert_eq!(pc.app.slot_state(1).mode, 1, "the mode not changed either");
    assert_eq!(pc.ask(msg::SLOT, &[3]).0, Status::BadArg as u8);
    assert_eq!(pc.ask(msg::SLOT, &[0, m::FREQ, 1, 2]).0, Status::BadArg as u8, "a field cut short");
    assert_eq!(pc.ask(msg::SLOT, &[0, 0x80]).0, Status::BadArg as u8);
}
