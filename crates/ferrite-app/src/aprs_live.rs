//! The radio's APRS task logic (manual chapter 25, roadmap 5b-5d): AX.25 frames from the AFSK decoder in, frames
//! for the modulator out. The heard list (APRS-3, APRS-4: RAM only), messages to us acked (APRS-6), ours tried
//! on APRS-13's schedule until an ack, beacons by the beacon mode (APRS-24) and after voice (APRS-27), the
//! fill-in digipeater (APRS-12, APRS-26: off unless asked), the path (0 5 6 1), APRS-9's 30 s spacing and
//! APRS-20's wait for the band relay. Pure logic: the firmware's UI
//! task feeds it frames and ticks it once a second (the store and the App are there); the host tests run the
//! same code on recorded audio. The simulators keep their made-up traffic (`sample::aprs`).
//!
//! Frames are the AX.25 bytes without the FCS, as `uafsk`'s demodulator gives them and its modulator takes them.

use crate::aprs::{beacon_due, retry_wait, AprsCfg, LastBeacon, HEARD_MAX, INFO_MAX, TRIES};
use crate::data::{Beacon, Fix, MsgState, Pos, Station, Sym};
use core::cell::Cell;
use ferrite_ui::text::Buf;
/// The APRS library, for the firmware's KISS TNC (chapter 26).
pub use uaprs;
use uaprs::ax25::{Address, Frame};
use uaprs::parse::{self, MessageKind, Packet};

/// The largest frame: 10 addresses, control, PID and [`INFO_MAX`] bytes of information (`uafsk`'s HDLC limit
/// less the FCS).
pub const FRAME_MAX: usize = 328;

/// Our messages being tried at once, the acks owed, and the frames remembered for the digipeater's duplicate
/// check.
const OUT: usize = 4;
const ACKS: usize = 4;
const DUPES: usize = 16;
/// The messages to us remembered (sender, number), so a repeat is acked again without a second inbox entry.
const SEEN: usize = 4;
/// APRS-9: automatic transmissions at least this far apart; a message's first try (PTT's or Send's), acks,
/// digipeats and KISS frames are exempt.
const AUTO_GAP_S: u32 = 30;
/// The digipeater: a frame heard again within this long isn't repeated, and at most one digipeat this often
/// (decided Oct 7, 2026, chapter 25).
const DUPE_S: u32 = 30;
const DIGI_GAP_S: u32 = 5;
/// The destination ("tocall") that names the software: APZ is experimental, FER for Ferrite.
const TOCALL: &[u8] = b"APZFER";

/// A station heard: what the heard list and the station screen show, about 110 bytes.
#[derive(Clone, Copy)]
struct Heard {
    call: Buf<10>,
    sym: Sym,
    pos: Option<Pos>,
    at_s: u32,
    /// The digipeater it came through (the last used one), empty when heard direct.
    via: Buf<10>,
    course: Option<(u16, u16)>,
    alt_m: Option<i32>,
    times: u16,
    comment: Buf<40>,
}

#[derive(Clone, Copy)]
struct Out {
    peer: Buf<10>,
    text: Buf<67>,
    id: u8,
    tries: u8,
    next_s: u32,
}

#[derive(Clone, Copy)]
struct AckDue {
    peer: Buf<10>,
    id: Buf<5>,
}

/// What a received frame did, for the App (inbox, toasts) and the store (messages, the log).
#[derive(Clone, Copy, Default)]
pub struct Rx {
    /// A message to us (APRS-6): notify, save, log.
    pub inbound: Option<(Buf<10>, Buf<67>)>,
    /// One of ours moved on: acked or rejected (failed), for the store.
    pub state: Option<(Buf<10>, Buf<67>, MsgState)>,
    /// The heard list changed.
    pub changed: bool,
}

/// What a second did.
#[derive(Clone, Copy, Default)]
pub struct Tick {
    /// A frame to send: its length in the caller's buffer (0: none), what it is (for a refusal's toast), and
    /// whether APRS-9's spacing applies.
    pub len: usize,
    pub what: &'static str,
    /// Our messages that moved on (tried again, failed), for the store.
    pub states: [Option<(Buf<10>, Buf<67>, MsgState)>; 2],
    pub changed: bool,
}

