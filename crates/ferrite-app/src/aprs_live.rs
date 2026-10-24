//! The radio's APRS task logic (manual chapter 25, roadmap 5b-5d): AX.25 frames from the AFSK decoder in, frames
//! for the modulator out. The heard list (APRS-3, APRS-4: RAM only), messages to us acked (APRS-6), ours tried
//! on APRS-13's schedule until an ack, beacons by the beacon mode (APRS-24) and after voice (APRS-27), the
//! fill-in digipeater (APRS-12, APRS-26: off unless asked), the path (0 5 9 1), APRS-9's 30 s spacing and
//! APRS-20's wait for the band relay; a message heard again stored once (APRS-32), queries answered (APRS-33,
//! APRS-35), rejections (APRS-34). Pure logic: the firmware's UI task feeds it frames and ticks it once a second
//! (the store and the App are there); the host tests run the same code on recorded audio. The simulators keep
//! their made-up traffic (`sample::aprs`).
//!
//! Frames are the AX.25 bytes without the FCS, as `uafsk`'s demodulator gives them and its modulator takes them.

use crate::aprs::{beacon_due, retry_wait, AprsCfg, LastBeacon, HEARD_MAX, INFO_MAX, TRIES};
use crate::data::{Beacon, Fix, MsgState, Pos, Station, Sym};
use core::cell::Cell;
use ferrite_core::liveview::{fnv, FNV0};
use ferrite_ui::text::Buf;
use ufmt::uwrite;
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
/// APRS-32: the messages to us remembered, 8 bytes each (a key of the sender, the number and the text, and when
/// first heard), so a repeat is acked again without a second inbox entry or ring; 24 entries, 192 bytes.
const SEEN: usize = 24;
/// APRS-32: a message without a number heard again within this long is the same one (a second digipeater's copy).
const SEEN_PLAIN_S: u32 = 30;
/// APRS-35: general queries (`?APRS?`) answered at most this often, after a random wait of
/// [`GENERAL_WAIT_S`] plus up to [`GENERAL_SPREAD_S`]; an answer not sent within [`QUERY_TTL_S`] of its time is
/// dropped.
const GENERAL_GAP_S: u32 = 300;
const GENERAL_WAIT_S: u32 = 5;
const GENERAL_SPREAD_S: u32 = 56;
const QUERY_TTL_S: u32 = 120;
/// APRS-9: automatic transmissions at least this far apart; a message's first try (PTT's or Send's), acks,
/// digipeats and KISS frames are exempt.
const AUTO_GAP_S: u32 = 30;
/// The digipeater: a frame heard again within this long isn't repeated, and at most one digipeat this often
/// (decided Oct 7, 2026, chapter 25).
const DUPE_S: u32 = 30;
const DIGI_GAP_S: u32 = 5;
/// GPS-33: an automatic beacon whose fix is older than its interval waits this long for a fresh one (the GPS
/// manager wakes the module for it), then is skipped until the next.
const FIX_WAIT_S: u32 = 120;
/// The destination ("tocall") that names the software: APZ is experimental, FER for Ferrite.
const TOCALL: [u8; 6] = *b"APZFER";

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
    /// APRS-44: its last frame came as FX.25: the bytes corrected + 1 (0: plain AX.25).
    fx25: u8,
}

#[derive(Clone, Copy)]
struct Out {
    peer: Buf<10>,
    text: Buf<67>,
    id: u8,
    tries: u8,
    next_s: u32,
}

/// APRS-33: what a directed query gets.
#[derive(Clone, Copy)]
enum Answer {
    /// Our position report (`?APRSP`, `?APRS?`).
    Position,
    /// Our status report (`?APRSS`).
    Status,
    /// A message back, without a number (`?APRSD`, `?APRSH`, `?APRST`, `?PING?`, `?VER`).
    Text(Buf<67>),
}

#[derive(Clone, Copy)]
struct Query {
    to: Buf<10>,
    answer: Answer,
    at_s: u32,
}

/// APRS-35: a general query (`?APRS?`) to answer at `due_s`, with its footprint (centre in millionths of a
/// degree, radius in metres) if it gave one.
#[derive(Clone, Copy)]
struct General {
    due_s: u32,
    foot: Option<((i32, i32), u32)>,
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
    /// Its message number (empty: none), for the store's check after a power cycle (APRS-32).
    pub inbound_id: Buf<5>,
    /// One of ours moved on: acked or rejected (APRS-34), for the store.
    pub state: Option<(Buf<10>, Buf<67>, MsgState)>,
    /// The heard list changed.
    pub changed: bool,
    /// APRS-36: our own frame, heard back for the first time since we sent it, and the digipeater that sent it.
    pub echo: Option<Buf<10>>,
}

/// What the APRS task may send (APRS-22, APRS-29).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sending {
    /// Nothing: APRS mode off, no beacons (APRS-22 only decodes).
    Nothing,
    /// Beacons only: APRS mode off, automatic beacons on (APRS-29): no acks, messages or digipeats.
    Beacons,
    /// Everything: APRS mode on.
    All,
}

/// APRS-10, APRS-29 (decision 181): a frame waits for a clear moment on the APRS frequency (p-persistence on C's
/// squelch) at most this long, then it isn't sent.
pub const CLEAR_WAIT_MS: u32 = 5_000;

/// APRS-10, APRS-29: a frame waiting to go, sent in the first clear moment within [`CLEAR_WAIT_MS`] (the KISS
/// TNC's p-persistence, `uaprs::kiss::Access`, at its defaults: persistence 63, 100 ms slots).
#[derive(Clone, Copy, Debug, Default)]
pub struct ClearWait {
    since: Option<u32>,
    access: uaprs::kiss::Access,
}

/// [`ClearWait::poll`]'s answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    Wait,
    Send,
    /// Busy for the whole window: not sent.
    GiveUp,
}

impl ClearWait {
    /// A frame is waiting since `now_ms`.
    pub fn start(&mut self, now_ms: u32) {
        *self = ClearWait { since: Some(now_ms), access: uaprs::kiss::Access::default() };
    }

    pub fn waiting(&self) -> bool {
        self.since.is_some()
    }

