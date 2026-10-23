//! The link protocol's radio side (manual chapter 29, LINK-1 – LINK-11, Appendix "Link protocol v1"): one
//! session per port, next to the App like the terminal ([`crate::term`]), so it reads and writes through the
//! same paths (channel writes queued for the Storage task, zones and presets through [`crate::data::Op`]).
//! The wire format is `ferrite_core::link`.
//!
//! The caller feeds the port's bytes while [`Session::ready`], and polls: a whole frame is handled when the
//! answer has room on the port and the store has taken the last write, so nothing is dropped and the PC waits
//! for each answer (every frame is acknowledged). The cable is always trusted (LINK-7); on Bluetooth, while the
//! radio's PIN lock is on, nothing but `HELLO` and `AUTH` is answered until `AUTH` gives the PIN (LINK-30).

use crate::chan::{Channel, ChannelStore, MAX_CHANNELS, NO_ZONE};
use crate::data::{Op, RangePreset, ZoneCfg};
use crate::persist::{Persist, Sink as PSink, Source};
use crate::screens::link::LinkAnswer;
use crate::screens::pin::RemotePin;
use crate::settings::Settings;
use crate::slot::{SlotCfg, MODES, SLOT_MODES, STEPS};
use crate::term::Sink;
use crate::App;
use ferrite_core::freq::mhz;
use ferrite_core::link::{self as wire, msg, what, Ptt, Status, MAX_PAYLOAD, MAX_WIRE, PROTOCOL};
use crate::remote::RemoteEnd;
use crate::term::cmd::{self, Done, DumpPos, Line, List};
use ferrite_core::msg::LiveAll;
use crate::term::debug::{Job, Step};
use ferrite_core::msg::{DebugAns, DebugReq, Device};
use ferrite_core::VERSION;
use ferrite_ui::text::Buf;

/// The longest record a `WRITE` assembles: the settings alone (the store's 1 KB record adds the slots; a test checks
/// their real length against it).
pub const RECORD_MAX: usize = 512;
/// A port stays in link mode this long after its last frame (TERM-2), then the first bytes decide again.
pub const ACTIVE_MS: u32 = 10_000;
/// A write session with no frame for this long ends by itself (its progress screen closes).
pub const SESSION_MS: u32 = 5_000;

/// One port's link session.
pub struct Session {
    /// The frame being received (COBS bytes, no delimiter).
    rx: [u8; MAX_WIRE],
    rn: u16,
    /// A frame longer than any valid one: dropped at its delimiter.
    overflow: bool,
    /// A whole frame waits to be handled.
    have: bool,
    /// The cable is trusted (LINK-7); a Bluetooth session gave the radio's PIN (LINK-30).
    trusted: bool,
    pin_ok: bool,
    /// When the last valid frame arrived, and whether one has.
    last: u32,
    seen: bool,
    /// The record a `WRITE` is assembling: what, index, total length, bytes so far.
    asm: [u8; RECORD_MAX],
    asm_key: (u8, u16, u16),
    asm_len: u16,
    /// A write session (`BEGIN` .. `END`, LINK-9): records announced and done, cancelled by BACK.
    session: bool,
    total: u16,
    done: u16,
    cancelled: bool,
    /// The fastest rate this port's UART reaches within 2 % (LINK-11), and a rate agreed, for the caller.
    max_baud: u32,
    baud: Option<u32>,
    /// `REBOOT` was accepted: the caller restarts once the answer has gone (LINK-24).
    reboot: bool,
    hellos: u16,
    /// Who this session is for remote PTT and keys: [`crate::remote::BY_CABLE`] (trusted) or [`crate::remote::BY_BT`].
    by: u8,
    /// The session started over (a new connection, LINK-29), as `by` was: a remote transmission it keyed and keys it
    /// held end at the next poll (LINK-20).
    closed: Option<u8>,
    /// The other port's session is mid-write (LINK-28): this one's writes answer `busy` until it is done.
    other_writing: bool,
    /// A terminal command's output still to send (LINK-34), and the time for its debug requests.
    run: Run,
    now: u32,
    /// A `WRITE_MANY` being applied (LINK-41).
    many: Option<Many>,
    /// Events pushed to this session (LINK-46).
    sub: sub::Sub,
}

/// A `WRITE_MANY` under way (LINK-41): its records wait in `asm` (what's after the `what` byte), applied one a
/// pass, as each may queue the one channel write the App holds; its answer goes when the last is applied or one
/// is refused. Its frame's seq, `what`, the records' length, the next one's offset, and how many are done.
#[derive(Clone, Copy, Debug)]
struct Many {
    seq: u8,
    what: u8,
    len: u8,
    at: u8,
    done: u8,
}

/// What a terminal command run over the link still has to say (LINK-34): made a line at a time, as the terminal's
/// console makes them, so nothing is buffered whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    Off,
    List(List, u16),
    Dump(DumpPos),
    Debug(Job),
}

impl Session {
    pub const fn new(trusted: bool) -> Self {
        Session {
            rx: [0; MAX_WIRE],
            rn: 0,
            overflow: false,
            have: false,
            trusted,
            pin_ok: false,
            last: 0,
            seen: false,
            asm: [0; RECORD_MAX],
            asm_key: (0, 0, 0),
            asm_len: 0,
            session: false,
            total: 0,
            done: 0,
            cancelled: false,
            max_baud: 115_200,
            baud: None,
            reboot: false,
            hellos: 0,
            by: if trusted { crate::remote::BY_CABLE } else { crate::remote::BY_BT },
            closed: None,
            other_writing: false,
            run: Run::Off,
            now: 0,
            many: None,
            sub: sub::Sub::new(),
        }
    }

    /// The fastest cable rate the caller's UART can run (LINK-11): `BAUD` above it is refused (0: every rate, as on
    /// Bluetooth, LINK-15).
    pub const fn set_max_baud(&mut self, b: u32) {
        self.max_baud = b;
    }

    /// The session starts over for a new connection on `trusted`'s port (the cable, or Bluetooth: [`crate::ports`],
    /// LINK-29): a frame half received, a PIN given and a running command go; so does a remote transmission it keyed
    /// (LINK-20). `max_baud` is the port's (Bluetooth: 0, `BAUD` never applies: LINK-15).
    pub fn set_port(&mut self, trusted: bool, max_baud: u32) {
        (self.rn, self.overflow, self.have) = (0, false, false);
        (self.trusted, self.pin_ok) = (trusted, false);
        (self.seen, self.baud, self.max_baud) = (false, None, max_baud);
        self.closed = Some(self.by);
        self.by = if trusted { crate::remote::BY_CABLE } else { crate::remote::BY_BT };
        self.run = Run::Off;
        self.many = None;
        self.unsubscribe(); // LINK-46: events end with the connection
    }

