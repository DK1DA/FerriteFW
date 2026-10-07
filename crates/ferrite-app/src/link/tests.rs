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
    bad[3..7].copy_from_slice(&1_000_000u32.to_le_bytes()); // 10 MHz
    let (s, why) = pc.write(what::CHANNEL, c.num, &bad, 240);
    assert_eq!(s, Status::Invalid as u8);
    assert!(std::str::from_utf8(&why).unwrap().contains("64 to 1000 MHz"), "{why:?}");
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

/// LINK-5, LINK-7: the settings read without the PIN or tokens, write back in parts, and the protected ones
/// (region, Extend TX, callsign, PIN, cable port) keep the radio's values.
#[test]
fn settings_round_trip_keeps_protected() {
    let mut pc = Pc::new(true);
    pc.app.set.pin = 1234;
    pc.app.set.pin_len = 4;
    pc.app.set.link_tokens[0] = [9; 8];
    let rec = pc.read(what::SETTINGS, 0).unwrap();
    assert!(rec.len() <= RECORD_MAX, "{} bytes", rec.len());
    let mut s = Settings::DEFAULT;
    s.load(&mut Rd { b: &rec, n: 0 });
    assert_eq!((s.pin, s.link_tokens[0]), (0, [0; 8]), "masked");
    s.theme = 2;
    s.region = 3;
    s.extend_tx = true;
    s.cable_port = 2;
    let mut out = std::vec![0u8; RECORD_MAX];
    let mut w = Window { b: &mut out, from: 0, n: 0 };
    s.save(&mut w);
    let n = w.n;
    let region = pc.app.set.region;
    assert_eq!(pc.write(what::SETTINGS, 0, &out[..n], 100).0, 0);
    assert_eq!(pc.app.set.theme, 2, "changed");
    assert_eq!((pc.app.set.region, pc.app.set.extend_tx, pc.app.set.cable_port), (region, false, 0), "kept");
    assert_eq!((pc.app.set.pin, pc.app.set.link_tokens[0]), (1234, [9; 8]), "kept, not zeroed by the masked read");
}

/// LINK-7: a port that isn't trusted asks once on screen; OK approves this device and gives it a token that
/// approves it next time without asking; BACK refuses.
#[test]
fn approval_once_per_device() {
    let mut pc = Pc::new(false);
    let c = pc.st.by_number(0).unwrap();
    let rec = pc.read(what::CHANNEL, c.num).unwrap(); // reading needs no approval
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, Status::Pending as u8);
    assert!(matches!(pc.app.screen(), Screen::Link(l) if l.page == LinkPage::Approve), "{:?}", pc.app.screen());
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, Status::Pending as u8, "still asking");
    pc.key(Key::Menu); // OK allows
    assert_eq!(pc.write(what::CHANNEL, c.num, &rec, 240).0, 0);
    let (s, token) = pc.ask(msg::TOKEN, &[]);
    assert_eq!(s, 0);
    assert_eq!(pc.app.set.link_tokens[0][..], token[..]);
    // the same device later, presenting its token
    let set = *pc.app.set;
    let mut pc2 = Pc::new(false);
    *pc2.app.set = set;
    let mut hello = std::vec![PROTOCOL];
    hello.extend_from_slice(&token);
    assert_eq!(pc2.ask(msg::HELLO, &hello).1[2], 1, "may write");
    assert_eq!(pc2.write(what::CHANNEL, c.num, &rec, 240).0, 0, "no asking");
    // a device the owner refuses
    let mut pc3 = Pc::new(false);
    assert_eq!(pc3.write(what::CHANNEL, c.num, &rec, 240).0, Status::Pending as u8);
    pc3.key(Key::Exit);
    assert_eq!(pc3.write(what::CHANNEL, c.num, &rec, 240).0, Status::NotApproved as u8);
    assert_eq!(pc3.ask(msg::TOKEN, &[]).0, Status::Refused as u8);
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