    /// Each turn while a frame waits: `busy` C's squelch, `random` a fresh random byte.
    pub fn poll(&mut self, now_ms: u32, busy: bool, random: u8) -> Clear {
        let Some(t) = self.since else { return Clear::Wait };
        if self.access.may_send(&uaprs::kiss::Params::default(), now_ms, busy, random) {
            self.since = None;
            Clear::Send
        } else if now_ms.wrapping_sub(t) >= CLEAR_WAIT_MS {
            self.since = None;
            Clear::GiveUp
        } else {
            Clear::Wait
        }
    }
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

/// The APRS task's state: about 6.8 KB on the radio (8.4 KB on a 64-bit PC, where text lengths are 8 bytes), most
/// of it the heard list ([`HEARD_MAX`] stations of about 110 bytes, each with its 40-character comment for
/// APRS-23) and the digipeater's waiting frame.
pub struct AprsLive {
    heard: [Cell<Option<Heard>>; HEARD_MAX as usize],
    out: [Cell<Option<Out>>; OUT],
    next_id: Cell<u8>,
    acks: [Cell<Option<AckDue>>; ACKS],
    seen: [Cell<(u32, u32)>; SEEN],
    seen_at: Cell<u8>,
    /// APRS-33: a directed query's answer waiting, a general query's, and when the last general one went.
    query: Cell<Option<Query>>,
    general: Cell<Option<General>>,
    last_general: Cell<Option<u32>>,
    dupes: [Cell<(u16, u32)>; DUPES],
    dupe_at: Cell<u8>,
    /// A frame to digipeat, waiting for its turn, and the second it may go (0 5 7 3's wait).
    digi: Cell<Option<([u8; FRAME_MAX], u16)>>,
    digi_at: Cell<u32>,
    last_digi: Cell<Option<u32>>,
    last_auto: Cell<Option<u32>>,
    beacon_last: Cell<Option<LastBeacon>>,
    beacon_now: Cell<bool>,
    /// APRS-27: a beacon owed after a voice transmission (kept to APRS-9's spacing, unlike "beacon now").
    after_voice: Cell<bool>,
    waiting: Cell<bool>,
    /// GPS-33: a due beacon waiting for a fresh fix since then.
    fix_wait: Cell<Option<u32>>,
    on_since: Cell<Option<u32>>,
    now_s: Cell<u32>,
    /// APRS-36: a frame of ours went out and hasn't been heard back yet; who repeated the last one heard back.
    echo: Cell<bool>,
    echo_by: Cell<Buf<10>>,
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
            seen: [const { Cell::new((0, 0)) }; SEEN],
            seen_at: Cell::new(0),
            query: Cell::new(None),
            general: Cell::new(None),
            last_general: Cell::new(None),
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
            fix_wait: Cell::new(None),
            on_since: Cell::new(None),
            now_s: Cell::new(0),
            echo: Cell::new(false),
            echo_by: Cell::new(Buf::new()),
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
            fx25: h.fx25.checked_sub(1),
        })
    }

    /// APRS-44: the frame just given to [`AprsLive::receive`] (or heard plain already, a duplicate) came as FX.25
    /// with `corrected` bytes repaired: its station says so (the station screen's Path row).
    pub fn note_fx25(&self, frame: &[u8], corrected: u8) {
        let Some(f) = Frame::parse(frame) else { return };
        let mut call = Buf::<10>::new();
        f.src.emit(&mut |c| {
            call.push(&[c]);
        });
        if let Some(c) = self.heard.iter().find(|c| c.get().is_some_and(|o| o.call == call)) {
            let mut h = c.get().unwrap();
            h.fx25 = corrected.saturating_add(1);
            c.set(Some(h));
        }
    }

    pub fn clear_heard(&self) {
        self.heard.iter().for_each(|h| h.set(None));
    }

    /// The reset (BOOT-14): the heard list, our messages being tried and the beacon timing go.
    pub fn clear(&self) {
        self.clear_heard();
        self.out.iter().for_each(|o| o.set(None));
        self.acks.iter().for_each(|a| a.set(None));
        self.query.set(None);
        self.general.set(None);
        self.beacon_last.set(None);
        self.after_voice.set(false);
        self.fix_wait.set(None);
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
        Beacon { waiting: self.waiting.get(), next_s: next_s.filter(|_| self.on_since.get().is_some()), digi: self.echo_by.get() }
    }

    // ---- receive ----

    /// A frame from the decoder. `me` is our callsign (empty: none set, nothing is acked or digipeated, APRS-8);
    /// `cfg` has the digipeater's settings (0 5 7, off by default: APRS-12, APRS-26).
    pub fn receive(&self, frame: &[u8], now_s: u32, me: &[u8], cfg: &AprsCfg) -> Rx {
        self.now_s.set(now_s);
        let mut rx = Rx::default();
        let Some(f) = Frame::parse(frame) else { return rx };
        if f.info.len() > INFO_MAX {
            return rx; // APRS-25
        }
        let my = Address::from_text(me);
        if my.is_some_and(|m| f.src.same(&m)) {
            // APRS-36: our own, digipeated back: the first time after a transmission, who repeated it
            let used = || f.digis().iter().rev().filter(|d| d.repeated);
            let station = used().find(|d| !d.call().starts_with(b"WIDE")).or_else(|| used().next());
            if let Some(d) = station.filter(|_| self.echo.get()) {
                let mut by = Buf::<10>::new();
                d.emit(&mut |c| {
                    by.push(&[c]);
                });
                self.echo.set(false);
                self.echo_by.set(by);
                rx.echo = Some(by);
            }
            return rx;
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
        let mut h = Heard { call, sym: Sym::Ht, pos: None, at_s: now_s, via, course: None, alt_m: None, times: 1, comment: Buf::new(), fx25: 0 };
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
                    self.to_us(&call, m, &f, dupe, now_s, &mut rx);
                }
            }
            Ok(Packet::Other { kind: b'?', data }) => {
                self.hear(h);
                rx.changed = true;
                if my.is_some() && !dupe {
                    self.general_query(data, &f, now_s);
                }
            }
            Ok(_) | Err(_) => {
                self.hear(h);
                rx.changed = true;
            }
        }
        rx
    }

    fn to_us(&self, from: &Buf<10>, m: parse::Message, f: &Frame, dupe: bool, now_s: u32, rx: &mut Rx) {
        match m.kind {
            MessageKind::Message => {
                // APRS-6, APRS-32: every copy with a number is acked (its sender retries because it missed ours).
                if let Some(i) = m.id {
                    self.owe_ack(from, &Buf::from(i));
                }
                // APRS 1.1 reply-ack: it also acknowledges one of ours.
                if let Some(a) = m.reply_ack {
                    self.acked(from, a, MsgState::Acked, rx);
                }
                // APRS-33: a query is answered (once for the copies digipeaters repeat), not kept as a message.
                if let Some(a) = self.directed(m.text, f) {
                    if let Some(answer) = a.filter(|_| !dupe && self.query.get().is_none()) {
                        self.query.set(Some(Query { to: *from, answer, at_s: now_s }));
                    }
                    return;
                }
                let text = Buf::<67>::from(&m.text[..m.text.len().min(67)]);
                if !self.seen_before(from, m.id, &text, now_s) {
                    rx.inbound = Some((*from, text));
                    rx.inbound_id = Buf::from(m.id.unwrap_or(b""));
                }
            }
            MessageKind::Ack => {
                if let Some(a) = m.id {
                    self.acked(from, a, MsgState::Acked, rx);
                }
            }
            // APRS-34: rejected ("rejNN", or "rejNN}AA" from a reply-ack station): ended at once, no more tries.
            MessageKind::Rej => {
                if let Some(a) = m.id {
                    self.acked(from, a, MsgState::Rejected, rx);
                }
            }
        }
    }

    /// APRS-32: whether this message to us was heard before: the same sender, number and text (a retry), or
    /// without a number the same sender and text within [`SEEN_PLAIN_S`]. A new one is remembered, the oldest
    /// entry making room.
    fn seen_before(&self, from: &Buf<10>, id: Option<&[u8]>, text: &Buf<67>, now_s: u32) -> bool {
        let k = fnv(fnv(fnv(fnv(FNV0, from.as_bytes()), &[0]), id.unwrap_or(b"")), &[0]);
        let mut k = fnv(k, text.as_bytes()) & !1 | id.is_some() as u32;
        if k >> 1 == 0 {
            k |= 2; // 0 is an empty entry
        }
        let hit = self.seen.iter().any(|c| {
            let (key, at) = c.get();
            key == k && (id.is_some() || now_s.wrapping_sub(at) < SEEN_PLAIN_S)
        });
        if !hit {
            let i = self.seen_at.get() as usize;
            self.seen[i].set((k, now_s));
            self.seen_at.set(((i + 1) % SEEN) as u8);
        }
        hit
    }

    /// APRS-33: a message to us that is a query: None when it isn't one, Some(None) for one we don't answer
    /// (`?APRSM`, `?APRSO`, `?WX?`, `?IGATE?`...), else the answer.
    fn directed(&self, text: &[u8], f: &Frame) -> Option<Option<Answer>> {
        let t = trim(text);
        let is = |q: &[u8]| t.eq_ignore_ascii_case(q);
        let starts = |q: &[u8]| t.len() >= q.len() && t[..q.len()].eq_ignore_ascii_case(q);
        if t.first() != Some(&b'?') {
            return None;
        }
        let mut b = Buf::<67>::new();
        Some(Some(if is(b"?APRSP") || is(b"?APRS?") {
            Answer::Position
        } else if is(b"?APRSS") {
            Answer::Status
        } else if is(b"?APRSD") {
            b.push(b"Directs=");
            for h in self.heard.iter().filter_map(|h| h.get()).filter(|h| h.via.is_empty()) {
                if b.len() + 1 + h.call.len() > 67 {
                    break;
                }
                b.push(b" ");
                b.push(h.call.as_bytes());
            }
            Answer::Text(b)
        } else if starts(b"?APRSH") {
            let call = trim(&t[6..]);
            if call.is_empty() {
                return Some(None);
            }
            let up = |c: &[u8]| -> Buf<10> {
                let mut u = Buf::<10>::new();
                c.iter().for_each(|&x| u.push(&[x.to_ascii_uppercase()]));
                u
            };
            let call = up(call);
            match self.heard.iter().filter_map(|h| h.get()).find(|h| h.call.as_bytes().eq_ignore_ascii_case(call.as_bytes())) {
                Some(h) => {
                    let min = self.now_s.get().saturating_sub(h.at_s) / 60;
                    b.push(h.call.as_bytes());
                    let _ = uwrite!(b, " heard {}x, last {} min ago", h.times, min);
                }
                None => {
                    b.push(call.as_bytes());
                    b.push(b" not heard");
                }
            }
            Answer::Text(b)
        } else if is(b"?APRST") || is(b"?PING?") {
            trace(f, &mut b);
            Answer::Text(b)
        } else if is(b"?VER") || is(b"?VER?") {
            b.push(b"Ferrite ");
            b.push(ferrite_core::VERSION.as_bytes());
            Answer::Text(b)
        } else if starts(b"?APRS") || is(b"?WX?") || is(b"?IGATE?") {
            return Some(None);
        } else {
            return None;
        }))
    }

    /// APRS-35: a general query heard (the information field after its `?`): `APRS?`, optionally with a
    /// footprint `lat,lon,radius` (degrees, miles); answered with our position after a random wait, at most once
    /// per [`GENERAL_GAP_S`]. Other general queries (`?WX?`, `?IGATE?`) aren't ours to answer.
    fn general_query(&self, data: &[u8], f: &Frame, now_s: u32) {
        let Some(rest) = data.strip_prefix(b"APRS?") else { return };
        if self.general.get().is_some() || self.last_general.get().is_some_and(|l| now_s.wrapping_sub(l) < GENERAL_GAP_S) {
            return;
        }
        let rest = trim(rest);
        let foot = if rest.is_empty() {
            None
        } else {
            match footprint(rest) {
                Some(p) => Some(p),
                None => return, // a footprint we can't read: not meant for everyone
            }
        };
        let h = fnv(fnv(FNV0, f.src.call()), f.info) ^ now_s;
        self.general.set(Some(General { due_s: now_s + GENERAL_WAIT_S + h % GENERAL_SPREAD_S, foot }));
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

    /// A second of the APRS task. `send`: what may go (APRS mode: all, APRS-15; beacons alone: beacons, APRS-29;
    /// else nothing, APRS-22); `relay_ok`: slot C can transmit with the band relay
    /// where the active slot holds it (APRS-20); `me` our callsign (APRS-8); `fix` the GPS's (beacons, APRS-24).
    /// A frame to send is written to `out`; the caller keys slot C, plays it, unkeys.
    pub fn tick(&self, now_s: u32, send: Sending, relay_ok: bool, me: &[u8], fix: Option<Fix>, cfg: &AprsCfg, out: &mut [u8; FRAME_MAX]) -> Tick {
        self.now_s.set(now_s);
        let mut t = Tick::default();
        // APRS-33: queries are answered only in APRS mode with automatic beacons on, and not long after they came.
        if send != Sending::All || !cfg.beacons_on() {
            self.query.set(None);
            self.general.set(None);
        }
        if self.query.get().is_some_and(|q| now_s.wrapping_sub(q.at_s) > QUERY_TTL_S) {
            self.query.set(None);
        }
        if self.general.get().is_some_and(|g| now_s > g.due_s + QUERY_TTL_S) {
            self.general.set(None);
        }
        if send == Sending::Beacons {
            // APRS-29: beacons alone answer nothing: acks owed and a digipeat waiting are dropped.
            self.acks.iter().for_each(|a| a.set(None));
            self.digi.set(None);
        }
        if send == Sending::Nothing {
            // Without APRS mode nothing is sent (APRS-22 only decodes): acks owed meanwhile are dropped, so
            // turning the mode on later doesn't send stale ones.
            self.acks.iter().for_each(|a| a.set(None));
            self.on_since.set(None);
            self.fix_wait.set(None);
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
        // The tocall, or a Mic-E report's latitude (APRS-52).
        let mut dest = TOCALL;
        // Acks first (they're owed), then a digipeat, then ours, then the beacon: one transmission a second.
        // Beacons alone (APRS-29): only the beacon.
        let all = send == Sending::All;
        if let Some(c) = self.acks.iter().find(|c| c.get().is_some()) {
            let a = c.get().unwrap();
            c.set(None);
            if let Some(n) = uaprs::build::ack(a.peer.as_bytes(), a.id.as_bytes(), &mut info) {
                return self.framed(my, &dest, &info[..n], "Ack", cfg, out, t);
            }
        }
        if let Some((buf, len)) = self.digi.take() {
            if now_s < self.digi_at.get() {
                self.digi.set(Some((buf, len))); // 0 5 7 3's wait
            } else if self.last_digi.get().is_none_or(|l| now_s.wrapping_sub(l) >= DIGI_GAP_S) {
                self.last_digi.set(Some(now_s));
                out[..len as usize].copy_from_slice(&buf[..len as usize]);
                return Tick { len: len as usize, what: "Digipeat", ..t };
            }
            // Over the rate: dropped, not delayed (the next station will carry it).
        }
        for c in self.out.iter().filter(|_| all) {
            let Some(mut o) = c.get().filter(|o| o.tries < TRIES && now_s >= o.next_s) else { continue };
            if o.tries > 0 && !auto_ok {
                continue;
            }
            let id = num(o.id);
            let Some(n) = uaprs::build::message(o.peer.as_bytes(), o.text.as_bytes(), Some(id.as_bytes()), &mut info) else {
                // Text APRS can't carry (`|`, `~`, `{`): failed at once rather than five silent tries.
                c.set(None);
                push_state(&mut t, (o.peer, o.text, MsgState::Failed));
                t.changed = true;
                continue;
            };
            o.tries += 1;
            o.next_s = now_s + retry_wait(o.tries) as u32;
            c.set(Some(o));
            if o.tries > 1 {
                self.last_auto.set(Some(now_s));
                push_state(&mut t, (o.peer, o.text, MsgState::Retrying(o.tries)));
            }
            t.changed = true;
            return self.framed(my, &dest, &info[..n], "Message", cfg, out, t);
        }
        // APRS-33: a query's answer, an automatic transmission (APRS-9's spacing).
        if all && auto_ok {
            if let Some(q) = self.query.take() {
                let n = match q.answer {
                    Answer::Position => fresh(fix, cfg).and_then(|f| position_info(&f, cfg, &mut info, &mut dest)),
                    Answer::Status => status_info(&mut info),
                    Answer::Text(b) => uaprs::build::message(q.to.as_bytes(), b.as_bytes(), None, &mut info),
                };
                if let Some(n) = n {
                    self.last_auto.set(Some(now_s));
                    return self.framed(my, &dest, &info[..n], "Query reply", cfg, out, t);
                }
            }
            if let Some(g) = self.general.get().filter(|g| now_s >= g.due_s) {
                self.general.set(None);
                let f = fresh(fix, cfg).filter(|f| g.foot.is_none_or(|(c, r)| within(c, (f.pos.lat / 10, f.pos.lon / 10), r)));
                if let Some(n) = f.and_then(|f| position_info(&f, cfg, &mut info, &mut dest)) {
                    self.last_auto.set(Some(now_s));
                    self.last_general.set(Some(now_s));
                    return self.framed(my, &dest, &info[..n], "Query reply", cfg, out, t);
                }
            }
        }
        let beacon = beacon || self.after_voice.get() && auto_ok;
        if beacon && (auto_ok || self.beacon_now.get()) {
            // GPS-33: never a fix older than the beacon interval as current (the fixed position stands in as
            // itself, GPS-3). An automatic beacon waits for a fresh fix, then is skipped (the caller logs it); one
            // asked for now is refused at once.
            let Some(f) = fresh(fix, cfg) else {
                if !self.beacon_now.get() {
                    let since = *self.fix_wait.get().get_or_insert(now_s);
                    self.fix_wait.set(Some(since));
                    if now_s.wrapping_sub(since) < FIX_WAIT_S {
                        return Tick { what: "Beacon: waiting for a fix", ..t };
                    }
                    self.beacon_last.set(Some(LastBeacon { at_s: now_s, ..self.beacon_last.get().unwrap_or_default() }));
                }
                self.fix_wait.set(None);
                self.beacon_now.set(false);
                self.after_voice.set(false);
                return Tick { what: if fix.is_some() { "Beacon: old fix" } else { "Beacon: no position" }, ..t };
            };
            self.after_voice.set(false);
            self.fix_wait.set(None);
            self.beacon_now.set(false);
            self.beacon_last.set(Some(LastBeacon::new(now_s, Some(f))));
            self.last_auto.set(Some(now_s));
            if let Some(n) = position_info(&f, cfg, &mut info, &mut dest) {
                t.changed = true;
                return self.framed(my, &dest, &info[..n], "Beacon", cfg, out, t);
            }
        }
        t
    }

    /// Our frame with 0 5 9 1's path (APRS-26).
    fn framed(&self, me: Address, dest: &[u8; 6], info: &[u8], what: &'static str, cfg: &AprsCfg, out: &mut [u8; FRAME_MAX], t: Tick) -> Tick {
        let names = cfg.path();
        let mut path = [me; 2];
        for (a, n) in path.iter_mut().zip(names.iter()) {
            *a = Address::from_text(n).unwrap_or(me);
        }
        let dest = Address::from_text(dest).unwrap_or(me);
        let len = Frame::new(me, dest, &path[..names.len().min(2)], info).and_then(|f| f.write(out)).unwrap_or(0);
        self.echo.set(self.echo.get() || len > 0);
        Tick { len, what, ..t }
    }
}

/// GPS-33: the fix a beacon (or a query's answer, APRS-33) may send: never one older than the beacon interval as
/// current; the fixed position stands in as itself (GPS-3).
fn fresh(fix: Option<Fix>, cfg: &AprsCfg) -> Option<Fix> {
    let max_s = cfg.every_min().map_or(u32::MAX, |m| m as u32 * 60);
    fix.filter(|f| f.age_s == crate::gpsdata::FIXED_AGE || f.age_s as u32 <= max_s)
}

/// Our position report: the beacon's, and a position query's answer (APRS-33), with 0 5 5 3's comment and
/// 0 5 5 6's ambiguity; with 0 5 5 7 on, a Mic-E report (APRS-52), its latitude in `dest` in place of the tocall.
fn position_info(f: &Fix, cfg: &AprsCfg, info: &mut [u8], dest: &mut [u8; 6]) -> Option<usize> {
    let (table, code) = cfg.symbol_chars(); // APRS-47
    let r = uaprs::build::Report {
        lat: f.pos.lat / 10,
        lon: f.pos.lon / 10,
        table,
        code,
        course_speed: (f.speed_kmh > 0).then(|| (f.heading.clamp(1, 360), (f.speed_kmh as u32 * 1000 / 1852) as u16)),
        altitude: f.three_d.then_some(f.alt_m * 10_000 / 3048),
        messaging: true,
        timestamp: None,
        comment: cfg.comment_text(), // APRS-50
        ambiguity: cfg.ambiguity,    // APRS-51: the uncompressed form carries it
    };
    if cfg.mic_e == 0 {
        return uaprs::build::position(&r, info);
    }
    let mut d = [0u8; 6];
    let n = uaprs::build::mic_e(&r, 8 - cfg.mic_e, &mut d, info)?;
    *dest = d;
    Some(n)
}

/// APRS-33: our status report, `>Ferrite v0.1.0` (no status text is set on the radio).
fn status_info(info: &mut [u8]) -> Option<usize> {
    let v = ferrite_core::VERSION.as_bytes();
    let n = 9 + v.len();
    let o = info.get_mut(..n)?;
    o[..9].copy_from_slice(b">Ferrite ");
    o[9..].copy_from_slice(v);
    Some(n)
}

/// APRS-33's trace answer (`?APRST`, `?PING?`): the query's path as heard, `FROM>TO,DIGI1,DIGI2*` (the last
/// digipeater that repeated it marked, as TNC2 text does).
fn trace(f: &Frame, b: &mut Buf<67>) {
    let put = |b: &mut Buf<67>, a: &Address| {
        a.emit(&mut |c| {
            b.push(&[c]);
        })
    };
    put(b, &f.src);
    b.push(b">");
    put(b, &f.dest);
    let last = f.digis().iter().rposition(|d| d.repeated);
    for (i, d) in f.digis().iter().enumerate() {
        b.push(b",");
        put(b, d);
        if Some(i) == last {
            b.push(b"*");
        }
    }
}

/// APRS-35: a general query's footprint, `lat,lon,radius` (decimal degrees, miles; e.g. `34.02,-117.15,0200`):
/// the centre in millionths of a degree and the radius in metres.
fn footprint(t: &[u8]) -> Option<((i32, i32), u32)> {
    let mut it = t.split(|&c| c == b',');
    let (lat, lon, r) = (degrees(trim(it.next()?))?, degrees(trim(it.next()?))?, trim(it.next()?));
    if it.next().is_some() || r.is_empty() || r.len() > 5 || !r.iter().all(u8::is_ascii_digit) || lat.abs() > 90_000_000 || lon.abs() > 180_000_000 {
        return None;
    }
    let miles = r.iter().fold(0u32, |v, &c| v * 10 + (c - b'0') as u32);
    Some(((lat, lon), miles * 1609))
}

/// Whether `p` is within `r_m` metres of `c` (millionths of a degree): flat earth around `c`, its latitude's
/// cosine by Bhaskara's approximation (within 0.2 %), ample for a footprint of miles.
fn within(c: (i32, i32), p: (i32, i32), r_m: u32) -> bool {
    let x = (c.0 / 1_000_000) as i64; // whole degrees
    let cos_q16 = (32_400 - 4 * x * x) * 65_536 / (32_400 + x * x);
    let mut dlon = (p.1 as i64 - c.1 as i64) % 360_000_000;
    if dlon > 180_000_000 {
        dlon -= 360_000_000;
    } else if dlon < -180_000_000 {
        dlon += 360_000_000;
    }
    // 0.111195 m per millionth of a degree of latitude.
    let ny = (p.0 as i64 - c.0 as i64) * 111_195 / 1_000_000;
    let nx = dlon * 111_195 / 1_000_000 * cos_q16 / 65_536;
    ny * ny + nx * nx <= (r_m as i64) * (r_m as i64)
}

/// Decimal degrees (`-117.15`) in millionths.
fn degrees(t: &[u8]) -> Option<i32> {
    let (neg, t) = match t.split_first() {
        Some((b'-', r)) => (true, r),
        _ => (false, t),
    };
    let (whole, frac) = match t.iter().position(|&c| c == b'.') {
        Some(i) => (&t[..i], &t[i + 1..]),
        None => (t, &b""[..]),
    };
    if whole.is_empty() || whole.len() > 3 || frac.len() > 6 || !whole.iter().chain(frac).all(u8::is_ascii_digit) {
        return None;
    }
    let mut v = whole.iter().fold(0i32, |v, &c| v * 10 + (c - b'0') as i32);
    for i in 0..6 {
        v = v * 10 + frac.get(i).map_or(0, |&c| (c - b'0') as i32);
    }
    Some(if neg { -v } else { v })
}

/// Spaces off both ends.
fn trim(mut t: &[u8]) -> &[u8] {
    while let Some((b' ', r)) = t.split_first() {
        t = r;
    }
    while let Some((b' ', r)) = t.split_last() {
        t = r;
    }
    t
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
        let n = HEARD_MAX as u32 + 5;
        for i in 0..n {
            let call = std::format!("VE3{}{}", (b'A' + (i / 26) as u8) as char, (b'A' + (i % 26) as u8) as char);
            let (f, n) = frame(call.as_bytes(), b"APRS", &[b"WIDE1-1*"], b"!4525.51N/07554.20W>Mobile");
            assert!(a.receive(&f[..n], i, b"VE3XYZ", &AprsCfg::DEFAULT).changed);
        }
        assert_eq!(a.stations(), HEARD_MAX);
        let s = a.station(0).unwrap();
        let last = n - 1;
        assert_eq!(s.call.as_bytes(), std::format!("VE3{}{}", (b'A' + (last / 26) as u8) as char, (b'A' + (last % 26) as u8) as char).as_bytes());
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

    /// APRS-44: a frame that came as FX.25 marks its station with the bytes corrected (the station screen's Path
    /// row); the station's next plain frame clears it. APRS-45: our own frames take 0 5 9 5's check bytes, a
    /// digipeat none.
    #[test]
    fn fx25_heard_and_sent() {
        let a = AprsLive::new();
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b"!4525.51N/07554.20W>Mobile");
        a.receive(&f[..n], 1, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.station(0).unwrap().fx25, None);
        a.note_fx25(&f[..n], 3);
        assert_eq!(a.station(0).unwrap().fx25, Some(3));
        a.receive(&f[..n], 40, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.station(0).unwrap().fx25, None, "heard plain since");
        let c = AprsCfg { fx25: 32, ..AprsCfg::DEFAULT };
        assert_eq!((AprsCfg::DEFAULT.fx25, c.fx25_for("Beacon"), c.fx25_for("Message"), c.fx25_for("Digipeat")), (16, 32, 32, 0));
    }

    /// APRS-11: what we send is capped and identifies us as the protocol requires: the source address is our
    /// callsign and SSID, the destination an APRS tocall, the path the chosen one, and the information field
    /// within 256 bytes; a message's text is cut to APRS's 67 characters however long it was typed.
    #[test]
    fn sent_frames_carry_our_id_and_stay_capped() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg::DEFAULT;
        let long = [b'x'; 200];
        assert!(a.send(b"VE3KPW-9", &long));
        let t = a.tick(0, Sending::All, true, b"VE3XYZ-7", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!((g.src.call(), g.src.ssid), (&b"VE3XYZ"[..], 7), "our callsign and SSID");
        assert!(g.dest.call().starts_with(b"AP"), "an APRS tocall");
        assert_eq!(g.digis().len(), 2, "the default path, WIDE1-1,WIDE2-1");
        assert!(g.info.len() <= INFO_MAX);
        // ":VE3KPW-9 :" + text + "{n": the text is APRS's 67 characters at most
        let text = &g.info[11..g.info.iter().rposition(|&c| c == b'{').unwrap()];
        assert!(!text.is_empty() && text.len() <= 67, "message text {} long", text.len());
        let a = AprsLive::new(); // a beacon alone (the message above would still be retrying)
        a.beacon_now();
        let t = a.tick(400, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!(g.src.call(), b"VE3XYZ");
        assert!(g.info.len() <= INFO_MAX);
    }

    /// APRS-36: our own frame heard back isn't a station; the first one after a transmission names the
    /// digipeater that repeated it, later ones say nothing until we send again.
    #[test]
    fn own_frames_heard_back_name_the_digipeater() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        let (f, n) = frame(me, b"APFRT1", &[b"VE3RSB-1*", b"WIDE1*", b"WIDE2-1"], b"!4525.00N/07541.00W[");
        assert!(a.receive(&f[..n], 10, me, &AprsCfg::DEFAULT).echo.is_none(), "nothing sent yet");
        a.beacon_now();
        assert_eq!(a.tick(20, Sending::All, true, me, Some(fix()), &AprsCfg::DEFAULT, &mut out).what, "Beacon");
        let rx = a.receive(&f[..n], 21, me, &AprsCfg::DEFAULT);
        assert_eq!(rx.echo.unwrap().as_bytes(), b"VE3RSB-1", "the last used entry that names a station");
        assert!(a.receive(&f[..n], 22, me, &AprsCfg::DEFAULT).echo.is_none(), "once per transmission");
        assert_eq!(a.stations(), 0, "not a station heard");
        assert_eq!(a.beacon(&AprsCfg::DEFAULT).digi.as_bytes(), b"VE3RSB-1");
    }

    /// APRS-25: a frame's information field is at most 256 bytes: 256 is taken whole, 257 is dropped.
    #[test]
    fn info_field_capped_at_256() {
        let a = AprsLive::new();
        for (len, kept) in [(INFO_MAX, true), (INFO_MAX + 1, false)] {
            let mut info = std::vec![b'x'; len];
            info[0] = b'>';
            let mut b = std::vec![0u8; 512];
            let src: &[u8] = if kept { b"VE3AA" } else { b"VE3BB" };
            let n = Frame::new(Address::from_text(src).unwrap(), Address::from_text(b"APRS").unwrap(), &[], &info).unwrap().write(&mut b).unwrap();
            assert_eq!(a.receive(&b[..n], 1, b"VE3XYZ", &AprsCfg::DEFAULT).changed, kept, "{len} bytes");
        }
    }

    /// APRS-29 (decision 181): beacons alone (APRS mode off) send the beacon and nothing else: a message to us isn't
    /// acked, our own messages wait, a digipeat isn't sent.
    #[test]
    fn beacons_alone_send_only_beacons() {
        let a = AprsLive::new();
        let cfg = AprsCfg { beacon: 4, ..AprsCfg::DEFAULT }; // every 5 min
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :On my way{12");
        a.receive(&f[..n], 100, b"VE3XYZ", &cfg);
        assert!(a.send(b"VE3KPW-9", b"QSL"));
        let mut out = [0u8; FRAME_MAX];
        let t = a.tick(100, Sending::Beacons, true, b"VE3XYZ", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon", "the beacon, not the ack or the message");
        for s in 101..131 {
            assert_eq!(a.tick(s, Sending::Beacons, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0, "{s}: nothing else");
        }
        // APRS mode on later: the ack owed meanwhile was dropped; our message goes.
        assert_eq!(a.tick(131, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Message");
        // Nothing at all without APRS mode or beacons (APRS-22).
        let b = AprsLive::new();
        b.beacon_now();
        assert_eq!(b.tick(0, Sending::Nothing, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0);
    }

    /// APRS-10, APRS-29 (decision 181): a frame goes in the first clear moment (p-persistence on C's squelch) and
    /// never while C is busy; busy for the whole 5 s window, it isn't sent.
    #[test]
    fn frame_waits_for_a_clear_moment() {
        let mut w = ClearWait::default();
        assert_eq!(w.poll(0, false, 0), Clear::Wait, "nothing waiting");
        w.start(1_000);
        assert!(w.waiting());
        for t in (1_000..3_000).step_by(10) {
            assert_eq!(w.poll(t, true, 0), Clear::Wait, "busy at {t}");
        }
        // Clear: a try each 100 ms slot at 64/256; with these draws the third slot sends.
        assert_eq!(w.poll(3_000, false, 200), Clear::Wait);
        assert_eq!(w.poll(3_050, false, 0), Clear::Wait, "within the slot");
        assert_eq!(w.poll(3_100, false, 100), Clear::Wait);
        assert_eq!(w.poll(3_200, false, 10), Clear::Send);
        assert!(!w.waiting());
        w.start(10_000);
        for t in (10_000..15_000).step_by(10) {
            assert_eq!(w.poll(t, true, 0), Clear::Wait, "busy at {t}");
        }
        assert_eq!(w.poll(15_000, true, 0), Clear::GiveUp, "busy for 5 s: not sent");
    }

    #[test]
    fn message_to_us_is_acked_once() {
        let a = AprsLive::new();
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :On my way{12");
        let rx = a.receive(&f[..n], 100, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(rx.inbound.unwrap().1.as_bytes(), b"On my way");
        let mut out = [0u8; FRAME_MAX];
        let t = a.tick(100, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.what, "Ack");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!(g.info, b":VE3KPW-9 :ack12");
        assert_eq!(g.digis().len(), 2);
        // The same message again (its sender didn't hear our ack): acked again, no second inbox entry.
        let rx = a.receive(&f[..n], 130, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert!(rx.inbound.is_none());
        assert_eq!(a.tick(131, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).what, "Ack");
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
        let t = a.tick(0, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        assert_eq!(Frame::parse(&out[..t.len]).unwrap().info, b":VE3KPW-9 :QSL{1");
        assert_eq!(a.tick(10, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "next try 30 s on");
        let t = a.tick(30, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Message");
        assert!(matches!(t.states[0], Some((_, _, MsgState::Retrying(2)))));
        let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :ack1");
        let rx = a.receive(&f[..n], 40, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert!(matches!(rx.state, Some((_, _, MsgState::Acked))));
        assert_eq!(a.tick(200, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "acked: no more tries");
        // Never acked: five tries, then failed.
        a.send(b"VA3ABC", b"Anyone?");
        let mut tries = 0;
        let mut failed = false;
        for s in 300..1_200 {
            let t = a.tick(s, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
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
        let t = a.tick(0, Sending::All, false, b"VE3XYZ", Some(fix()), &cfg, &mut out);
        assert_eq!(t.len, 0);
        assert!(a.beacon(&cfg).waiting, "APRS-20: waits for the relay");
        let t = a.tick(1, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert_eq!(g.dest.call(), b"APZFER");
        match parse::parse(b"APZFER", g.info) {
            Ok(Packet::Position { pos, .. }) => assert!((pos.lat - 45_425_200).abs() < 200 && (pos.lon + 75_690_100).abs() < 200),
            other => panic!("{other:?}"),
        }
        a.beacon_now();
        assert_eq!(a.tick(2, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).what, "Beacon: no position");
        a.beacon_now();
        assert_eq!(a.tick(3, Sending::All, true, b"", Some(fix()), &cfg, &mut out).len, 0, "APRS-8: no callsign, nothing sent");
    }

    /// GPS-33: a timed beacon with a fix older than its interval waits up to 2 minutes for a fresh one and goes on
    /// it; with none it is skipped (once, for the caller to log) and the next comes by the schedule. Beacon now
    /// with an old fix is refused at once.
    #[test]
    fn beacon_waits_for_a_fresh_fix() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg { beacon: 2, ..AprsCfg::DEFAULT }; // every minute
        let old = Some(Fix { age_s: 90, ..fix() });
        assert_eq!(a.tick(100, Sending::All, true, b"VE3XYZ", old, &cfg, &mut out).what, "Beacon: waiting for a fix");
        assert_eq!(a.tick(150, Sending::All, true, b"VE3XYZ", old, &cfg, &mut out).what, "Beacon: waiting for a fix");
        assert_eq!(a.tick(151, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Beacon", "a fresh fix: sent");
        // The next: the module never finds the sky again.
        let mut skipped = 0;
        let mut sent = 0;
        for s in 211..340 {
            let w = a.tick(s, Sending::All, true, b"VE3XYZ", old, &cfg, &mut out).what;
            skipped += (w == "Beacon: old fix") as u32;
            sent += (w == "Beacon") as u32;
        }
        assert_eq!((skipped, sent), (1, 0), "skipped once after 2 minutes, then the schedule's next");
        assert_eq!(a.tick(400, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).what, "Beacon: waiting for a fix", "no position: waits too");
        // The fixed position stands in as itself.
        let fixed = Some(Fix { age_s: crate::gpsdata::FIXED_AGE, ..fix() });
        assert_eq!(a.tick(401, Sending::All, true, b"VE3XYZ", fixed, &cfg, &mut out).what, "Beacon");
        a.beacon_now();
        assert_eq!(a.tick(440, Sending::All, true, b"VE3XYZ", old, &cfg, &mut out).what, "Beacon: old fix", "beacon now: at once");
    }

    const DIGI: AprsCfg = AprsCfg { digi: crate::aprs::DIGI_ON, ..AprsCfg::DEFAULT };

    #[test]
    fn digipeater_fills_in_once() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b">Walking");
        a.receive(&f[..n], 10, b"VE3XYZ", &DIGI);
        let t = a.tick(10, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.what, "Digipeat");
        let g = Frame::parse(&out[..t.len]).unwrap();
        assert!(g.digis()[0].repeated && g.digis()[0].call() == b"VE3XYZ");
        assert!(!g.digis()[1].repeated);
        // The same frame again within 30 s, or one already past WIDE1: not repeated.
        a.receive(&f[..n], 20, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(20, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0);
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1*", b"WIDE2-1"], b">Later");
        a.receive(&f[..n], 50, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(50, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0);
        // Off by default (APRS-12).
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b">Again");
        a.receive(&f[..n], 90, b"VE3XYZ", &AprsCfg::DEFAULT);
        assert_eq!(a.tick(90, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out).len, 0);
    }

    /// APRS-26: our callsign as an alias when asked, and 0 5 7 3's wait before relaying.
    #[test]
    fn digipeater_mycall_alias_and_wait() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"VE3XYZ", b"WIDE2-1"], b">To me");
        a.receive(&f[..n], 10, b"VE3XYZ", &DIGI);
        assert_eq!(a.tick(10, Sending::All, true, b"VE3XYZ", None, &DIGI, &mut out).len, 0, "my call isn't an alias by default");
        let cfg = AprsCfg { digi: crate::aprs::DIGI_ON | crate::aprs::DIGI_MYCALL | 3 << 4, ..AprsCfg::DEFAULT };
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[b"VE3XYZ", b"WIDE2-1"], b">To me again");
        a.receive(&f[..n], 40, b"VE3XYZ", &cfg);
        assert_eq!(a.tick(41, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "waits 3 s");
        let t = a.tick(43, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out);
        assert_eq!(t.what, "Digipeat");
        assert!(Frame::parse(&out[..t.len]).unwrap().digis()[0].repeated);
    }

    /// 0 5 9 1: the path our frames ask for.
    #[test]
    fn path_setting() {
        let mut out = [0u8; FRAME_MAX];
        for (k, want) in [(0u8, 0usize), (1, 1), (2, 2)] {
            let a = AprsLive::new();
            let cfg = AprsCfg { path: k, ..AprsCfg::DEFAULT };
            a.beacon_now();
            let t = a.tick(1, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out);
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
        assert_eq!(a.tick(100, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Beacon");
        a.after_voice();
        assert_eq!(a.tick(110, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0, "30 s spacing");
        assert_eq!(a.tick(131, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).what, "Beacon");
        assert_eq!(a.tick(170, Sending::All, true, b"VE3XYZ", Some(fix()), &cfg, &mut out).len, 0, "once per transmission");
    }

    /// The information field of the frame `tick` wrote.
    fn sent(out: &[u8; FRAME_MAX], t: &Tick) -> std::vec::Vec<u8> {
        Frame::parse(&out[..t.len]).unwrap().info.to_vec()
    }

    /// APRS-32: a message to us heard again (the sender's retries, a second digipeater's copy) is acked every
    /// time but goes to the inbox once and rings once; the same number with other text, another sender's same
    /// number, and a message without a number after 30 s are new ones.
    #[test]
    fn repeats_acked_every_time_stored_once() {
        let a = AprsLive::new();
        let cfg = AprsCfg::DEFAULT;
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        // A retry 30 s later, then the same frame through two digipeaters.
        let copies: [(&[&[u8]], u32); 4] = [(&[b"WIDE1-1", b"WIDE2-1"], 100), (&[b"WIDE1-1", b"WIDE2-1"], 130), (&[b"VE3RPT*", b"WIDE2-1"], 131), (&[b"VE3RPT", b"VE3DIG*"], 133)];
        let mut stored = 0;
        for (path, s) in copies {
            let (f, n) = frame(b"VE3KPW-9", b"APDR16", path, b":VE3XYZ-7 :Running late{AB12}");
            let rx = a.receive(&f[..n], s, me, &cfg);
            if let Some((from, text)) = rx.inbound {
                assert_eq!((from.as_bytes(), text.as_bytes(), rx.inbound_id.as_bytes()), (&b"VE3KPW-9"[..], &b"Running late"[..], &b"AB12"[..]));
                stored += 1;
            }
            let t = a.tick(s, Sending::All, true, me, None, &cfg, &mut out);
            assert_eq!((t.what, sent(&out, &t)), ("Ack", b":VE3KPW-9 :ackAB12".to_vec()), "{s}: acked again");
        }
        assert_eq!(stored, 1, "one inbox entry, one ring");
        // A reply-ack retry keys on its own number (MM of {MM}AA).
        for s in [200, 230] {
            let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[], b":VE3XYZ-7 :Here now{7}3");
            assert_eq!(a.receive(&f[..n], s, me, &cfg).inbound.is_some(), s == 200);
        }
        // The same number with other text (a sender that started counting again), another sender's same number.
        let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[], b":VE3XYZ-7 :Something else{AB12}");
        assert!(a.receive(&f[..n], 300, me, &cfg).inbound.is_some());
        let (f, n) = frame(b"VA3RTX", b"APK004", &[], b":VE3XYZ-7 :Running late{AB12}");
        assert!(a.receive(&f[..n], 301, me, &cfg).inbound.is_some());
        // Without a number: the same text within 30 s is the same message, later a new one.
        let (f, n) = frame(b"VA3RTX", b"APK004", &[b"WIDE1-1"], b":VE3XYZ-7 :QRV?");
        let (g, m) = frame(b"VA3RTX", b"APK004", &[b"VE3RPT*"], b":VE3XYZ-7 :QRV?");
        assert!(a.receive(&f[..n], 400, me, &cfg).inbound.is_some_and(|i| i.1.as_bytes() == b"QRV?"));
        assert!(a.receive(&g[..m], 410, me, &cfg).inbound.is_none(), "a digipeater's copy");
        assert!(a.receive(&f[..n], 431, me, &cfg).inbound.is_some(), "30 s on: new");
        assert!(a.receive(&f[..n], 431, me, &cfg).inbound_id.is_empty());
    }

    const QUERIES: AprsCfg = AprsCfg { beacon: 9, ..AprsCfg::DEFAULT }; // timed beacons, hourly

    /// A directed query from VE3KPW-9 at `s` (by `path`), and the frame that answers it in the next minute.
    fn ask(a: &AprsLive, text: &[u8], path: &[&[u8]], s: u32) -> Option<(&'static str, std::vec::Vec<u8>)> {
        let mut info = b":VE3XYZ-7 :".to_vec();
        info.extend_from_slice(text);
        let (f, n) = frame(b"VE3KPW-9", b"APDR16", path, &info);
        let rx = a.receive(&f[..n], s, b"VE3XYZ-7", &QUERIES);
        assert!(rx.inbound.is_none(), "a query isn't kept as a message");
        let mut out = [0u8; FRAME_MAX];
        (s..s + 60).find_map(|s| {
            let t = a.tick(s, Sending::All, true, b"VE3XYZ-7", Some(fix()), &QUERIES, &mut out);
            (t.len > 0).then(|| (t.what, sent(&out, &t)))
        })
    }

    /// APRS-50, APRS-51: the comment ends every position report, the beacon's and a query's answer, and ambiguity
    /// blanks the same digits of both coordinates; exact with none, as before.
    #[test]
    fn position_reports_carry_comment_and_ambiguity() {
        let mut cfg = AprsCfg { beacon: 1, ambiguity: 2, ..AprsCfg::DEFAULT };
        cfg.comment[..13].copy_from_slice(b"Ferrite, 73!!");
        let mut info = [0u8; INFO_MAX];
        let n = position_info(&fix(), &cfg, &mut info, &mut [0; 6]).unwrap();
        assert_eq!(&info[..n], b"=4525.  N/07541.  W[Ferrite, 73!!");
        let n = position_info(&fix(), &AprsCfg::DEFAULT, &mut info, &mut [0; 6]).unwrap();
        assert_eq!(&info[..n], b"=4525.51N/07541.41W[");
        // the beacon itself, and the longest comment with course, speed and altitude: within the frame
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        a.beacon_now();
        let t = a.tick(0, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        assert!(Frame::parse(&out[..t.len]).unwrap().info.ends_with(b"[Ferrite, 73!!"));
        cfg.comment = [b'x'; crate::aprs::COMMENT_LEN];
        let moving = Fix { three_d: true, alt_m: 120, speed_kmh: 40, heading: 90, ..fix() };
        let n = position_info(&moving, &cfg, &mut info, &mut [0; 6]).unwrap();
        assert!(info[..n].ends_with(&[b'x'; 40]) && n < INFO_MAX);
    }

    /// APRS-52, APRS-52.1, APRS-52.2: with 0 5 5 7 on, the beacon and a position query's answer go out as Mic-E with that status: the
    /// latitude in the destination in place of the tocall, our callsign, SSID and path as before, the comment and
    /// ambiguity kept; a message and a status answer keep the tocall.
    #[test]
    fn mic_e_beacons_and_answers() {
        let mut cfg = AprsCfg { beacon: 9, mic_e: 2, ambiguity: 1, ..AprsCfg::DEFAULT }; // En route
        cfg.comment[..7].copy_from_slice(b"Ferrite");
        let mut out = [0u8; FRAME_MAX];
        let check = |out: &[u8], len: usize| {
            let g = Frame::parse(&out[..len]).unwrap();
            assert_eq!((g.src.call(), g.src.ssid, g.dest.ssid, g.digis().len()), (&b"VE3XYZ"[..], 7, 0, 2));
            let Ok(uaprs::parse::Packet::Position { pos, mic_e: Some(m), comment, .. }) = uaprs::parse::parse(g.dest.call(), g.info) else { panic!("{:?}", g.info) };
            assert_eq!((m.bits, m.custom, pos.ambiguity), (0b110, false, 1));
            assert!((pos.lat - 45_425_200).abs() < 1000 && (pos.lon + 75_690_100).abs() < 1000, "{pos:?}");
            assert!(comment.bytes().eq(b"Ferrite".iter().copied()));
            std::vec::Vec::from(g.dest.call())
        };
        let a = AprsLive::new();
        a.beacon_now();
        let t = a.tick(0, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!(t.what, "Beacon");
        assert_eq!(check(&out, t.len), b"TU2U5Z"); // 45 25.5_ N, En route 110, +0, W
        // a position query's answer: Mic-E too; a status query's: the tocall
        for (q, mic_e) in [(&b"?APRSP"[..], true), (b"?APRSS", false)] {
            let mut info = b":VE3XYZ-7 :".to_vec();
            info.extend_from_slice(q);
            let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[b"WIDE1-1"], &info);
            a.receive(&f[..n], 1000, b"VE3XYZ-7", &cfg);
            let t = (1000..1100).map(|s| a.tick(s, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out)).find(|t| t.len > 0).unwrap();
            assert_eq!(t.what, "Query reply");
            if mic_e {
                check(&out, t.len);
            } else {
                assert_eq!(Frame::parse(&out[..t.len]).unwrap().dest.call(), TOCALL);
            }
        }
        assert!(a.send(b"VE3KPW-9", b"hi"));
        let t = a.tick(2000, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!((t.what, Frame::parse(&out[..t.len]).unwrap().dest.call()), ("Message", &TOCALL[..]));
    }

    /// APRS-52.3: a Mic-E comment that would read as a device type or an altitude goes out behind a space or two
    /// and is read back as typed (`build::mic_e`'s own tests cover each case).
    #[test]
    fn mic_e_comment_protected() {
        let mut info = [0u8; INFO_MAX];
        for (c, pad) in [(&b"]Ferrite"[..], 1), (b"abc} here", 2), (b"Ferrite", 0)] {
            let mut cfg = AprsCfg { mic_e: 1, ..AprsCfg::DEFAULT };
            cfg.comment[..c.len()].copy_from_slice(c);
            let mut dest = TOCALL;
            let n = position_info(&fix(), &cfg, &mut info, &mut dest).unwrap();
            assert_eq!(&info[9..n], [&b"  "[..pad], c].concat().as_slice());
            let Ok(uaprs::parse::Packet::Position { pos, comment, .. }) = uaprs::parse::parse(&dest, &info[..n]) else { panic!() };
            assert!(comment.bytes().eq(c.iter().copied()) && pos.altitude.is_none());
        }
    }

    /// APRS-33: each directed query's answer, exactly: ?APRSP and ?APRS? our position, ?APRSS a status naming the
    /// firmware, ?APRSD the stations heard direct, ?APRSH a station's count, ?APRST and ?PING? the path as heard,
    /// ?VER the version; the others aren't answered (nor kept as messages). A numbered query is acked too.
    #[test]
    fn directed_queries_answered() {
        let a = AprsLive::new();
        a.beacon_last.set(Some(LastBeacon::new(0, Some(fix())))); // the hourly beacon not due
        let cfg = QUERIES;
        let (f, n) = frame(b"VE3OCB-7", b"APRS", &[], b"!4525.51N/07554.20W>Mobile");
        a.receive(&f[..n], 0, b"VE3XYZ-7", &cfg);
        let (f, n) = frame(b"VA3ABC", b"APRS", &[b"VE3RPT*"], b">Via a digi");
        a.receive(&f[..n], 10, b"VE3XYZ-7", &cfg);
        let reply = |w: &[u8]| Some(("Query reply", w.to_vec()));
        let pos = b"=4525.51N/07541.41W[";
        assert_eq!(ask(&a, b"?APRSP", &[], 100), reply(pos));
        assert_eq!(ask(&a, b"?APRS?", &[], 200), reply(pos));
        let ver = std::format!("Ferrite {}", ferrite_core::VERSION);
        assert_eq!(ask(&a, b"?APRSS", &[], 300), reply(std::format!(">{ver}").as_bytes()));
        assert_eq!(ask(&a, b"?APRSD", &[], 400), reply(b":VE3KPW-9 :Directs= VE3KPW-9 VE3OCB-7"));
        assert_eq!(ask(&a, b"?APRSH VE3OCB-7", &[], 500), reply(b":VE3KPW-9 :VE3OCB-7 heard 1x, last 8 min ago"));
        assert_eq!(ask(&a, b"?APRSH N0CALL", &[], 600), reply(b":VE3KPW-9 :N0CALL not heard"));
        assert_eq!(ask(&a, b"?APRST", &[b"VE3RPT*", b"WIDE2-1"], 700), reply(b":VE3KPW-9 :VE3KPW-9>APDR16,VE3RPT*,WIDE2-1"));
        assert_eq!(ask(&a, b"?PING?", &[b"WIDE1-1*"], 800), reply(b":VE3KPW-9 :VE3KPW-9>APDR16,WIDE1-1*"));
        assert_eq!(ask(&a, b"?VER", &[], 900), reply(std::format!(":VE3KPW-9 :{ver}").as_bytes()));
        assert_eq!(ask(&a, b"?APRSM", &[], 1000), None, "messages waiting: not answered");
        assert_eq!(ask(&a, b"?APRSO", &[], 1100), None);
        // Numbered: acked first, then answered.
        let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[], b":VE3XYZ-7 :?APRSP{5");
        a.receive(&f[..n], 1200, b"VE3XYZ-7", &cfg);
        let mut out = [0u8; FRAME_MAX];
        let t = a.tick(1200, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!(sent(&out, &t), b":VE3KPW-9 :ack5");
        let t = a.tick(1201, Sending::All, true, b"VE3XYZ-7", Some(fix()), &cfg, &mut out);
        assert_eq!((t.what, sent(&out, &t)), ("Query reply", pos.to_vec()));
        // Plain text starting with ? is a message.
        let (f, n) = frame(b"VE3KPW-9", b"APDR16", &[], b":VE3XYZ-7 :?? where are you");
        assert!(a.receive(&f[..n], 1300, b"VE3XYZ-7", &cfg).inbound.is_some());
    }

    /// APRS-33 with APRS-9: answers are automatic transmissions, 30 s apart; one waits at a time (a second query
    /// meanwhile, or a digipeater's copy, isn't answered); no position without a fresh fix; none at all without a
    /// callsign (APRS-8), without APRS mode, or without automatic beacons, and one waiting is dropped then.
    #[test]
    fn query_answers_follow_the_transmit_rules() {
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        let q = |path: &[&[u8]], text: &[u8]| {
            let mut info = b":VE3XYZ-7 :".to_vec();
            info.extend_from_slice(text);
            frame(b"VE3KPW-9", b"APDR16", path, &info)
        };
        let a = AprsLive::new();
        a.beacon_last.set(Some(LastBeacon::new(0, Some(fix())))); // the hourly beacon not due
        let (f, n) = q(&[], b"?VER");
        a.receive(&f[..n], 100, me, &QUERIES);
        assert_eq!(a.tick(100, Sending::All, true, me, None, &QUERIES, &mut out).what, "Query reply");
        let (f, n) = q(&[], b"?APRSS");
        a.receive(&f[..n], 105, me, &QUERIES);
        let (g, m) = q(&[b"VE3RPT*"], b"?APRSS");
        a.receive(&g[..m], 107, me, &QUERIES); // the same query through a digipeater
        let (h, k) = q(&[], b"?APRSD");
        a.receive(&h[..k], 108, me, &QUERIES); // one waits already
        let sends: std::vec::Vec<u32> = (108..200).filter(|&s| a.tick(s, Sending::All, true, me, None, &QUERIES, &mut out).len > 0).collect();
        assert_eq!(sends, [130], "30 s after the last, once");
        // No fresh fix: a position query goes unanswered.
        let (f, n) = q(&[], b"?APRSP");
        a.receive(&f[..n], 300, me, &QUERIES);
        let old = Some(Fix { age_s: 4_000, ..fix() }); // older than the hourly beacon interval (GPS-33)
        assert_eq!(a.tick(300, Sending::All, true, me, old, &QUERIES, &mut out).len, 0);
        // No callsign: nothing heard as ours, nothing sent.
        let b = AprsLive::new();
        let (f, n) = q(&[], b"?VER");
        assert!(b.receive(&f[..n], 100, b"", &QUERIES).inbound.is_none());
        assert_eq!(b.tick(100, Sending::All, true, b"", None, &QUERIES, &mut out).len, 0);
        // Without APRS mode, or with beacons off or manual: not answered, and not later either.
        for (send, cfg) in [(Sending::Beacons, QUERIES), (Sending::Nothing, QUERIES), (Sending::All, AprsCfg::DEFAULT), (Sending::All, AprsCfg { beacon: 1, ..AprsCfg::DEFAULT })] {
            let b = AprsLive::new();
            b.receive(&f[..n], 100, me, &cfg);
            assert_eq!(b.tick(100, send, true, me, Some(fix()), &cfg, &mut out).what, if send == Sending::Beacons { "Beacon" } else { "" });
            for s in 101..200 {
                assert_ne!(b.tick(s, Sending::All, true, me, Some(fix()), &QUERIES, &mut out).what, "Query reply", "{send:?} {s}");
            }
        }
    }

    /// APRS-35: a general query (`?APRS?` heard on air) is answered with our position after a random 5-60 s, at
    /// most once in 5 minutes; with a footprint only when we are inside it.
    #[test]
    fn general_query_rate_limited() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        let me = b"VE3XYZ-7";
        let cfg = QUERIES;
        let mut answers = std::vec::Vec::new();
        let (f, n) = frame(b"VE3OCB", b"APRS", &[b"WIDE1-1", b"WIDE2-1"], b"?APRS?");
        let (g, m) = frame(b"VA3ABC", b"APRS", &[b"WIDE2-2"], b"?APRS? 45.42,-75.69,0010");
        let (h, k) = frame(b"W6XYZ", b"APRS", &[], b"?APRS? 34.02,-117.15,0200");
        a.beacon_last.set(Some(LastBeacon::new(0, Some(fix()))));
        for s in 0..1500u32 {
            match s {
                100 | 200 | 380 => _ = a.receive(&f[..n], s, me, &cfg),
                700 => _ = a.receive(&h[..k], s, me, &cfg), // far away
                800 => _ = a.receive(&g[..m], s, me, &cfg),
                _ => {}
            }
            let t = a.tick(s, Sending::All, true, me, Some(fix()), &cfg, &mut out);
            if t.len > 0 {
                assert_eq!((t.what, sent(&out, &t)), ("Query reply", b"=4525.51N/07541.41W[".to_vec()));
                answers.push(s);
            }
        }
        assert_eq!(answers.len(), 2, "{answers:?}");
        assert!((105..=160).contains(&answers[0]), "{answers:?}: a random wait");
        assert!((805..=860).contains(&answers[1]), "{answers:?}: the 200 and 380 queries within 5 min, the 700 one outside its footprint");
        assert_eq!(footprint(b"34.02,-117.15,0200"), Some(((34_020_000, -117_150_000), 200 * 1609)));
        assert_eq!(footprint(b"34.02,-117.15"), None);
        // About 45.4 km east of Ottawa at 45.42 N (0.58 degrees of longitude): inside 30 miles, outside 25.
        assert!(within((45_420_000, -75_690_000), (45_420_000, -75_110_000), 30 * 1609));
        assert!(!within((45_420_000, -75_690_000), (45_420_000, -75_110_000), 25 * 1609));
    }

    /// APRS-34: a rejection of one of ours ("rejNN", or "rejNN}" from a reply-ack station) ends it at once as
    /// rejected: no more tries. Another station's rejection, or one for a number we don't have waiting, changes
    /// nothing.
    #[test]
    fn rejected_stops_retries() {
        let mut out = [0u8; FRAME_MAX];
        let cfg = AprsCfg::DEFAULT;
        for rej in [&b":VE3XYZ   :rej1"[..], b":VE3XYZ   :rej1}", b":VE3XYZ   :rej1}45"] {
            let a = AprsLive::new();
            assert!(a.send(b"VE3KPW-9", b"QSL"));
            assert_eq!(a.tick(0, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).what, "Message");
            let (f, n) = frame(b"VA3RTX", b"APRS", &[], rej);
            assert!(a.receive(&f[..n], 5, b"VE3XYZ", &cfg).state.is_none(), "not from the recipient");
            let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], b":VE3XYZ   :rej9");
            assert!(a.receive(&f[..n], 6, b"VE3XYZ", &cfg).state.is_none(), "not our number");
            let (f, n) = frame(b"VE3KPW-9", b"APRS", &[], rej);
            let rx = a.receive(&f[..n], 10, b"VE3XYZ", &cfg);
            assert!(matches!(rx.state, Some((p, t, MsgState::Rejected)) if p.as_bytes() == b"VE3KPW-9" && t.as_bytes() == b"QSL"), "{:?}", core::str::from_utf8(rej));
            assert!(rx.inbound.is_none());
            for s in 11..600 {
                assert_eq!(a.tick(s, Sending::All, true, b"VE3XYZ", None, &cfg, &mut out).len, 0, "no more tries");
            }
        }
    }

    /// APRS-13: a message whose text APRS can't carry (`|`, `~`, `{`) fails at once instead of five silent tries.
    #[test]
    fn unsendable_text_fails_at_once() {
        let a = AprsLive::new();
        let mut out = [0u8; FRAME_MAX];
        assert!(a.send(b"VE3KPW-9", b"a{b"));
        let t = a.tick(0, Sending::All, true, b"VE3XYZ", None, &AprsCfg::DEFAULT, &mut out);
        assert_eq!(t.len, 0);
        assert!(matches!(t.states[0], Some((_, _, MsgState::Failed))));
    }

    #[test]
    fn size() {
        let n = core::mem::size_of::<AprsLive>();
        std::println!("AprsLive: {n} B, heard entry {} B, out {} B", core::mem::size_of::<Option<Heard>>(), core::mem::size_of::<Option<Out>>());
        assert!(n <= 8_500, "{n}"); // 50 stations (decision 246)
    }
}