    /// A write is under way: a write session (`BEGIN` .. `END`) or a record half assembled (LINK-28).
    pub fn writing(&self) -> bool {
        self.session || self.asm_len > 0 || self.many.is_some()
    }

    /// LINK-28: the other port's session is mid-write (set by [`crate::ports::Ports`] before each poll).
    pub fn set_other_writing(&mut self, w: bool) {
        self.other_writing = w;
    }

    /// This session may do anything a session may (LINK-7, LINK-30): the cable, Bluetooth with no PIN lock on, or
    /// Bluetooth after `AUTH` gave the PIN.
    pub fn authorised(&self, app: &App) -> bool {
        self.trusted || self.pin_ok || !app.pin_required()
    }

    /// Takes another byte now: false while a whole frame waits for its answer.
    pub fn ready(&self) -> bool {
        !self.have
    }

    /// A byte from the port. A 0x00 ends a frame (an empty one is only a sync), and claims the port for the link
    /// (TERM-2) as a whole frame does.
    pub fn feed(&mut self, b: u8, now: u32) {
        if b == 0 {
            (self.seen, self.last) = (true, now);
            if self.rn > 0 && !self.overflow {
                self.have = true;
            } else {
                self.rn = 0;
            }
            self.overflow = false;
            return;
        }
        if (self.rn as usize) < self.rx.len() {
            self.rx[self.rn as usize] = b;
            self.rn += 1;
        } else {
            (self.overflow, self.rn) = (true, 0);
        }
    }

    /// A frame or a sync arrived lately: the port belongs to the link (TERM-2).
    pub fn active(&self, now: u32) -> bool {
        self.seen && now.wrapping_sub(self.last) < ACTIVE_MS
    }

    /// The write session's progress, for the Device's `link.write_pct` (LINK-9).
    pub fn progress(&self) -> Option<u8> {
        (self.session && self.total > 0).then(|| (self.done.min(self.total) as u32 * 100 / self.total as u32) as u8)
    }

    /// A rate agreed by `BAUD`, once: switch after this answer has gone out, back to 115200 unless a `HELLO`
    /// arrives at the new rate within 1 s ([`Session::hellos`] counts them).
    pub fn take_baud(&mut self) -> Option<u32> {
        self.baud.take()
    }

    /// `REBOOT` was accepted, once: save, quiet the radio and reset after this answer has gone (LINK-24).
    pub fn take_reboot(&mut self) -> bool {
        core::mem::take(&mut self.reboot)
    }

    /// `HELLO`s handled so far.
    pub fn hellos(&self) -> u16 {
        self.hellos
    }

