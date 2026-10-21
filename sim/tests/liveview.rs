//! Live View over the simulator's Bluetooth (LINK-46, LINK-47, LINK-48): what Ferrite Console's Bluetooth Live View sees.
//! A phone subscribes to the UI, gets the whole compact state, then deltas as the screen changes; the stream's byte
//! rate is measured for an idle home screen, a scan and opening a menu.

mod common;
use common::{key, Radio};
use ferrite_core::link::{self as wire, event as ev, msg, MAX_WIRE, PROTOCOL};
use ferrite_core::liveview::{self, Lines};

#[derive(Default)]
struct Phone {
    lines: Vec<Vec<u8>>,
    last: Option<u8>,
    seq: u8,
    /// UI event bytes on the wire, and events, since the last look.
    bytes: usize,
    events: usize,
    buf: Vec<u8>,
    answers: Vec<(u8, u8, Vec<u8>)>,
}

impl Lines for Phone {
    fn count(&self) -> usize {
        self.lines.len()
    }
    fn set_count(&mut self, n: usize) {
        self.lines.resize(n, Vec::new());
    }
    fn line(&self, i: usize) -> &[u8] {
        &self.lines[i]
    }
    fn set_line(&mut self, i: usize, t: &[u8]) {
        self.lines[i] = t.to_vec();
    }
}

impl Phone {
    fn send(&mut self, r: &mut Radio, ty: u8, payload: &[u8]) -> u8 {
        self.seq = self.seq.wrapping_add(1);
        let mut w = [0u8; MAX_WIRE];
        let n = wire::build(ty, self.seq, payload, &mut w).unwrap();
        let mut b = vec![0u8];
        b.extend_from_slice(&w[..n]);
        r.sim.bt_feed(&b);
        self.seq
    }

    /// Bluetooth's bytes so far: events applied, answers kept.
    fn pump(&mut self, r: &mut Radio) {
        self.buf.extend(r.sim.bt_take());
        while let Some(end) = self.buf.iter().position(|&b| b == 0) {
            let mut f: Vec<u8> = self.buf.drain(..=end).collect();
            f.pop();
            if f.is_empty() {
                continue;
            }
            let wire_len = f.len() + 1;
            let p = wire::parse(&mut f).expect("a good frame");
            if p.ty == msg::EVENT {
                assert_eq!(p.payload[0], ev::UI);
                if std::env::var("LV_DEBUG").is_ok() {
                    let mut d = vec![0u8; 512];
                    let n = liveview::decode(&p.payload[1..], &mut d);
                    eprintln!("ev {} B: {:?}", wire_len, String::from_utf8_lossy(&d[..n]));
                }
                self.last = Some(liveview::apply(self, &p.payload[1..], self.last).expect("applies"));
                (self.bytes, self.events) = (self.bytes + wire_len, self.events + 1);
            } else {
                self.answers.push((p.ty, p.seq, p.payload.to_vec()));
            }
        }
    }

    fn ask(&mut self, r: &mut Radio, ty: u8, payload: &[u8]) -> Vec<u8> {
        let seq = self.send(r, ty, payload);
        for _ in 0..50 {
            r.wait(20);
            self.pump(r);
            if let Some(i) = self.answers.iter().position(|a| a.0 == ty | wire::ANSWER && a.1 == seq) {
                return self.answers.remove(i).2;
            }
        }
        panic!("no answer to {ty:#x}");
    }

    /// `ms` pass with the phone renewing its subscription every 5 s (LINK-46); the UI bytes a second meanwhile.
    fn watch(&mut self, r: &mut Radio, ms: u32) -> f32 {
        (self.bytes, self.events) = (0, 0);
        let mut t = 0;
        while t < ms {
            r.wait(100);
            t += 100;
            self.pump(r);
            if t % 5000 == 0 {
                self.send(r, msg::SUBSCRIBE, &[ev::SUB_UI, 0, ev::RENEW]);
            }
        }
        self.bytes as f32 * 1000.0 / ms as f32
    }

    /// Waits (up to 3 s) for the phone to have the screen as the radio's readback says it now.
    fn settle(&mut self, r: &mut Radio) -> bool {
        for _ in 0..150 {
            if self.in_step(r) {
                return true;
            }
            r.wait(20);
            self.pump(r);
        }
        false
    }