/// The APRS task's state: about 3.3 KB on the radio (4 KB on a 64-bit PC, where text lengths are 8 bytes), most
/// of it the heard list ([`HEARD_MAX`] stations of about 110 bytes, each with its 40-character comment for
/// APRS-23) and the digipeater's waiting frame.
pub struct AprsLive {
    heard: [Cell<Option<Heard>>; HEARD_MAX as usize],
    out: [Cell<Option<Out>>; OUT],
    next_id: Cell<u8>,
    acks: [Cell<Option<AckDue>>; ACKS],
    seen: [Cell<Option<(Buf<10>, Buf<5>)>>; SEEN],
    dupes: [Cell<(u16, u32)>; DUPES],
    dupe_at: Cell<u8>,
    /// A frame to digipeat, waiting for its turn, and the second it may go (0 5 8 3's wait).
    digi: Cell<Option<([u8; FRAME_MAX], u16)>>,
    digi_at: Cell<u32>,
    last_digi: Cell<Option<u32>>,
    last_auto: Cell<Option<u32>>,
    beacon_last: Cell<Option<LastBeacon>>,
    beacon_now: Cell<bool>,
    /// APRS-27: a beacon owed after a voice transmission (kept to APRS-9's spacing, unlike "beacon now").
    after_voice: Cell<bool>,
    waiting: Cell<bool>,
    on_since: Cell<Option<u32>>,
    now_s: Cell<u32>,
}

impl Default for AprsLive {
    fn default() -> Self {
        Self::new()
    }
}

impl AprsLive {
    pub const fn new() -> Self {
        AprsLive {
            heard: [const { Cell::new(None) }; HEARD_MAX as usize],
            out: [const { Cell::new(None) }; OUT],
            next_id: Cell::new(1),
            acks: [const { Cell::new(None) }; ACKS],
            seen: [const { Cell::new(None) }; SEEN],
            dupes: [const { Cell::new((0, 0)) }; DUPES],
            dupe_at: Cell::new(0),
            digi: Cell::new(None),
            digi_at: Cell::new(0),
            last_digi: Cell::new(None),
            last_auto: Cell::new(None),
            beacon_last: Cell::new(None),
            beacon_now: Cell::new(false),
            after_voice: Cell::new(false),
            waiting: Cell::new(false),
            on_since: Cell::new(None),
            now_s: Cell::new(0),
        }
    }

    // ---- the heard list (APRS-3, APRS-4) ----

    pub fn stations(&self) -> u16 {
        self.heard.iter().take_while(|h| h.get().is_some()).count() as u16
    }

    pub fn station(&self, i: u16) -> Option<Station> {
        let h = self.heard.get(i as usize)?.get()?;
        let mut via = Buf::<24>::new();
        if !h.via.is_empty() {
            via.push(h.via.as_bytes());
            via.push(b"*");
        }
        Some(Station {
            call: h.call,
            sym: h.sym,
            pos: h.pos,
            ago_s: self.now_s.get().saturating_sub(h.at_s),
            via,
            course: h.course,
            alt_m: h.alt_m,
            times: h.times,
            comment: h.comment,
            msg: Buf::new(),
            msg_min: None,
        })
    }

    pub fn clear_heard(&self) {
        self.heard.iter().for_each(|h| h.set(None));
    }

    /// The reset (BOOT-14): the heard list, our messages being tried and the beacon timing go.
    pub fn clear(&self) {
        self.clear_heard();
        self.out.iter().for_each(|o| o.set(None));
        self.acks.iter().for_each(|a| a.set(None));
        self.beacon_last.set(None);
        self.after_voice.set(false);
        self.digi.set(None);
    }