    /// Handles the owner's answers, ends a quiet session, and answers a waiting frame when there is room.
    pub fn poll<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, live: &LiveAll, now: u32, out: &mut impl Sink) {
        self.now = now;
        // BACK on the progress screen (LINK-9) is ours only while a write session runs.
        if self.session && app.take_link_answer() == Some(LinkAnswer::Cancelled) {
            self.cancelled = true;
        }
        if self.session && now.wrapping_sub(self.last) >= SESSION_MS {
            self.end(app);
        }
        // LINK-20: remote PTT ends with the session that keyed it, and every key it held (LINK-19).
        if let Some(by) = self.closed.take() {
            app.remote_session_closed(by, now, store);
        }
        // `KEY` never waits for the store, nor for room for the largest answer: its answers are a few bytes, and on
        // the cable they wait in the send ring while keyed (TX-43), so keepalives must still be handled then.
        let key = self.many.is_none() && self.rn >= 2 && self.rx[0] >= 2 && self.rx[1] == msg::KEY;
        if !self.have || !key && (out.room() < MAX_WIRE || app.write_pending() || store.data().busy()) {
            return;
        }
        if self.many.is_some() {
            self.many_poll(app, store, now, out); // LINK-41: the frame was taken already; `have` holds the port
            return;
        }
        let mut ans = [0u8; MAX_PAYLOAD];
        let n = self.rn as usize;
        let (ty, seq, len) = match wire::parse(&mut self.rx[..n]) {
            Err(e) => {
                ans[0] = Status::BadFrame as u8;
                (e.ty, e.seq, 1)
            }
            Ok(f) => {
                let (ty, seq) = (f.ty, f.seq);
                let mut p = [0u8; MAX_PAYLOAD];
                let pl = f.payload.len();
                p[..pl].copy_from_slice(f.payload);
                (self.last, self.seen) = (now, true);
                let len = self.handle(app, store, dev, live, now, ty, &p[..pl], &mut ans);
                match &mut self.many {
                    Some(m) => {
                        m.seq = seq;
                        match self.many_step(app, store, &mut ans) {
                            Some(len) => (ty, seq, len),
                            None => return, // more records: the next passes (LINK-41)
                        }
                    }
                    None => (ty, seq, len),
                }
            }
        };
        self.send(ty, seq, &ans[..len], out);
    }

    /// The answer out, and the port takes the next frame.
    fn send(&mut self, ty: u8, seq: u8, ans: &[u8], out: &mut impl Sink) {
        let mut w = [0u8; MAX_WIRE];
        if let Some(k) = wire::build(ty | wire::ANSWER, seq, ans, &mut w) {
            // Only a `KEY` answer can find too little room (the send ring full of them while keyed, TX-43): it is
            // dropped whole, never cut, and the request has still been acted on.
            if out.room() >= k {
                out.put(&w[..k]);
            }
        }
        (self.have, self.rn) = (false, 0);
    }

    /// A `WRITE_MANY`'s next record (LINK-41), and its answer once the last is applied or one is refused.
    fn many_poll<S: ChannelStore>(&mut self, app: &mut App, store: &S, now: u32, out: &mut impl Sink) {
        self.last = now;
        let seq = self.many.map_or(0, |m| m.seq);
        let mut ans = [0u8; MAX_PAYLOAD];
        if let Some(len) = self.many_step(app, store, &mut ans) {
            self.send(msg::WRITE_MANY, seq, &ans[..len], out);
        }
    }

    /// One request: its answer's payload into `a` (status first); the length.
    #[allow(clippy::too_many_arguments)]
    fn handle<S: ChannelStore>(&mut self, app: &mut App, st: &S, dev: &Device, live: &LiveAll, now: u32, ty: u8, p: &[u8], a: &mut [u8; MAX_PAYLOAD]) -> usize {
        let mut o = Out { a, n: 1 };
        let s = match ty {
            msg::HELLO => {
                // LINK-20.5: a HELLO is the session starting over: a remote transmission it keyed ends now, as a
                // cut (no keepalive wait, no 400 ms hold), and the keys it holds are let go.
                app.remote_session_closed(self.by, now, st);
                self.unsubscribe(); // LINK-46
                self.hello(app, dev, p, &mut o)
            }
            msg::AUTH => self.auth(app, p),
            // LINK-30: on Bluetooth with the PIN lock on, nothing else until AUTH.
            _ if !self.authorised(app) => Status::NotAuthorised,
            msg::INFO => info(app, st, dev, &mut o),
            msg::READ => read(app, p, &mut o),
            msg::USED => used(st, p, &mut o),
            msg::READ_MANY => read_many(st, p, &mut o),
            msg::LOG_READ => log_read(st, p, &mut o),
            msg::WRITE | msg::DELETE | msg::BEGIN | msg::WRITE_MANY => match self.may_write(app) {
                Status::Ok => match ty {
                    msg::WRITE => self.write(app, p, &mut o),
                    msg::WRITE_MANY => self.write_many(p),
                    msg::DELETE => self.delete(app, st, p),
                    _ => self.begin(app, p),
                },
                s => s,
            },
            msg::END => {
                self.end(app);
                Status::Ok
            }
            msg::BAUD => match p.get(..4) {
                Some(b) => {
                    let r = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    if wire::RATES.contains(&r) && r <= self.max_baud {
                        self.baud = Some(r);
                        Status::Ok
                    } else {
                        Status::Refused
                    }
                }
                None => Status::BadArg,
            },
            msg::REBOOT if app.radio.keyed.is_some() || live.tx.keyed => Status::Busy, // LINK-24.3: a data key-up too
            msg::REBOOT => {
                self.reboot = true;
                Status::Ok
            }
            msg::KEY => key(app, st, self.by, now, p, &mut o),
            msg::SLOT => slot(app, st, p, &mut o),
            msg::CMD => self.command(app, st, dev, live, p, &mut o),
            msg::MORE => {
                self.more(app, st, dev, live, &mut o);
                Status::Ok
            }
            // LINK-19: what the screen shows, as the terminal's `ui` (or `ui keys`) prints it.
            // what=2: the compact one-line status (`ui status`), for a slow link.
            msg::UI => match p.first().copied().unwrap_or(0) {
                w @ (0..=2) => {
                    self.run = Run::List([List::Ui, List::UiKeys, List::UiStatus][w as usize], 0);
                    self.more(app, st, dev, live, &mut o);
                    Status::Ok
                }
                _ => Status::BadArg,
            },
            msg::SUBSCRIBE => self.subscribe(st, now, p, &mut o),
            msg::HEARD => sub::heard(st, p, &mut o),
            msg::MSGS => sub::msgs(st, p, &mut o),
            msg::MSG_SEND => sub::msg_send(app, st, dev, p, &mut o),
            _ => Status::UnknownType,
        };
        o.a[0] = s as u8;
        if s != Status::Ok && s != Status::Invalid {
            o.n = 1; // a refusal carries no partial answer
        }
        o.n
    }

    /// A debug command's next request for the firmware (TERM-21, run over the link: LINK-34), once.
    pub fn debug_req(&mut self) -> Option<DebugReq> {
        match &mut self.run {
            Run::Debug(j) => j.request(self.now),
            _ => None,
        }
    }

    /// The firmware's answer to the last [`Session::debug_req`]; ignored once the command has ended.
    pub fn debug_answer(&mut self, a: DebugAns) {
        if let Run::Debug(j) = &mut self.run {
            j.answer(a);
        }
    }

    /// `CMD` (LINK-34): the line as the terminal's command line runs it (`cmd::run`: the same commands, checks and
    /// refusals), then its first output.
    fn command<S: ChannelStore>(&mut self, app: &mut App, st: &S, dev: &Device, live: &LiveAll, p: &[u8], o: &mut Out) -> Status {
        self.run = Run::Off;
        app.cmd_by = self.by; // XB-20.6: `xb start` over the link logs which port
        let ran = cmd::run(app, st, dev, p, false);
        app.cmd_by = crate::remote::BY_TERM;
        let r = match ran {
            Ok(Done::Nothing) => Run::Off,
            Ok(Done::Said(m)) => {
                o.put(&[0]);
                o.put(m.as_bytes());
                o.put(b"\n");
                return Status::Ok;
            }
            Ok(Done::List(l)) => Run::List(l, 0),
            Ok(Done::Dump) => Run::Dump(DumpPos::default()),
            Ok(Done::Debug(j)) => Run::Debug(j),
            Ok(Done::Load) => {
                o.put(b"load: not over the link; write the records, or load in the terminal");
                return Status::Invalid;
            }
            Err(m) => {
                o.put(m.as_bytes());
                return Status::Invalid;
            }
        };
        self.run = r;
        self.more(app, st, dev, live, o);
        Status::Ok
    }

    /// The running command's next whole lines, as many as fit: the "more" byte, then the text.
    fn more<S: ChannelStore>(&mut self, app: &App, st: &S, dev: &Device, live: &LiveAll, o: &mut Out) {
        let flag = o.n;
        o.put(&[0]);
        let mut l = Line::new();
        loop {
            let was = self.run;
            let got = match &mut self.run {
                Run::Off => false,
                Run::List(list, i) => {
                    let g = cmd::list_line(app, st, dev, live, *list, *i, &mut l);
                    *i += 1;
                    g
                }
                Run::Dump(pos) => cmd::dump_line(app, st, dev, pos, &mut l),
                Run::Debug(j) => match j.line(self.now, &mut l) {
                    Step::Line => true,
                    Step::Wait => {
                        o.a[flag] = 1; // the firmware hasn't answered yet: MORE again
                        return;
                    }
                    Step::Over => false,
                },
            };
            if !got {
                self.run = Run::Off;
                return;
            }
            let room = MAX_PAYLOAD - o.n;
            if l.len() + 1 > room && o.n > flag + 1 {
                self.run = was; // this line goes in the next answer
                o.a[flag] = 1;
                return;
            }
            o.put(&l.as_bytes()[..l.len().min(room - 1)]); // a line longer than any answer is cut
            o.put(b"\n");
        }
    }

    /// `HELLO`: the protocol, the largest payload, and whether this session may go on (0: `AUTH` first).
    fn hello(&mut self, app: &App, dev: &Device, _p: &[u8], o: &mut Out) -> Status {
        self.hellos = self.hellos.wrapping_add(1);
        o.put(&[PROTOCOL, MAX_PAYLOAD as u8, self.authorised(app) as u8]);
        o.text(VERSION.as_bytes());
        o.put(&[ferrite_core::link::build::of(dev.tx_built, app.dry_tx(), dev.bench)]); // LINK-35, TX-48
        Status::Ok
    }

    /// `AUTH`: the radio's PIN over Bluetooth (LINK-31). The cable and a session with no PIN lock need none.
    fn auth(&mut self, app: &mut App, p: &[u8]) -> Status {
        if self.authorised(app) {
            return Status::Ok;
        }
        match app.remote_pin(p) {
            RemotePin::Ok => {
                self.pin_ok = true;
                Status::Ok
            }
            RemotePin::Wrong => Status::Refused,
            RemotePin::Wait(_) => Status::Busy,
        }
    }

    /// Whether this session's writes go ahead (LINK-9: refused after BACK until `END`; LINK-28: `busy` while the
    /// other port's session is mid-write).
    fn may_write(&mut self, _app: &mut App) -> Status {
        if self.cancelled {
            return Status::Cancelled;
        }
        if self.other_writing {
            return Status::Busy;
        }
        Status::Ok
    }

    fn begin(&mut self, app: &mut App, p: &[u8]) -> Status {
        let Some(b) = p.get(..2) else { return Status::BadArg };
        (self.session, self.total, self.done) = (true, u16::from_le_bytes([b[0], b[1]]), 0);
        app.link_write_progress();
        Status::Ok
    }

    fn end(&mut self, app: &mut App) {
        if self.session {
            app.link_write_done();
        }
        (self.session, self.cancelled, self.asm_len) = (false, false, 0);
    }

    fn write(&mut self, app: &mut App, p: &[u8], o: &mut Out) -> Status {
        if p.len() < 7 {
            return Status::BadArg;
        }
        let (w, idx, off, total) = (p[0], le16(&p[1..]), le16(&p[3..]), le16(&p[5..]));
        let data = &p[7..];
        if total as usize > RECORD_MAX || off as usize + data.len() > total as usize {
            return Status::BadArg;
        }
        if off == 0 {
            (self.asm_key, self.asm_len) = ((w, idx, total), 0);
        } else if self.asm_key != (w, idx, total) || off != self.asm_len {
            return Status::BadArg; // parts in order, of one record
        }
        self.asm[off as usize..off as usize + data.len()].copy_from_slice(data);
        self.asm_len = off + data.len() as u16;
        if self.asm_len < total {
            return Status::Ok;
        }
        self.asm_len = 0;
        let rec = &self.asm[..total as usize];
        // LINK-21, LINK-3: the records bigger than a frame; a channel, zone or preset goes by `WRITE_MANY`
        let s = match w {
            what::SETTINGS if idx == 0 => apply_settings(app, rec),
            what::APRS if idx == 0 => sub::aprs_apply(app, rec, o), // LINK-43
            _ => Status::Refused,
        };
        if s == Status::Ok && self.session {
            self.done = self.done.saturating_add(1);
        }
        s
    }

    /// `WRITE_MANY` (LINK-41): what, then records as `READ_MANY` gives them. The whole frame is checked first (a
    /// malformed one changes nothing); then the records wait in `asm` for [`Session::many_step`]. A `WRITE` half
    /// assembled is dropped.
    fn write_many(&mut self, p: &[u8]) -> Status {
        let Some((&w, recs)) = p.split_first() else { return Status::BadArg };
        if !matches!(w, what::CHANNEL | what::ZONE | what::PRESET | what::CONTACT) {
            return Status::BadArg; // the settings are one record, larger than a frame: WRITE
        }
        let mut r = recs;
        while let Some(&l) = r.get(2) {
            let Some(rest) = r.get(3 + l as usize..) else { return Status::BadArg };
            r = rest;
        }
        if !r.is_empty() || recs.is_empty() {
            return Status::BadArg;
        }
        self.asm[..recs.len()].copy_from_slice(recs);
        self.asm_len = 0;
        self.many = Some(Many { seq: 0, what: w, len: recs.len() as u8, at: 0, done: 0 });
        Status::Ok
    }

    /// The next record of a `WRITE_MANY`, applied (LINK-22's checks, LINK-9's count):
    /// None while more follow; else the answer's length: the status, how many were applied, and a refusal's
    /// reason.
    fn many_step<S: ChannelStore>(&mut self, app: &mut App, st: &S, a: &mut [u8; MAX_PAYLOAD]) -> Option<usize> {
        let mut m = self.many?;
        let mut o = Out { a, n: 2 };
        let s = if self.cancelled {
            Status::Cancelled
        } else {
            let at = m.at as usize;
            let (idx, l) = (le16(&self.asm[at..]), self.asm[at + 2] as usize);
            let s = apply(app, st, m.what, idx, &self.asm[at + 3..at + 3 + l], &mut o);
            if s == Status::Ok {
                (m.done, m.at) = (m.done + 1, (at + 3 + l) as u8);
                if self.session {
                    self.done = self.done.saturating_add(1);
                }
            }
            s
        };
        if s == Status::Ok && m.at < m.len {
            self.many = Some(m);
            return None;
        }
        self.many = None;
        (o.a[0], o.a[1]) = (s as u8, m.done);
        Some(if s == Status::Invalid { o.n } else { 2 })
    }

    fn delete<S: ChannelStore>(&mut self, app: &mut App, st: &S, p: &[u8]) -> Status {
        if p.len() < 3 {
            return Status::BadArg;
        }
        let (w, idx) = (p[0], le16(&p[1..]));
        match w {
            what::CHANNEL if idx < MAX_CHANNELS => {
                if crate::term::list::radio_channel(app) == Some(idx) {
                    return Status::Busy; // open in the radio's editor (TERM-17's rule here too)
                }
                if st.get(idx).is_none() {
                    return Status::NotFound;
                }
                app.delete_channel(idx, st);
                Status::Ok
            }
            what::ZONE if idx < st.zones() as u16 => request(st, Op::DeleteZone(idx as u8)),
            what::PRESET if idx < st.data().presets() as u16 => request(st, Op::DeletePreset(idx as u8)),
            what::ZONE | what::PRESET => Status::NotFound,
            _ => Status::BadArg,
        }
    }
}