    /// Whether the phone has what the radio's readback says now (the compact form).
    fn in_step(&self, r: &Radio) -> bool {
        let want: Vec<Vec<u8>> = r
            .sim
            .readback()
            .lines()
            .take(liveview::LINES)
            .map(|l| {
                let mut b = [0u8; 256];
                let n = liveview::compact(&l.as_bytes()[..l.len().min(255)], &mut b);
                b[..n].to_vec()
            })
            .collect();
        want == self.lines
    }
}

/// A radio with Bluetooth on (0 9 2 1) and the cable's terminal closed, and a phone subscribed to the UI.
fn subscribed() -> (Radio, Phone) {
    let mut r = Radio::new();
    r.sim.term_feed(b"\r:set 0 9 2 1 On\r");
    r.wait(200);
    r.sim.term_feed(&[0x03]);
    r.wait(200);
    r.sim.term_take();
    let mut ph = Phone::default();
    assert_eq!(ph.ask(&mut r, msg::HELLO, &[PROTOCOL])[0], 0);
    let a = ph.ask(&mut r, msg::SUBSCRIBE, &[ev::SUB_UI]);
    assert_eq!(a, [0, ev::SUB_UI, liveview::DICT_VERSION]);
    (r, ph)
}

/// LINK-46, LINK-47, LINK-48 in the simulator: subscribed over Bluetooth, the phone gets the whole UI, then a delta soon
/// after a key, always matching the readback; and the stream's cost: an idle home screen, a scan, a menu opened.
/// The rates print with `--nocapture`.
#[test]
fn live_view_over_bluetooth() {
    let (mut r, mut ph) = subscribed();
    ph.watch(&mut r, 3000);
    // every event applied and checked (a mismatch fails `apply`); C scans, so compare the screen's line
    assert!(ph.lines.first().is_some_and(|l| l.starts_with(b"screen Home")), "the whole state arrived");
    // the sample's C scans the airband (stopping on its traffic)
    let scan_c = ph.watch(&mut r, 30_000);
    let scan_c_ev = ph.events;
    // C, the sample's scanning slot, stops: an idle home screen (B still hears its repeater now and then)
    r.press(key("SLOT")).press(key("SLOT")).press(key("SIDE2")).press(key("SLOT")).wait(1000);
    assert!(!r.sim.radio().live.slots[2].scanning);
    ph.watch(&mut r, 2000);
    let idle = ph.watch(&mut r, 60_000);
    let idle_ev = ph.events;
    assert!(ph.settle(&mut r));
    // a key: the delta follows at once (not waiting for the 500 ms gap)
    r.down(key("DOWN")).wait(100);
    ph.pump(&mut r);
    let before = ph.events;
    r.up(key("DOWN")).wait(130); // a short press acts when let go
    ph.pump(&mut r);
    assert!(ph.events > before, "a delta within 150 ms of the key: idle {idle} B/s {idle_ev} ev; {}", r.sim.readback());
    r.press(key("UP"));
    assert!(ph.settle(&mut r));
    // a scan on A (the VHF band)
    r.press(key("SIDE2")).wait(500);
    assert!(r.sim.radio().live.slots[0].scanning);
    let scan = ph.watch(&mut r, 30_000);
    let scan_ev = ph.events;
    r.press(key("SIDE2")).wait(500);
    ph.watch(&mut r, 2000);
    // a menu opened: how long until the phone has it, and its bytes
    r.press(key("OK"));
    let (mut ms, mut bytes) = (0, 0);
    (ph.bytes, ph.events) = (0, 0);
    while !ph.in_step(&r) && ms < 10_000 {
        r.wait(20);
        ms += 20;
        ph.pump(&mut r);
    }
    bytes += ph.bytes;
    assert!(ph.in_step(&r), "the menu arrived");
    println!(
        "Live View over Bluetooth: idle home {idle:.1} B/s ({idle_ev} updates in 60 s), C scanning the airband {scan_c:.1} B/s ({scan_c_ev} in 30 s), A scanning {scan:.1} B/s ({scan_ev} in 30 s), menu opened {bytes} B in {} events",
        ph.events
    );
    assert!(idle < 60.0, "idle home: {idle} B/s");
    assert!(scan_c <= 210.0 && scan <= 210.0, "the rate cap holds: {scan_c}, {scan} B/s");
    assert!(bytes < 400, "the menu: {bytes} B");
    // HELLO ends the subscription: nothing more
    ph.ask(&mut r, msg::HELLO, &[PROTOCOL]);
    r.press(key("BACK"));
    ph.watch(&mut r, 2000);
    assert_eq!(ph.events, 0);
}