    /// A station heard now: to the front, its old entry (if any) gone, the oldest dropped when full.
    fn hear(&self, mut h: Heard) {
        let old = self.heard.iter().position(|c| c.get().is_some_and(|o| o.call == h.call));
        if let Some(o) = old.and_then(|i| self.heard[i].get()) {
            h.times = o.times.saturating_add(1);
            if h.pos.is_none() {
                (h.pos, h.course, h.alt_m, h.sym) = (o.pos, o.course, o.alt_m, o.sym);
            }
            if h.comment.is_empty() {
                h.comment = o.comment;
            }
        }
        let end = old.unwrap_or(self.heard.len() - 1);
        for i in (1..=end).rev() {
            self.heard[i].set(self.heard[i - 1].get());
        }
        self.heard[0].set(Some(h));
    }

    // ---- messages (APRS-6, APRS-13) ----

    /// Our message (the store's Send): its first try at the next tick, exempt from APRS-9's spacing.
    pub fn send(&self, to: &[u8], text: &[u8]) -> bool {
        let Some(slot) = self.out.iter().find(|o| o.get().is_none()) else { return false };
        let id = self.next_id.get();
        self.next_id.set(if id >= 99 { 1 } else { id + 1 });
        slot.set(Some(Out { peer: Buf::from(to), text: Buf::from(text), id, tries: 0, next_s: self.now_s.get() }));
        true
    }

    pub fn beacon_now(&self) {
        self.beacon_now.set(true);
    }

    /// A voice transmission ended (APRS-27): a beacon at the next turn that APRS-9's spacing allows.
    pub fn after_voice(&self) {
        self.after_voice.set(true);
    }

    /// The beacon's state for the APRS card (APRS-17, APRS-20).
    pub fn beacon(&self, cfg: &AprsCfg) -> Beacon {
        let next_s = cfg.every_min().map(|m| match self.beacon_last.get() {
            Some(l) => (l.at_s + m as u32 * 60).saturating_sub(self.now_s.get()),
            None => 0,
        });
        Beacon { waiting: self.waiting.get(), next_s: next_s.filter(|_| self.on_since.get().is_some()) }
    }

    // ---- receive ----

    /// A frame from the decoder. `me` is our callsign (empty: none set, nothing is acked or digipeated, APRS-8);
    /// `cfg` has the digipeater's settings (0 5 8, off by default: APRS-12, APRS-26).
    pub fn receive(&self, frame: &[u8], now_s: u32, me: &[u8], cfg: &AprsCfg) -> Rx {
        self.now_s.set(now_s);
        let mut rx = Rx::default();
        let Some(f) = Frame::parse(frame) else { return rx };
        if f.info.len() > INFO_MAX {
            return rx; // APRS-25
        }
        let my = Address::from_text(me);
        if my.is_some_and(|m| f.src.same(&m)) {
            return rx; // our own, digipeated back
        }
        let dupe = self.dupe(&f, now_s);
        if cfg.digi_on() && !dupe {
            if let Some(m) = my {
                if self.digipeat(&f, m, cfg.digi_mycall()) {
                    self.digi_at.set(now_s + cfg.digi_wait_s() as u32);
                }
            }
        }
        let mut call = Buf::<10>::new();
        f.src.emit(&mut |c| {
            call.push(&[c]);
        });
        let mut via = Buf::<10>::new();
        if let Some(d) = f.digis().iter().rev().find(|d| d.repeated) {
            d.emit(&mut |c| {
                via.push(&[c]);
            });
        }
        let mut h = Heard { call, sym: Sym::Ht, pos: None, at_s: now_s, via, course: None, alt_m: None, times: 1, comment: Buf::new() };
        let mut dest = [0u8; 6];
        let dc = f.dest.call();
        dest[..dc.len()].copy_from_slice(dc);
        match parse::parse(&dest[..dc.len()], f.info) {
            Ok(Packet::Position { pos, comment, .. } | Packet::Object { pos, comment, .. } | Packet::Item { pos, comment, .. }) => {
                h.pos = Some(Pos { lat: pos.lat.saturating_mul(10), lon: pos.lon.saturating_mul(10) });
                h.sym = sym(pos.table, pos.code);
                h.course = pos.course.map(|c| (c, pos.speed.map_or(0, |k| (k as u32 * 1852 / 1000) as u16)));
                h.alt_m = pos.altitude.map(|ft| ft * 3048 / 10_000);
                // APRS-23: a weather report's fields stay at the start of the comment for the station screen.
                for b in comment.bytes().take(40) {
                    h.comment.push(&[b]);
                }
                self.hear(h);
                rx.changed = true;
            }
            Ok(Packet::Status { text, .. }) => {
                h.comment = Buf::from(&text[..text.len().min(40)]);
                self.hear(h);
                rx.changed = true;
            }
            Ok(Packet::Message(m)) => {
                self.hear(h);
                rx.changed = true;
                if my.is_some_and(|me| Address::from_text(m.addressee).is_some_and(|a| a.same(&me))) {
                    self.to_us(&call, m, &mut rx);
                }
            }
            Ok(_) | Err(_) => {
                self.hear(h);
                rx.changed = true;
            }
        }
        rx
    }