/// Where a byte from the cable goes (TERM-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Term,
    Link,
    /// KISS mode has the port (chapter 26): neither the terminal nor the link reads it.
    Drop,
}

/// TERM-2: the cable port setting (0 9 2 3: auto, link, terminal, KISS) decides; in auto a link frame (its
/// 0x00 delimiter: the web app sends one before its first frame) takes the port, and keeps it while frames
/// keep coming ([`ACTIVE_MS`]); anything else is the terminal's, which starts on a typed Enter.
pub fn route(cable_port: u8, link_active: bool, b: u8) -> Route {
    match cable_port {
        1 => Route::Link,
        2 => Route::Term,
        3 => Route::Drop,
        _ if link_active || b == 0 => Route::Link,
        _ => Route::Term,
    }
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn request<S: ChannelStore>(st: &S, op: Op) -> Status {
    if st.data().request(op) {
        Status::Ok
    } else {
        Status::Busy
    }
}

/// An answer's payload being written: after the status byte.
struct Out<'a> {
    a: &'a mut [u8; MAX_PAYLOAD],
    n: usize,
}

impl Out<'_> {
    fn put(&mut self, b: &[u8]) {
        let k = b.len().min(MAX_PAYLOAD - self.n);
        self.a[self.n..self.n + k].copy_from_slice(&b[..k]);
        self.n += k;
    }

    /// A length byte, then the text.
    fn text(&mut self, t: &[u8]) {
        self.put(&[t.len().min(255) as u8]);
        self.put(t);
    }
}