    fn to_us(&self, from: &Buf<10>, m: parse::Message, rx: &mut Rx) {
        match m.kind {
            MessageKind::Message => {
                let id = m.id.map(Buf::<5>::from);
                let repeat = id.is_some_and(|i| self.seen.iter().any(|s| s.get() == Some((*from, i))));
                if let Some(i) = id {
                    self.owe_ack(from, &i);
                    if !repeat {
                        for k in (1..SEEN).rev() {
                            self.seen[k].set(self.seen[k - 1].get());
                        }
                        self.seen[0].set(Some((*from, i)));
                    }
                }
                if !repeat {
                    rx.inbound = Some((*from, Buf::from(&m.text[..m.text.len().min(67)])));
                }
                // APRS 1.1 reply-ack: it also acknowledges one of ours.
                if let Some(a) = m.reply_ack {
                    self.acked(from, a, MsgState::Acked, rx);
                }
            }
            MessageKind::Ack => {
                if let Some(a) = m.id {
                    self.acked(from, a, MsgState::Acked, rx);
                }
            }
            MessageKind::Rej => {
                if let Some(a) = m.id {
                    self.acked(from, a, MsgState::Failed, rx);
                }
            }
        }
    }

    fn acked(&self, from: &Buf<10>, id: &[u8], state: MsgState, rx: &mut Rx) {
        for c in &self.out {
            if let Some(o) = c.get().filter(|o| o.peer.as_bytes().eq_ignore_ascii_case(from.as_bytes()) && num(o.id).as_bytes() == id) {
                c.set(None);
                rx.state = Some((o.peer, o.text, state));
            }
        }
    }

    fn owe_ack(&self, peer: &Buf<10>, id: &Buf<5>) {
        let a = AckDue { peer: *peer, id: *id };
        if self.acks.iter().any(|c| c.get().is_some_and(|x| x.peer == a.peer && x.id == a.id)) {
            return;
        }
        if let Some(c) = self.acks.iter().find(|c| c.get().is_none()) {
            c.set(Some(a));
        }
    }

    /// The duplicate check: source and information seen within [`DUPE_S`].
    fn dupe(&self, f: &Frame, now_s: u32) -> bool {
        let mut h = 0xFFFFu16;
        let mut add = |b: u8| h = crc16(h, b);
        f.src.call().iter().for_each(|&b| add(b));
        add(f.src.ssid);
        f.info.iter().for_each(|&b| add(b));
        let seen = self.dupes.iter().any(|d| {
            let (k, t) = d.get();
            k == h && t != 0 && now_s.wrapping_sub(t) < DUPE_S
        });
        let i = self.dupe_at.get() as usize;
        self.dupes[i].set((h, now_s.max(1)));
        self.dupe_at.set(((i + 1) % DUPES) as u8);
        seen
    }