/// `KEY`: a keypad or side key, or the knob (LINK-19), or remote PTT (LINK-20).
fn key<S: ChannelStore>(app: &mut App, st: &S, by: u8, now: u32, p: &[u8], o: &mut Out) -> Status {
    use crate::remote::{KeyRefusal, Press};
    use ferrite_core::link::key as k;
    let res = match (p.first().copied(), p.get(1).copied(), p.len()) {
        (Some(c), Some(a), 2) if c <= k::LAST_CODE => match Press::of(a) {
            Some(how) => keypad(app, st, |app| app.remote_press(c, how, by, now, st)),
            None => Err(KeyRefusal::Invalid("")),
        },
        (Some(k::KNOB), Some(n), 2) => keypad(app, st, |app| app.remote_knob(n as i8, now, st)),
        (Some(k::PTT), ..) => return remote_ptt(app, st, by, now, p, o),
        _ => return Status::BadArg,
    };
    match res {
        Ok(()) => Status::Ok,
        Err(KeyRefusal::Busy) => Status::Busy,
        Err(KeyRefusal::Invalid("")) => Status::BadArg,
        Err(KeyRefusal::Invalid(why)) => {
            o.put(why.as_bytes());
            Status::Invalid
        }
    }
}

/// A remote key waits for the store as every other request does (a key may write a channel); `KEY` alone is
/// handled without waiting for it (LINK-20's keepalives), so the wait is a `busy` here.
fn keypad<S: ChannelStore>(app: &mut App, st: &S, f: impl FnOnce(&mut App) -> Result<(), crate::remote::KeyRefusal>) -> Result<(), crate::remote::KeyRefusal> {
    if app.write_pending() || st.data().busy() {
        return Err(crate::remote::KeyRefusal::Busy);
    }
    f(app)
}

/// `KEY` (LINK-20): remote PTT.
///
/// LINK-28: one keyer at a time. `on` and keepalives from the port that didn't key are refused; `off` from either
/// port unkeys (stopping a transmission is always allowed).
fn remote_ptt<S: ChannelStore>(app: &mut App, st: &S, by: u8, now: u32, p: &[u8], o: &mut Out) -> Status {
    let res = match Ptt::decode(p) {
        None => return Status::BadArg,
        Some(Ptt::Off) => {
            app.remote_unkey(RemoteEnd::Off, now, st);
            return Status::Ok;
        }
        Some(Ptt::On(slot)) => app.remote_key(slot, by, now, st).map(|s| o.put(&[s])),
        Some(Ptt::Alive) => app.remote_keepalive(by, now),
    };
    match res {
        Ok(()) => Status::Ok,
        Err(why) => {
            o.put(why.as_bytes());
            Status::Invalid
        }
    }
}

/// `SLOT` (LINK-33): the slot, then optionally a mask and its fields; the slot after, or `invalid` and the reason
/// (nothing changed).
fn slot<S: ChannelStore>(app: &mut App, st: &S, p: &[u8], o: &mut Out) -> Status {
    use ferrite_core::link::slot as m;
    let Some(&s) = p.first().filter(|&&s| (s as usize) < ferrite_core::msg::SLOTS) else { return Status::BadArg };
    let mask = p.get(1).copied().unwrap_or(0);
    let mut c = crate::slotctl::SlotChange { vfo: mask & m::VFO != 0, active: mask & m::ACTIVE != 0, ..Default::default() };
    let mut f = p.get(2..).unwrap_or(&[]);
    let mut take = |n: usize| -> Option<u32> {
        let b = f.get(..n)?;
        f = &f[n..];
        Some(b.iter().rev().fold(0u32, |v, &x| v << 8 | x as u32))
    };
    if mask & m::FREQ != 0 {
        let Some(v) = take(4) else { return Status::BadArg };
        c.freq = Some(v);
    }
    if mask & m::MODE != 0 {
        let Some(v) = take(1) else { return Status::BadArg };
        c.mode = Some(v as u8);
    }
    if mask & m::POWER != 0 {
        let Some(v) = take(1) else { return Status::BadArg };
        c.power = Some(v as u8);
    }
    if mask & m::RX_TONE != 0 {
        let Some(v) = take(2) else { return Status::BadArg };
        c.rx_tone = Some(v as u16);
    }
    if mask & m::TX_TONE != 0 {
        let Some(v) = take(2) else { return Status::BadArg };
        c.tx_tone = Some(v as u16);
    }
    if mask & m::OFFSET != 0 {
        let (Some(v), Some(split)) = (take(4), take(1)) else { return Status::BadArg };
        if split > 1 {
            return Status::BadArg;
        }
        c.offset = Some((v as i32, split == 1));
    }
    if !f.is_empty() {
        return Status::BadArg;
    }
    if !c.is_empty() {
        if let Err(why) = app.slot_apply(s, &c, st) {
            o.put(why.as_bytes());
            return Status::Invalid;
        }
    }
    let g = app.slot_state(s);
    o.put(&[s, g.active as u8]);
    o.put(&g.channel.unwrap_or(0xFFFF).to_le_bytes());
    o.put(&g.freq.to_le_bytes());
    o.put(&[g.mode, g.power]);
    o.put(&g.rx_tone.to_le_bytes());
    o.put(&g.tx_tone.to_le_bytes());
    o.put(&g.offset.to_le_bytes());
    let flags = if g.split { m::SPLIT } else { 0 } | if g.aprs { m::APRS } else { 0 } | if g.scanning { m::SCANNING } else { 0 };
    o.put(&[flags]);
    Status::Ok
}

fn info<S: ChannelStore>(app: &App, st: &S, dev: &Device, o: &mut Out) -> Status {
    o.text(VERSION.as_bytes());
    o.put(&st.count().to_le_bytes());
    o.put(&[st.zones(), st.data().presets()]);
    o.put(&(settings_record(app, &mut Window { b: &mut [], from: 0, n: 0 }) as u16).to_le_bytes());
    o.put(&[app.set.region]);
    o.text(app.set.callsign.as_bytes());
    o.put(&[ferrite_core::link::build::of(dev.tx_built, app.dry_tx(), dev.bench)]); // LINK-35, TX-48
    o.put(&ferrite_core::uid::bytes(dev.selftest.uid)); // LINK-35.1: the unique ID, zeros until read
    Status::Ok
}

// ---- Records (Appendix "Link protocol v1": the same byte layouts as the radio's own storage) ----

/// A record's bytes `from..from + b.len()` into `b` as it is written out, counting all of it: a read takes a
/// window of a record without a copy of the whole.
struct Window<'a> {
    b: &'a mut [u8],
    from: usize,
    n: usize,
}

impl PSink for Window<'_> {
    fn put(&mut self, bytes: &[u8]) {
        for (k, &c) in bytes.iter().enumerate() {
            if let Some(i) = (self.n + k).checked_sub(self.from) {
                if let Some(d) = self.b.get_mut(i) {
                    *d = c;
                }
            }
        }
        self.n += bytes.len();
    }
}

/// Bytes out of a record.
struct Rd<'a> {
    b: &'a [u8],
    n: usize,
}

impl Source for Rd<'_> {
    fn take(&mut self, out: &mut [u8]) -> bool {
        match self.b.get(self.n..self.n + out.len()) {
            Some(s) => {
                out.copy_from_slice(s);
                self.n += out.len();
                true
            }
            None => false,
        }
    }
}

/// The settings record as the link reads it, into `w`: the PIN reads as zeros (LINK-7). Its whole length.
fn settings_record(app: &App, w: &mut Window) -> usize {
    let m = Settings { pin: 0, pin_misses: 0, pin_wait: 0, ..*app.set };
    m.save(w);
    w.n
}

/// Settings the link never changes (LINK-5): the region, Extend TX, the PIN, setup, the cable port (the link
/// would lose its own port) and the spare bytes. Dry TX (TX-48) isn't in the record at all. A written record keeps the radio's. The callsign is written, in
/// capitals as on the radio (LINK-26, decision 77: as nicFW's RMS writes it).
fn keep_protected(new: &mut Settings, old: &Settings) {
    let call = new.callsign;
    new.set_callsign(call.as_bytes());
    new.region = old.region;
    (new.extend_tx, new.extend_warned) = (old.extend_tx, old.extend_warned);
    (new.pin_mode, new.pin, new.pin_len, new.pin_misses, new.pin_wait) = (old.pin_mode, old.pin, old.pin_len, old.pin_misses, old.pin_wait);
    new.setup_done = old.setup_done;
    new.cable_port = old.cable_port;
    new.kiss_port = old.kiss_port; // KISS-7: turning KISS on is the owner's consent, made on the radio
    new.spare_tokens = [0; 30];
    new.spare_temp = 0;
    // PWR-24: Low Power Mode and its Auto level sit in what was spare; a writer that doesn't know them (the
    // Console so far) would zero them, so the radio's stay.
    (new.lpm, new.lpm_below) = (old.lpm, old.lpm_below);
}

fn channel_save(c: &Channel, s: &mut dyn PSink) {
    c.num.save(s);
    c.zone.save(s);
    c.rx.save(s);
    s.put(&c.name);
    c.cfg.save(s);
    c.flags.save(s);
    c.tones.save(s); // decision 86: end tone and tail; a writer from before it leaves it off (Global)
}

fn zone_save(name: &[u8], c: &ZoneCfg, s: &mut dyn PSink) {
    let mut n = [0u8; 16];
    n[..name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
    s.put(&n);
    for v in [c.scan_all, c.shown, c.by_distance] {
        v.save(s);
    }
    c.resume.save(s);
    c.dwell.save(s);
    0u8.save(s); // reserved: it was smart scan (SCAN-23)
    c.pri.save(s);
}

fn preset_save(p: &RangePreset, s: &mut dyn PSink) {
    let mut n = [0u8; 12];
    let b = p.name.as_bytes();
    n[..b.len().min(12)].copy_from_slice(&b[..b.len().min(12)]);
    s.put(&n);
    p.start.save(s);
    p.end.save(s);
    p.step.save(s);
    p.mode.save(s);
    p.narrow.save(s);
    p.resume.save(s); // SCAN-24; a record without it reads as Global
}

/// A name from a fixed field: up to the first 0, printable ASCII only.
fn name_of<const N: usize>(b: &[u8]) -> Option<Buf<N>> {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len()).min(N);
    if !b[..n].iter().all(|c| (0x20..0x7F).contains(c)) {
        return None;
    }
    Some(Buf::from(&b[..n]))
}