    /// A fill-in digipeater (APRS-26): the first unused path entry, when it is WIDE1-1 (or our callsign, with
    /// `mycall`), becomes our call, used. True when a frame is waiting to go.
    fn digipeat(&self, f: &Frame, me: Address, mycall: bool) -> bool {
        let wide1 = Address::from_text(b"WIDE1-1");
        let Some(i) = f.digis().iter().position(|d| !d.repeated) else { return false };
        let next = &f.digis()[i];
        if !(wide1.is_some_and(|w| next.same(&w)) || mycall && next.same(&me)) {
            return false;
        }
        let mut path = [me; uaprs::ax25::MAX_DIGIS];
        let n = f.digis().len();
        path[..n].copy_from_slice(f.digis());
        let mut used = me;
        used.repeated = true;
        path[i] = used;
        let mut buf = [0u8; FRAME_MAX];
        let Some(len) = Frame::new(f.src, f.dest, &path[..n], f.info).and_then(|g| g.write(&mut buf)) else { return false };
        self.digi.set(Some((buf, len as u16)));
        true
    }

    // ---- transmit ----

    /// A second of the APRS task. `on`: APRS mode (APRS-15); `relay_ok`: slot C can transmit with the band relay
    /// where the active slot holds it (APRS-20); `me` our callsign (APRS-8); `fix` the GPS's (beacons, APRS-24).
    /// A frame to send is written to `out`; the caller keys slot C, plays it, unkeys.
    pub fn tick(&self, now_s: u32, on: bool, relay_ok: bool, me: &[u8], fix: Option<Fix>, cfg: &AprsCfg, out: &mut [u8; FRAME_MAX]) -> Tick {
        self.now_s.set(now_s);
        let mut t = Tick::default();
        if !on {
            // Without APRS mode nothing is sent (APRS-22 only decodes): acks owed meanwhile are dropped, so
            // turning the mode on later doesn't send stale ones.
            self.acks.iter().for_each(|a| a.set(None));
            self.on_since.set(None);
            t.changed = self.waiting.replace(false);
            return t;
        }
        if self.on_since.get().is_none() {
            self.on_since.set(Some(now_s));
        }
        // Ours that ran out of tries.
        for c in &self.out {
            if let Some(o) = c.get().filter(|o| o.tries >= TRIES && now_s >= o.next_s) {
                c.set(None);
                push_state(&mut t, (o.peer, o.text, MsgState::Failed));
                t.changed = true;
            }
        }
        let beacon = self.beacon_now.get() || beacon_due(cfg, self.beacon_last.get(), now_s, fix);
        let waiting = beacon && !relay_ok;
        t.changed |= self.waiting.replace(waiting) != waiting;
        let Some(my) = Address::from_text(me).filter(|_| relay_ok) else { return t };
        let auto_ok = self.last_auto.get().is_none_or(|l| now_s.wrapping_sub(l) >= AUTO_GAP_S);
        let mut info = [0u8; INFO_MAX];
        // Acks first (they're owed), then a digipeat, then ours, then the beacon: one transmission a second.
        if let Some(c) = self.acks.iter().find(|c| c.get().is_some()) {
            let a = c.get().unwrap();
            c.set(None);
            if let Some(n) = uaprs::build::ack(a.peer.as_bytes(), a.id.as_bytes(), &mut info) {
                return self.framed(my, &info[..n], "Ack", cfg, out, t);
            }
        }
        if let Some((buf, len)) = self.digi.take() {
            if now_s < self.digi_at.get() {
                self.digi.set(Some((buf, len))); // 0 5 8 3's wait
            } else if self.last_digi.get().is_none_or(|l| now_s.wrapping_sub(l) >= DIGI_GAP_S) {
                self.last_digi.set(Some(now_s));
                out[..len as usize].copy_from_slice(&buf[..len as usize]);
                return Tick { len: len as usize, what: "Digipeat", ..t };
            }
            // Over the rate: dropped, not delayed (the next station will carry it).
        }
        for c in &self.out {
            let Some(mut o) = c.get().filter(|o| o.tries < TRIES && now_s >= o.next_s) else { continue };
            if o.tries > 0 && !auto_ok {
                continue;
            }
            o.tries += 1;
            o.next_s = now_s + retry_wait(o.tries) as u32;
            c.set(Some(o));
            if o.tries > 1 {
                self.last_auto.set(Some(now_s));
                push_state(&mut t, (o.peer, o.text, MsgState::Retrying(o.tries)));
            }
            let id = num(o.id);
            if let Some(n) = uaprs::build::message(o.peer.as_bytes(), o.text.as_bytes(), Some(id.as_bytes()), &mut info) {
                t.changed = true;
                return self.framed(my, &info[..n], "Message", cfg, out, t);
            }
        }
        let beacon = beacon || self.after_voice.get() && auto_ok;
        if beacon && (auto_ok || self.beacon_now.get()) {
            self.after_voice.set(false);
            let Some(f) = fix else {
                self.beacon_now.set(false);
                return Tick { what: "Beacon: no position", ..t };
            };
            self.beacon_now.set(false);
            self.beacon_last.set(Some(LastBeacon::new(now_s, Some(f))));
            self.last_auto.set(Some(now_s));
            let r = uaprs::build::Report {
                lat: f.pos.lat / 10,
                lon: f.pos.lon / 10,
                table: b'/',
                code: b'[',
                course_speed: (f.speed_kmh > 0).then(|| (f.heading.clamp(1, 360), (f.speed_kmh as u32 * 1000 / 1852) as u16)),
                altitude: f.three_d.then_some(f.alt_m * 10_000 / 3048),
                messaging: true,
                timestamp: None,
                comment: b"",
            };
            if let Some(n) = uaprs::build::position(&r, &mut info) {
                t.changed = true;
                return self.framed(my, &info[..n], "Beacon", cfg, out, t);
            }
        }
        t
    }

    /// Our frame with 0 5 6 1's path (APRS-26).
    fn framed(&self, me: Address, info: &[u8], what: &'static str, cfg: &AprsCfg, out: &mut [u8; FRAME_MAX], t: Tick) -> Tick {
        let names = cfg.path();
        let mut path = [me; 2];
        for (a, n) in path.iter_mut().zip(names) {
            *a = Address::from_text(n).unwrap_or(me);
        }
        let dest = Address::from_text(TOCALL).unwrap_or(me);
        let len = Frame::new(me, dest, &path[..names.len().min(2)], info).and_then(|f| f.write(out)).unwrap_or(0);
        Tick { len, what, ..t }
    }
}

fn push_state(t: &mut Tick, s: (Buf<10>, Buf<67>, MsgState)) {
    if let Some(x) = t.states.iter_mut().find(|x| x.is_none()) {
        *x = Some(s);
    }
}

/// A message number as its digits.
fn num(id: u8) -> Buf<5> {
    let mut b = Buf::<5>::new();
    if id >= 10 {
        b.push(&[b'0' + id / 10]);
    }
    b.push(&[b'0' + id % 10]);
    b
}

/// The heard list's symbol names (APRS-3) from the APRS symbol table and code.
fn sym(table: u8, code: u8) -> Sym {
    match (table == b'/', code) {
        (_, b'>') | (_, b'j') | (_, b'v') => Sym::Car,
        (true, b'-') => Sym::Home,
        (_, b'#') => Sym::Digi,
        (_, b'_') => Sym::Wx,
        (false, b'&') | (true, b'&') => Sym::IGate,
        (_, b'k') | (_, b'u') => Sym::Truck,
        (_, b's') | (_, b'Y') => Sym::Boat,
        _ => Sym::Ht,
    }
}