/// Channel, zone or preset `idx`, as `READ_MANY` gives it, into `win`: `not found` when there is none.
fn record<S: ChannelStore>(st: &S, w: u8, idx: u16, win: &mut Window) -> Status {
    match w {
        what::CHANNEL => match st.get(idx) {
            Some(c) => channel_save(&c, win),
            None => return Status::NotFound,
        },
        what::ZONE => match st.zone_name(idx as u8).filter(|_| idx < st.zones() as u16) {
            Some(n) => zone_save(n.as_bytes(), &st.data().zone_cfg(idx as u8), win),
            None => return Status::NotFound,
        },
        what::PRESET => match st.data().preset(idx as u8).filter(|_| idx < 256) {
            Some(pr) => preset_save(&pr, win),
            None => return Status::NotFound,
        },
        // APRS-42: callsign (10) and name (12), 0-padded, then the flags (0)
        what::CONTACT => match st.data().contact(idx) {
            Some(c) => {
                let mut b = [0u8; 23];
                b[..c.call.len()].copy_from_slice(c.call.as_bytes());
                b[10..10 + c.name.len()].copy_from_slice(c.name.as_bytes());
                win.put(&b);
            }
            None => return Status::NotFound,
        },
        _ => return Status::BadArg,
    }
    Status::Ok
}

fn read(app: &App, p: &[u8], o: &mut Out) -> Status {
    if p.len() < 6 {
        return Status::BadArg;
    }
    let (w, idx, off, len) = (p[0], le16(&p[1..]), le16(&p[3..]) as usize, (p[5] as usize).min(MAX_PAYLOAD - 3));
    let mut buf = [0u8; MAX_PAYLOAD];
    let mut win = Window { b: &mut buf[..len], from: off, n: 0 };
    match w {
        // LINK-21: the records bigger than a frame; a channel, zone or preset goes by `READ_MANY`
        what::SETTINGS if idx == 0 => _ = settings_record(app, &mut win),
        what::APRS if idx == 0 => sub::aprs_record(app, &mut win), // LINK-43
        _ => return Status::BadArg,
    }
    let total = win.n;
    if off > total {
        return Status::BadArg;
    }
    let got = len.min(total - off);
    o.put(&(total as u16).to_le_bytes());
    o.put(&buf[..got]);
    Status::Ok
}

/// How many indexes `w` has: channels 0-999, the zones and presets there are. None for the settings.
fn indexes<S: ChannelStore>(st: &S, w: u8) -> Option<u16> {
    match w {
        what::CHANNEL => Some(MAX_CHANNELS),
        what::ZONE => Some(st.zones() as u16),
        what::PRESET => Some(st.data().presets() as u16),
        what::CONTACT => Some(st.data().contacts()),
        _ => None,
    }
}

/// Whether index `i` of `w` holds a record; a channel from the store's zone map (RAM on the radio), without
/// reading it.
fn held<S: ChannelStore>(st: &S, w: u8, i: u16) -> bool {
    match w {
        what::CHANNEL => st.zone_of(i).is_some(),
        what::ZONE => st.zone_name(i as u8).is_some(),
        what::CONTACT => i < st.data().contacts(),
        _ => st.data().preset(i as u8).is_some(),
    }
}

/// `USED` (LINK-39): the index count, then the bitmap of the ones holding a record.
fn used<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> Status {
    let Some(w) = p.first().copied() else { return Status::BadArg };
    let Some(n) = indexes(st, w) else { return Status::BadArg };
    o.put(&n.to_le_bytes());
    let mut byte = 0u8;
    for i in 0..n {
        if held(st, w, i) {
            byte |= 1 << (i % 8);
        }
        if i % 8 == 7 || i + 1 == n {
            o.put(&[byte]);
            byte = 0;
        }
    }
    Status::Ok
}

/// `READ_MANY` (LINK-40): from `first`, `count` indexes on, each record held as a whole, as many as fit; the
/// index to go on from first.
fn read_many<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> Status {
    if p.len() < 5 {
        return Status::BadArg;
    }
    let (w, first, count) = (p[0], le16(&p[1..]), le16(&p[3..]));
    let Some(n) = indexes(st, w) else { return Status::BadArg };
    let end = (first as u32 + count as u32).min(n as u32) as u16;
    let next = o.n;
    o.put(&[0, 0]);
    let mut i = first;
    let mut buf = [0u8; MAX_PAYLOAD];
    while i < end {
        if held(st, w, i) {
            let mut win = Window { b: &mut buf, from: 0, n: 0 };
            if record(st, w, i, &mut win) == Status::Ok {
                let k = win.n;
                if o.n + 3 + k > MAX_PAYLOAD {
                    break;
                }
                o.put(&i.to_le_bytes());
                o.put(&[k as u8]);
                o.put(&buf[..k]);
            }
        }
        i += 1;
    }
    o.a[next..next + 2].copy_from_slice(&i.max(first).to_le_bytes());
    Status::Ok
}

/// `LOG_READ` (LINK-42): the event log (INBOX-6) newest first, from below the sequence number `before`.
fn log_read<S: ChannelStore>(st: &S, p: &[u8], o: &mut Out) -> Status {
    use ferrite_core::link::{LOG_FIXED, LOG_POS, LOG_UNREAD};
    let Some(b) = p.get(..4) else { return Status::BadArg };
    let before = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let most = p.get(4).copied().filter(|&m| m > 0).unwrap_or(u8::MAX);
    let d = st.data();
    let kept = d.events();
    let top = d.event_seq(0).map_or(0, |s| s.wrapping_add(1));
    o.put(&top.to_le_bytes());
    o.put(&kept.to_le_bytes());
    let count = o.n;
    o.put(&[0]);
    let (mut i, mut n) = (top.saturating_sub(before).min(kept as u32) as u16, 0u8);
    while i < kept && n < most {
        if let (Some(e), Some(seq)) = (d.event(i), d.event_seq(i)) {
            let (nm, tx) = (e.name.as_bytes(), e.text.as_bytes());
            let k = LOG_FIXED + if e.pos.is_some() { 8 } else { 0 } + 2 + nm.len() + tx.len();
            if o.n + k > MAX_PAYLOAD {
                break;
            }
            o.put(&seq.to_le_bytes());
            o.put(&e.ago_s.to_le_bytes());
            let flags = if e.unread { LOG_UNREAD } else { 0 } | if e.pos.is_some() { LOG_POS } else { 0 };
            o.put(&[e.src as u8, e.slot, flags]);
            o.put(&e.freq.to_le_bytes());
            o.put(&e.id.to_le_bytes());
            o.put(&e.x.to_le_bytes());
            o.put(&e.y.to_le_bytes());
            if let Some(ps) = e.pos {
                o.put(&ps.lat.to_le_bytes());
                o.put(&ps.lon.to_le_bytes());
            }
            o.text(nm);
            o.text(tx);
            n += 1;
        }
        i += 1;
    }
    o.a[count] = n;
    Status::Ok
}