/// CRC-16/CCITT, one byte.
fn crc16(mut crc: u16, b: u8) -> u16 {
    crc ^= (b as u16) << 8;
    for _ in 0..8 {
        crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(src: &[u8], dest: &[u8], path: &[&[u8]], info: &[u8]) -> ([u8; FRAME_MAX], usize) {
        let p: std::vec::Vec<Address> = path.iter().map(|p| Address::from_text(p).unwrap()).collect();
        let mut b = [0u8; FRAME_MAX];
        let n = Frame::new(Address::from_text(src).unwrap(), Address::from_text(dest).unwrap(), &p, info).unwrap().write(&mut b).unwrap();
        (b, n)
    }

    fn fix() -> Fix {
        Fix { pos: Pos { lat: 454_252_000, lon: -756_901_000 }, three_d: false, alt_m: 0, speed_kmh: 0, heading: 0, hdop_x10: 10, used: 8, age_s: 0, utc_s: 0 }
    }

    #[test]
    fn heard_list_newest_first_and_capped() {
        let a = AprsLive::new();
        for i in 0..25u32 {
            let call = std::format!("VE3A{}", (b'A' + i as u8) as char);
            let (f, n) = frame(call.as_bytes(), b"APRS", &[b"WIDE1-1*"], b"!4525.51N/07554.20W>Mobile");
            assert!(a.receive(&f[..n], i, b"VE3XYZ", &AprsCfg::DEFAULT).changed);
        }
        assert_eq!(a.stations(), HEARD_MAX);
        let s = a.station(0).unwrap();
        assert_eq!(s.call.as_bytes(), b"VE3AY");
        assert_eq!(s.sym, Sym::Car);
        assert!(s.pos.is_some_and(|p| (p.lat - 454_251_666).abs() < 200));
        assert_eq!(s.via.as_bytes(), b"WIDE1-1*");
        assert_eq!(s.comment.as_bytes(), b"Mobile");
        // Heard again: to the front, counted, not twice.
        let (f, n) = frame(b"VE3AM", b"APRS", &[], b">On the air");
        a.receive(&f[..n], 30, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.station(0).unwrap().call.as_bytes(), b"VE3AM");
        assert_eq!(a.station(0).unwrap().times, 2);
        assert!(a.station(0).unwrap().pos.is_some(), "a status keeps the last position");
        assert_eq!(a.stations(), HEARD_MAX);
    }

    #[test]
    fn message_to_us_is_acked_once() {
        let a = AprsLive::new();
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :On my way{12");
        let rx = a.receive(&f[..n], 100, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(rx.inbound.unwrap().1.as_bytes(), b"On my way");
        let mut out = [0u8; FRAME_MAX];
        let t = a.tick(100, true, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.what, "Ack");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!(g.info, b":VE3KPW-9 :ack12");
        assert_eq!(g.digis().len(), 2);
        // The same message again (its sender didn't hear our ack): acked again, no second inbox entry.
        let rx = a.receive(&f[..n], 130, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert!(rx.inbound.is_none());
        assert_eq!(a.tick(131, true, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).what, "Ack");
        // Not to us: nothing.
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VA3ABC   :Hi{3");
        assert!(a.receive(&f[..n], 140, b"VE3XYZ", &AprsCfg::DEFAULT).inbound.is_none());
    }

    #[test]
    fn ours_tried_until_acked() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg::DEFAULT;
        assert!(a.send(b"VE3KPW-9", b"QSL"));
        let t = a.tick(0, true, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        assert_eq!(Frame::parse(&out[..t.len]).unwrap().info, b":VE3KPW-9 :QSL{1");
        assert_eq!(a.tick(10, true, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "next try 30 s on");
        let t = a.tick(30, true, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        assert!(matches!(t.states[0], Some((_, _, MsgState::Retrying(2)))));
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :ack1");
        let rx = a.receive(&f[..n], 40, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert!(matches!(rx.state, Some((_, _, MsgState::Acked))));
        assert_eq!(a.tick(200, true, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "acked: no more tries");
        // Never acked: five tries, then failed.
        a.send(b"VA3ABC", b"Anyone?");
        let mut tries = 0;
        let mut failed = false;
        for s in 300..1_200 {
            let t = a.tick(s, true, true, b"VE3XYZ", None, &cfg, &mut out);
            tries += (t.what == "Message") as u32;
            failed |= t.states.iter().flatten().any(|s| s.2 == MsgState::Failed);
        }
        assert_eq!(tries, 5);
        assert!(failed);
    }

    #[test]
    fn beacons_wait_for_the_relay_and_need_a_position() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg::DEFAULT;
        a.beacon_now();
        let t = a.tick(0, true, false, b"VE3XYZ", Some(fix()), &cfg, &mut out);
        assert_eq!(t.len, 0);
        assert!(a.beacon(&cfg).waiting, "APRS-20: waits for the relay");
        let t = a.tick(1, true, true, b"VE3XYZ", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!(g.dest.call(), b"APZFER");
        match parse::parse(b"APZFER", g.info) {
            Ok(Packet::Position { pos, .. }) => assert!((pos.lat - 45_425_200).abs() < 200 && (pos.lon + 75_690_100).abs() < 200),
            other => panic!("{other:?}"),
        }
        a.beacon_now();
        assert_eq!(a.tick(2, true, true, b"VE3XYZ", None, &cfg, &mut out).what, "Beacon: no position");
        a.beacon_now();
        assert_eq!(a.tick(3, true, true, b"", Some(fix()), &cfg, &mut out).len, 0, "APRS-8: no callsign, nothing sent");
    }

    const DIGI: AprsCfg = AprsCfg { digi: crate::aprs::DIGI_ON, ..AprsCfg::DEFAULT };

    #[test]
    fn digipeater_fills_in_once() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b">Walking");
        a.receive(&f[..n], 10, b"VE3XYZ", &DIGI);
        let t = a.tick(10, true, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.what, "Digipeat");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert!(g.digis()[0].repeated && g.digis()[0].call() == b"VE3XYZ");
        assert!(!g.digis()[1].repeated);
        // The same frame again within 30 s, or one already past WIDE1: not repeated.
        a.receive(&f[..n], 20, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(20, true, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0);
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1*", b"WIDE2-1"], b">Later");
        a.receive(&f[..n], 50, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(50, true, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0);
        // Off by default (APRS-12).
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b">Again");
        a.receive(&f[..n], 90, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.tick(90, true, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0);
    }

    /// APRS-26: our callsign as an alias when asked, and 0 5 8 3's wait before relaying.
    #[test]
    fn digipeater_mycall_alias_and_wait() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"VE3XYZ", b"WIDE2-1"], b">To me");
        a.receive(&f[..n], 10, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(10, true, true, b"VE3XYZ", None, &DIGI, &mut out).len, 0, "my call isn't an alias by default");
        let cfg = AprsCfg { digi: crate::aprs::DIGI_ON | crate::aprs::DIGI_MYCALL | 3 << 4, ..AprsCfg::DEFAULT };
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"VE3XYZ", b"WIDE2-1"], b">To me again");
        a.receive(&f[..n], 40, b"VE3XYZ", &cfg);
        assert_eq!(a.tick(41, true, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "waits 3 s");
        let t = a.tick(43, true, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Digipeat");
        assert!(Frame::parse(&out[..t.len]).unwrap().digis()[0].repeated);
    }

    /// 0 5 6 1: the path our frames ask for.
    #[test]
    fn path_setting() {
        let mut out = [0u8; FRAME_MAX];
        for (k, want) in [(0u8, 0usize), (1, 1), (2, 2)] {
            let a = AprsLive::new();
            let cfg = AprsCfg { path: k, ..AprsCfg::DEFAULT };
            a.beacon_now();
            let t = a.tick(1, true, true, b"VE3XYZ", Some(fix()), &cfg, &mut out);
            assert_eq!(t.what, "Beacon");
            let g = Frame::parse(&out[..t.len]).unwrap();
            assert_eq!(g.digis().len(), want);
            if want > 0 {
                assert_eq!(g.digis()[0].call(), b"WIDE1");
            }
        }
    }

    /// APRS-27: a beacon after voice, kept to APRS-9's 30 s spacing.
    #[test]
    fn beacon_after_voice() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg { beacon: 1, after_voice: true, ..AprsCfg::DEFAULT }; // manual beacons only
        a.after_voice();
        assert_eq!(a.tick(100, true, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Beacon");
        a.after_voice();
        assert_eq!(a.tick(110, true, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0, "30 s spacing");
        assert_eq!(a.tick(131, true, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Beacon");
        assert_eq!(a.tick(170, true, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0, "once per transmission");
    }

    #[test]
    fn size() {
        let n = core::mem::size_of::<AprsLive>();
        std::println!("AprsLive: {n} B, heard entry {} B, out {} B", core::mem::size_of::<Option<Heard>>(), core::mem::size_of::<Option<Out>>());
        assert!(n <= 4_200, "{n}");
    }
}