/// The settings record written (`WRITE`): the protected ones keep the radio's (LINK-5).
fn apply_settings(app: &mut App, rec: &[u8]) -> Status {
    let old = *app.set;
    let mut new = old;
    new.load(&mut Rd { b: rec, n: 0 });
    new.sanitize();
    keep_protected(&mut new, &old);
    *app.set = new;
    app.send_radio_settings();
    Status::Ok
}

/// A channel, zone or preset written (`WRITE_MANY`): checked like the radio's own editors (LINK-5), then saved the
/// same way.
#[inline(never)] // one copy, not one in `many_step` (flash)
fn apply<S: ChannelStore>(app: &mut App, st: &S, w: u8, idx: u16, rec: &[u8], o: &mut Out) -> Status {
    let invalid = |o: &mut Out, why: &[u8]| {
        o.put(why);
        Status::Invalid
    };
    match w {
        what::CHANNEL if idx < MAX_CHANNELS => {
            let mut c = Channel { num: idx, zone: NO_ZONE, rx: 0, name: [0; 16], cfg: SlotCfg::default(), flags: 0, tones: 0 };
            let mut r = Rd { b: rec, n: 0 };
            let mut num = 0u16;
            num.load(&mut r);
            c.zone.load(&mut r);
            c.rx.load(&mut r);
            let mut name = [0u8; 16];
            if !r.take(&mut name) {
                return Status::BadArg;
            }
            c.cfg.load(&mut r);
            c.flags.load(&mut r);
            if r.n < rec.len() {
                c.tones.load(&mut r); // decision 86; absent from an older writer: Global
                c.tones = c.end_tone() | c.tail() << 3;
            }
            if r.n != rec.len() {
                return Status::BadArg;
            }
            let Some(nm) = name_of::<16>(&name) else { return invalid(o, "name: printable ASCII only".as_bytes()) };
            c.set_name(nm.as_bytes());
            if c.zone != NO_ZONE && c.zone >= st.zones() {
                return invalid(o, "zone: no such zone".as_bytes());
            }
            if c.cfg.mode as usize >= SLOT_MODES || c.cfg.step as usize >= STEPS.len() {
                return invalid(o, "mode or step out of range".as_bytes());
            }
            if let Err(m) = crate::term::cmd::check(&c) {
                return invalid(o, m.as_bytes());
            }
            if crate::term::list::radio_channel(app) == Some(idx) {
                return Status::Busy; // open in the radio's editor: the radio wins (TERM-17)
            }
            if st.get(idx) != Some(c) {
                app.put_channel(c);
            }
            Status::Ok
        }
        what::ZONE if idx <= st.zones() as u16 && idx < 254 => {
            let mut r = Rd { b: rec, n: 0 };
            let mut name = [0u8; 16];
            if !r.take(&mut name) {
                return Status::BadArg;
            }
            let mut c = ZoneCfg::default();
            for v in [&mut c.scan_all, &mut c.shown, &mut c.by_distance] {
                v.load(&mut r);
            }
            c.resume.load(&mut r);
            c.dwell.load(&mut r);
            0u8.load(&mut r); // reserved
            c.pri.load(&mut r);
            let Some(nm) = name_of::<16>(&name).filter(|n| !n.as_bytes().is_empty()) else { return invalid(o, "name: 1 to 16 printable characters".as_bytes()) };
            if c.resume > 12 {
                return invalid(o, "resume out of range".as_bytes());
            }
            request(st, Op::PutZone(idx as u8, nm, c))
        }
        what::PRESET if idx <= st.data().presets() as u16 && idx < 16 => {
            let mut r = Rd { b: rec, n: 0 };
            let mut name = [0u8; 12];
            if !r.take(&mut name) {
                return Status::BadArg;
            }
            let Some(nm) = name_of::<12>(&name) else { return invalid(o, "name: printable ASCII only".as_bytes()) };
            let mut p = RangePreset { name: nm, start: 0, end: 0, step: 0, mode: 0, narrow: true, skips: 0, resume: 0 };
            p.start.load(&mut r);
            p.end.load(&mut r);
            p.step.load(&mut r);
            p.mode.load(&mut r);
            p.narrow.load(&mut r);
            p.resume.load(&mut r);
            if p.resume > 12 {
                return invalid(o, "resume out of range".as_bytes());
            }
            if !(mhz(18)..=mhz(1300)).contains(&p.start) || p.end <= p.start || p.end > mhz(1300) {
                return invalid(o, "start and end: 18 to 1300 MHz, end above start".as_bytes());
            }
            if p.step as usize >= STEPS.len() || p.mode as usize >= SLOT_MODES.min(MODES.len()) {
                return invalid(o, "step or mode out of range".as_bytes());
            }
            if let Some(old) = st.data().preset(idx as u8) {
                p.skips = old.skips;
                if old == p {
                    return Status::Ok;
                }
            }
            request(st, Op::PutPreset(idx as u8, p))
        }
        what::ZONE | what::PRESET => Status::NotFound,
        // APRS-42: added or renamed by callsign (the index is ignored), deleted with flag bit 0
        what::CONTACT => {
            if rec.len() != 23 {
                return Status::BadArg;
            }
            let call = &rec[..rec[..10].iter().position(|&c| c == 0).unwrap_or(10)];
            if call.len() < 3 || call.len() > 9 || !call.iter().all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'-') {
                return invalid(o, "callsign: capitals, digits and -, 3 to 9".as_bytes());
            }
            let call = Buf::<10>::from(call);
            if rec[22] & 1 != 0 {
                return if st.data().contact_of(call.as_bytes()).is_some() { request(st, Op::DeleteContact(call)) } else { Status::NotFound };
            }
            let mut name = Buf::<12>::new();
            for &c in rec[10..22].iter().take_while(|&&c| c != 0) {
                name.push(&[ferrite_core::name::map_name(c)]); // CHAN-31's set (decision 221)
            }
            let c = crate::data::Contact { call, name };
            if st.data().contact_of(call.as_bytes()) == Some(c) {
                return Status::Ok;
            }
            if st.data().contacts() >= crate::screens::contacts::CONTACT_MAX && st.data().contact_of(call.as_bytes()).is_none() {
                return invalid(o, "128 contacts".as_bytes());
            }
            request(st, Op::SaveContact(c))
        }
        _ => Status::Refused, // nothing else is written over the link (LINK-3)
    }
}

mod sub;
/// LINK-43: the APRS record's length, for Ferrite Console (which reads its layout from this crate).
pub use sub::APRS_REC;

#[cfg(test)]
mod tests;
