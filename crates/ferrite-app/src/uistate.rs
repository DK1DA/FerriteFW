//! What the screen shows, as text (manual chapter 29, LINK-19; decision 150): the link's `UI` and the terminal's
//! `ui`. A PC script presses keys over the link and reads this back to check the radio's UI as a person would.
//!
//! It is built from the App's own state, the same values the drawing code draws (each screen's rows come from the
//! functions its `render` uses), never from pixels. One line at a time, by number ([`line`]): nothing is held, so
//! it costs no RAM, and a page of an answer is the next lines. The lines, in this order:
//!
//! - `screen ID "Name" title="..."`: the screen (ID is stable: its variant), its name and the status bar's title.
//! - `state frame=N ago=MS dark=0 locked=0 active=A keyed=- remote=0 mode=off fm=0`: redraws that changed
//!   something, the time since the last, the backlight off, the key lock, the active slot, the slot transmitting,
//!   remote PTT, the exclusive mode running (MODE-1: off, scope, strip, tone, counter, fm, hf, xb) and whether
//!   broadcast FM plays.
//! - `bar lock=0 scan=0 watch=0 inbox=0 alert=0 gps=fix bt=0 battery=80 low=0 clock=14:32`: the status bar.
//! - `toast "head" "body"` or `popup "head" "body"` while one shows; `qmenu "PTT A" "Reverse" "off"` per row of the
//!   quick menu while it is up.
//! - The screen's own lines: home's `card` per slot (a broadcast card followed by its `bcast` report: the RDS
//!   text, `stereo`, `dbuv`, `snr`); a list's `row N "name" "value"` (with `sel`, `dot` changed from its default, `grey`), its `help`; an
//!   editor's `value`; a question's `ask`; the squelch screen's `squelch`; tone search's `tsearch` (the tone found,
//!   the slot's signal, the hits) and the counter's `counter` (frequency and level); a card's `hint` while it shows
//!   a tone hint; the PIN entry's `pin` (digits as
//!   stars); FM's `fm`, HF mode's `hf`, the band scope's `scope`, crossband's `xb`
//!   with its `check`s, setup's `setup`, compose's `compose`, the GPS position page's `gps`, `pos` and `near`
//!   (their option pages, grids and tiles as rows); and so on. A screen without its own lines yet shows only
//!   the rest.
//! - `hints "KEY" "label" ...`: the hint bar's pairs (under a toast, what it will show again).
//!
//! [`status_line`] is the compact one-line status (`UI` what=2, `ui status`).
//!
//! Strings are in double quotes, `"` and `\` escaped with `\`; the arrows are `^ v < >`.

use crate::chan::ChannelStore;
use crate::screens::{self, shared::Cx};
use crate::slot::{card_tone, mode_chip, shift_chip, QUICK};
use crate::term::cmd::Line;
use crate::{App, Screen};
use ferrite_core::freq::Freq10;
use ferrite_core::keys::{self as k, Key};
use ferrite_core::msg::{Device, GpsState, LiveAll, SLOTS};
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// Line `i` of the description into `out`; false past the last.
pub fn line<S: ChannelStore>(app: &App, st: &S, dev: &Device, live: &LiveAll, i: u16, out: &mut Line) -> bool {
    out.clear();
    let mut d = Desc { want: i, n: 0, out, got: false };
    describe(app, &Cx { live, dev, store: st }, &mut d);
    d.got
}

/// One line of the description is wanted: the others are counted, not written.
struct Desc<'a> {
    want: u16,
    n: u16,
    out: &'a mut Line,
    got: bool,
}

impl Desc<'_> {
    /// The next line: Some(its buffer) if it is the one wanted.
    fn ln(&mut self) -> Option<&mut Line> {
        let me = self.n == self.want;
        self.n += 1;
        if me {
            self.got = true;
            Some(self.out)
        } else {
            None
        }
    }
}

/// A string in double quotes: `"` and `\` escaped, the font's arrows as `^ v < >`, other control bytes as `?`.
fn q(l: &mut Line, s: &[u8]) {
    l.push(b"\"");
    for &c in s {
        match c {
            b'"' | b'\\' => l.push(&[b'\\', c]),
            0x18 => l.push(b"^"),
            0x19 => l.push(b"v"),
            0x1A => l.push(b">"),
            0x1B => l.push(b"<"),
            c if c < 0x20 => l.push(b"?"),
            c => l.push(&[c]),
        }
    }
    l.push(b"\"");
}

/// ` key=` then a quoted string.
fn kq(l: &mut Line, key: &str, s: &[u8]) {
    l.push(b" ");
    l.push(key.as_bytes());
    l.push(b"=");
    q(l, s);
}

/// ` key=n`.
fn kn(l: &mut Line, key: &str, n: u32) {
    let _ = uwrite!(l, " {}={}", key, n);
}

/// MHz with five decimals (10 Hz steps): "146.52000".
fn mhz(l: &mut Line, f: Freq10) {
    let frac = f % 100_000;
    let _ = uwrite!(l, "{}.", f / 100_000);
    let mut d = 10_000;
    while d > 0 {
        l.push(&[b'0' + (frac / d % 10) as u8]);
        d /= 10;
    }
}

fn letter(s: u8) -> u8 {
    b'A' + s
}

/// ` keyed=A remote=0 mode=off`: the slot transmitting (`-` none), remote PTT and the exclusive mode (MODE-1).
/// A data key-up (APRS, KISS: the UI's own `keyed` is voice only) counts too, from the Radio task's live state
/// (LINK-19), so a test sees and times every transmission.
fn keyed_remote_mode(app: &App, live: &LiveAll, l: &mut Line) {
    let keyed = app.radio.keyed.or(live.tx.keyed.then_some(live.tx.slot));
    match keyed {
        Some(s) if (s as usize) < SLOTS => {
            let _ = uwrite!(l, " keyed={}", letter(s) as char);
        }
        Some(_) => l.push(b" keyed=HF"),
        None => l.push(b" keyed=-"),
    }
    kn(l, "remote", app.remote_keyed() as u32);
    use screens::activity::Mode;
    l.push(b" mode=");
    l.push(match app.radio.mode {
        Mode::Off => b"off".as_slice(),
        Mode::Scope => b"scope",
        Mode::Strip => b"strip",
        Mode::Tone => b"tone",
        Mode::Counter => b"counter",
        Mode::Fm => b"fm",
        Mode::Hf => b"hf",
        Mode::Xb => b"xb",
    });
}

/// The compact status (LINK-19, `UI` what=2; the terminal's `ui status`): one short line for a slow link
/// (Bluetooth, LINK-36), the facts tests poll, the same values as the full readback:
/// `st scr=Home act=A keyed=- remote=0 mode=off A=RX/20 B=heard/12 C=-/0 xb=0`. Per slot its card's state word
/// (`-` for idle, spaces as `_`: `RX_muted`) and its meter; `xb=1,STATE,PASSES` while the repeater runs. About
/// 70-85 bytes, 140 at the very most: always one answer.
pub fn status_line(app: &App, live: &LiveAll, l: &mut Line) {
    l.clear();
    l.push(b"st scr=");
    l.push(id(app.screen).as_bytes());
    let _ = uwrite!(l, " act={}", letter(app.active) as char);
    keyed_remote_mode(app, live, l);
    for s in 0..SLOTS as u8 {
        let _ = uwrite!(l, " {}=", letter(s) as char);
        match app.state(s, live).0 {
            "idle" => l.push(b"-"),
            w => {
                for &c in w.as_bytes() {
                    l.push(&[if c == b' ' { b'_' } else { c }]);
                }
            }
        }
        let _ = uwrite!(l, "/{}", w::meter_level(live.slots[s as usize].rssi));
    }
    match live.mode {
        ferrite_core::msg::ModeLive::Xb(x) if screens::xb::running(app) => {
            let _ = uwrite!(l, " xb=1,{},{}", x.state, x.passes);
        }
        _ if screens::xb::running(app) => l.push(b" xb=1,0,0"),
        _ => l.push(b" xb=0"),
    }
}

fn describe<S: ChannelStore>(app: &App, cx: &Cx<S>, d: &mut Desc) {
    let sc = app.screen;
    if let Some(l) = d.ln() {
        l.push(b"screen ");
        l.push(id(sc).as_bytes());
        l.push(b" ");
        q(l, sc.name().as_bytes());
        let mut t = Buf::<10>::new();
        sc.title(app, &mut t);
        kq(l, "title", t.as_bytes());
    }
    if let Some(l) = d.ln() {
        l.push(b"state");
        kn(l, "frame", app.frames);
        kn(l, "ago", app.now.wrapping_sub(app.drawn_at));
        kn(l, "dark", app.dark() as u32);
        kn(l, "locked", app.locked as u32);
        let _ = uwrite!(l, " active={}", letter(app.active) as char);
        keyed_remote_mode(app, cx.live, l);
        kn(l, "fm", app.radio.fm_on as u32);
    }
    if let Some(l) = d.ln() {
        let b = app.status_bar(cx.live, cx.dev);
        l.push(b"bar");
        kn(l, "lock", b.locked as u32);
        kn(l, "scan", b.scan as u32);
        kn(l, "watch", b.watch as u32);
        kn(l, "inbox", b.inbox as u32);
        kn(l, "alert", b.alert as u32);
        l.push(match b.gps {
            GpsState::Off => b" gps=off".as_slice(),
            GpsState::Searching => b" gps=searching",
            GpsState::Fix => b" gps=fix",
        });
        kn(l, "bt", b.bt as u32);
        kn(l, "battery", cx.dev.battery_pct as u32);
        kn(l, "low", b.battery_low as u32);
        kn(l, "cutoff", b.battery_cutoff as u32);
        match b.clock {
            Some(m) => {
                let _ = uwrite!(l, " clock={}:{}{}", m / 60, (m % 60) / 10, m % 10);
            }
            None => l.push(b" clock=-"),
        }
        kn(l, "utc", b.utc as u32);
    }
    if let Some(t) = app.toast {
        if let Some(l) = d.ln() {
            l.push(if t.center { b"popup ".as_slice() } else { b"toast " });
            q(l, t.head.as_bytes());
            l.push(b" ");
            q(l, t.body.as_bytes());
        }
    }
    if app.qmenu.is_some() {
        for (key, name, v, _, _) in screens::qmenu::lines(app) {
            if let Some(l) = d.ln() {
                l.push(b"qmenu ");
                q(l, key.as_bytes());
                l.push(b" ");
                q(l, name.as_bytes());
                l.push(b" ");
                q(l, v.as_bytes());
            }
        }
    }
    body(app, cx, d);
    if let Some(l) = d.ln() {
        l.push(b"hints");
        for (k, v) in sc.hints(app).as_slice() {
            l.push(b" ");
            q(l, k.as_bytes());
            l.push(b" ");
            q(l, v.as_bytes());
        }
    }
}

/// A list row: `row N "name" "value"` with `sel`, `dot`, `grey` when they apply. N is the number the row shows (0
/// for none).
fn row(d: &mut Desc, num: u8, name: &[u8], value: &[u8], sel: bool, dot: bool, grey: bool) {
    if let Some(l) = d.ln() {
        let _ = uwrite!(l, "row {} ", num);
        q(l, name);
        l.push(b" ");
        q(l, value);
        for (on, w) in [(sel, " sel"), (dot, " dot"), (grey, " grey")] {
            if on {
                l.push(w.as_bytes());
            }
        }
    }
}

/// `help "where" "text"`: the help box.
fn help(d: &mut Desc, at: &[u8], text: &[u8]) {
    if let Some(l) = d.ln() {
        l.push(b"help ");
        q(l, at);
        l.push(b" ");
        q(l, text);
    }
}

/// A label + value page ([`screens::table`]), row `sel` selected.
fn table<S: ChannelStore>(app: &App, cx: &Cx<S>, t: &screens::table::Table<()>, sel: Option<u8>, d: &mut Desc) {
    let ctx = screens::table::Ctx { app, live: cx.live, dev: cx.dev, x: &() };
    for (i, (label, val)) in t.rows.iter().enumerate() {
        let mut b = Buf::<40>::new();
        val(&ctx, &mut b);
        row(d, 0, label.as_bytes(), b.as_bytes(), sel == Some(i as u8), false, false);
    }
}

/// What each screen adds: the same values its `render` draws.
fn body<S: ChannelStore>(app: &App, cx: &Cx<S>, d: &mut Desc) {
    use screens::*;
    match app.screen {
        Screen::Home => {
            for s in 0..SLOTS as u8 {
                card(app, cx, s, d);
                bcast_line(app, cx, s, d);
            }
        }
        Screen::Quick(qk) => {
            if quick::fm_rows(app) {
                return; // BCR-28: FM's own rows (not described yet)
            }
            for (i, item) in QUICK.into_iter().enumerate() {
                let v = quick::row_value(app, item);
                row(d, ((i + 1) % 10) as u8, item.name().as_bytes(), v.as_bytes(), i as u8 == qk.row, !app.is_default(item), app.greyed(item).is_some());
            }
        }
        Screen::Advanced(a) => {
            if let Some(l) = d.ln() {
                l.push(b"recent");
                for r in app.recent.iter().flatten() {
                    l.push(b" ");
                    q(l, r.label().as_bytes());
                }
                if a.row >= advanced::RECENT {
                    kn(l, "sel", (a.row - advanced::RECENT) as u32 + 1);
                }
            }
            for (i, (name, _)) in advanced::GROUPS.iter().enumerate() {
                let v = advanced::group_value(app, cx.store.count(), cx.dev, i);
                row(d, i as u8 + 1, name.as_bytes(), v.as_bytes(), i as u8 == a.row, false, false);
            }
        }
        Screen::Group(g) => {
            for i in 0..g.items().len() as u8 {
                let (name, v, dot, grey) = g.row(app, cx.dev, cx.store, i);
                row(d, i + 1, name.as_bytes(), v.as_bytes(), i == g.row, dot, grey);
            }
            let (path, h) = g.help(app);
            help(d, path.as_bytes(), h.as_bytes());
        }
        Screen::Options(o) => {
            let all = opts::rows(o.of);
            for (i, (label, act, _)) in all.iter().enumerate() {
                let v: &[u8] = if matches!(act, opts::Act::Open(_)) { b">" } else { b"" };
                row(d, i as u8 + 1, label.as_bytes(), v, i as u8 == o.row, false, false);
            }
            row(d, 0, b"Full menu", b">", false, false, false);
            let (label, _, h) = all[o.row as usize % all.len()];
            if o.confirm {
                ask(d, label.as_bytes(), b"?", h.as_bytes());
            } else {
                help(d, opts::title(o.of), h.as_bytes());
            }
        }
        Screen::Status(s) => table(app, cx, &status::PAGE, Some(s.row), d),
        Screen::Link(l) if l.page == link::LinkPage::Status => table(app, cx, &link::STATUS, None, d),
        Screen::Info(i) => {
            let mut b = Buf::<40>::new();
            if let Some((head, text, _)) = info::message(i.page, cx.dev, &mut b) {
                if let Some(l) = d.ln() {
                    l.push(b"message ");
                    q(l, head.as_bytes());
                    l.push(b" ");
                    q(l, b.as_bytes());
                    l.push(b" ");
                    q(l, text.as_bytes());
                }
            } else if !matches!(i.page, info::Page::Help(_)) {
                let p = info::def(i.page);
                let t = screens::table::Table { rows: p.rows, go: &[], y: 0, h: 0, style: w::KV_STATUS };
                table(app, cx, &t, None, d);
                if !p.note.is_empty() {
                    help(d, p.title.as_bytes(), p.note.as_bytes()); // the page's note under its rows
                }
            }
        }
        Screen::Value(v) => {
            if let Some(l) = d.ln() {
                let (cur, _) = value::cur(app, v.item);
                let mut t = Buf::<16>::new();
                value::text_of(app, v.item, cur, &mut t);
                l.push(b"value ");
                q(l, v.item.name().as_bytes());
                l.push(b" ");
                q(l, t.as_bytes());
                if let Some(why) = app.greyed(v.item) {
                    kq(l, "grey", why.as_bytes());
                }
            }
        }
        Screen::Confirm(c) => {
            let (title, goes, kept, _) = confirm::words(c.ask);
            if let Some(l) = d.ln() {
                l.push(b"ask ");
                q(l, title.as_bytes());
                kq(l, "goes", goes.as_bytes());
                kq(l, "kept", kept.as_bytes());
            }
        }
        Screen::Coming(c) => {
            if let Some(l) = d.ln() {
                let mut h = Buf::<32>::new();
                c.dest.heading(&mut h);
                l.push(b"coming ");
                q(l, h.as_bytes());
            }
        }
        Screen::TxRefused(t) => {
            if let Some(l) = d.ln() {
                l.push(b"refused");
                slot_word(l, t.slot);
                kq(l, "why", transmit::short(t.why).as_bytes());
                kn(l, "beeps", app.refusal_beeps as u32); // SND-4: refusal beeps sent since power-on
                if let Some((path, name, _)) = transmit::fix(app, t.why) {
                    kq(l, "fix", path.as_bytes()); // TX-20: the menu path that fixes it, and what it is
                    kq(l, "fixes", name.as_bytes());
                }
            }
        }
        Screen::Transmit(t) => {
            if let Some(l) = d.ln() {
                l.push(b"transmit");
                slot_word(l, t.slot);
                kn(l, "remote", app.remote_keyed() as u32);
                kn(l, "dial", t.dial as u32);
                kn(l, "secs", app.now.wrapping_sub(app.radio.t0) / 1000);
            }
        }
        Screen::Emergency(e) => {
            if let Some(l) = d.ln() {
                l.push(b"emergency");
                slot_word(l, e.slot);
            }
        }
        Screen::Entry(e) => {
            if let Some(l) = d.ln() {
                l.push(b"entry ");
                q(l, &e.buf[..e.n as usize % (e.buf.len() + 1)]);
            }
        }
        Screen::Zones(z) => zones_lines(app, cx, z, d),
        Screen::Text(_) => {
            if let Some(l) = d.ln() {
                l.push(b"text ");
                q(l, app.text.as_bytes());
            }
        }
        Screen::Fm(f) if f.menu => {
            for (i, (name, ..)) in fm::OPTIONS.iter().enumerate() {
                let mut v = Buf::<24>::new();
                fm::option_text(app, cx.dev, i as u8, &mut v);
                row(d, i as u8 + 1, name.as_bytes(), v.as_bytes(), i as u8 == f.row, false, false);
            }
            row(d, 0, b"Full menu", b">", false, false, false);
        }
        Screen::Fm(_) => {
            if let Some(l) = d.ln() {
                let f = fm::Fm::shown(app, cx.store.data(), cx.live);
                l.push(b"fm freq=");
                l.push(f.freq.as_bytes());
                kq(l, "title", f.title.as_bytes());
                kn(l, "stereo", f.stereo as u32);
                kn(l, "dbuv", f.dbuv as u32);
                kn(l, "snr", f.snr as u32);
                kq(l, "rds", f.rds.as_bytes());
                kq(l, "preset", f.preset.as_bytes());
                kq(l, "monitor", f.monitor.as_bytes());
            }
        }
        Screen::Hf(h) => hf_lines(app, cx, h, d),
        Screen::Pin(p) => {
            // PIN-2, PIN-3: the heading, a star per digit typed (never the digits), the wait or attempts left
            if let Some(l) = d.ln() {
                let (head, n, note) = p.shown(app);
                l.push(b"pin ");
                q(l, head.as_bytes());
                kq(l, "typed", &b"******"[..(n as usize).min(6)]);
                kq(l, "note", note.as_bytes());
            }
        }
        Screen::ToneSearch(ts) => {
            // TSRCH-2: the tone found ("" while searching), the slot's signal, the question, the last hits
            if let Some(l) = d.ln() {
                let mut t = Buf::<16>::new();
                crate::slot::tone_text(ts.shown(app, cx.live), &mut t);
                l.push(b"tsearch");
                slot_word(l, ts.slot);
                kq(l, "found", t.as_bytes());
                let lv = cx.live.slots[ts.slot as usize];
                kn(l, "open", lv.squelch_open as u32);
                kq(l, "s", w::s_text(lv.rssi).as_bytes());
                kn(l, "ask", ts.ask_tx as u32);
                l.push(b" hits=");
                let mut hits = Buf::<64>::new();
                for h in app.radio.hits.iter().filter(|h| h.what != 0) {
                    crate::slot::tone_text(h.what as u16, &mut t);
                    if !hits.is_empty() {
                        hits.push(b",");
                    }
                    hits.push(t.as_bytes());
                }
                q(l, hits.as_bytes());
            }
        }
        Screen::Counter(c) => {
            // TSRCH-6, TSRCH-11: the frequency (0 while listening) and level shown, live or the last hit's, held
            if let Some(l) = d.ln() {
                let (f, rssi, now) = c.shown(app, cx.live);
                l.push(b"counter freq=");
                mhz(l, f);
                kq(l, "s", if f == 0 { Buf::new() } else { w::s_text(rssi) }.as_bytes());
                kn(l, "meter", if f == 0 { 0 } else { w::meter_level(rssi) as u32 });
                kn(l, "live", now as u32);
                kn(l, "hold", c.hold as u32);
            }
        }
        Screen::Squelch(sq) => {
            // SQL-7, SQL-12: the level, open or closed, the live signal and the noise floor beside it
            if let Some(l) = d.ln() {
                let lv = cx.live.slots[app.active as usize];
                let _ = uwrite!(l, "squelch level={}", app.sq_level(app.active));
                kn(l, "open", lv.squelch_open as u32);
                kq(l, "s", w::s_text(lv.rssi).as_bytes());
                kq(l, "floor", sq.floor_text(lv.rssi, lv.squelch_open).as_bytes());
            }
        }
        Screen::Scope(sc) => {
            if let Some(l) = d.ln() {
                l.push(b"scope centre=");
                mhz(l, app.radio.centre);
                kn(l, "step", scope::step_of(app) as u32 * 10);
                let n = scope::bars_n(app);
                kn(l, "bars", n as u32);
                if let ferrite_core::msg::ModeLive::Scope { bars, n: swept, sweeps, .. } = cx.live.mode {
                    let swept = (swept as usize).min(n);
                    kn(l, "swept", swept as u32);
                    kn(l, "blank", bars[..swept].iter().filter(|&&b| b == 0).count() as u32);
                    kn(l, "sweeps", sweeps as u32);
                }
                l.push(b" marker=");
                mhz(l, scope::bar_freq(app.radio.centre, scope::step_of(app), n, sc.marker as i32));
                kn(l, "listen", sc.listen as u32);
            }
        }
        Screen::Crossband(c) => xb_lines(app, cx, c, d),
        Screen::Setup(s) => {
            let mut rows = [(Buf::<20>::new(), Buf::<32>::new(), false); 10];
            let mut k = 0;
            let (page, of, head, note) = s.shown(app, cx.store.count(), &mut |name, v, sel| {
                if k < rows.len() {
                    rows[k] = (Buf::from(name), Buf::from(v), sel);
                    k += 1;
                }
            });
            if let Some(l) = d.ln() {
                let _ = uwrite!(l, "setup page={} of={}", page, of);
                kq(l, "head", head.as_bytes());
            }
            for (name, v, sel) in &rows[..k] {
                row(d, 0, name.as_bytes(), v.as_bytes(), *sel, false, false);
            }
            if !note.is_empty() {
                help(d, b"summary", note.as_bytes());
            }
        }
        Screen::Compose(c) => {
            if let Some(l) = d.ln() {
                let (to, count, text, field) = c.shown(app, cx.store);
                l.push(b"compose");
                kq(l, "to", to.as_bytes());
                kq(l, "count", count.as_bytes());
                kq(l, "text", text.as_bytes());
                kn(l, "field", field as u32);
            }
        }
        Screen::Gps(g) if g.page == gps::Page::Position => {
            let p = gps::position_shown(app, cx.store.data(), cx.dev);
            if let Some(l) = d.ln() {
                l.push(b"gps ");
                q(l, p.status.as_bytes());
                kq(l, "age", p.age.as_bytes());
            }
            if let Some((a, b, tiles)) = &p.fix {
                if let Some(l) = d.ln() {
                    l.push(b"pos ");
                    q(l, a.as_bytes());
                    l.push(b" ");
                    q(l, b.as_bytes());
                }
                for (name, v) in gps::tile_names().zip(tiles) {
                    row(d, 0, name, v.as_bytes(), false, false, false);
                }
                if let Some(l) = d.ln() {
                    l.push(b"near ");
                    match &p.near {
                        Some((name, dist, deg)) => {
                            q(l, name.as_bytes());
                            l.push(b" ");
                            q(l, dist.as_bytes());
                            kn(l, "deg", *deg as u32);
                        }
                        None => l.push(b"\"\" \"None: # marks one here\""),
                    }
                }
            }
        }
        _ => {}
    }
}

/// HF mode (HF-15): `hf loading=N` while the SSB patch loads; its options page as rows; else `hf freq=MHz
/// band="31 m" rx="SI4732" mode="AM" step="5 kHz" bw="6 kHz" bfo="" signal="SNR 12 dB" entry="" tx=0 seek=0`.
fn hf_lines<S: ChannelStore>(app: &App, cx: &Cx<S>, h: screens::hf::Hf, d: &mut Desc) {
    let v = h.shown(app, cx.live);
    if let Some(pct) = v.loading {
        if let Some(l) = d.ln() {
            l.push(b"hf");
            kn(l, "loading", pct as u32);
        }
        return;
    }
    if let Some((sel, vals)) = &v.menu {
        for (i, (name, val)) in screens::hf::option_names().zip(vals).enumerate() {
            row(d, i as u8 + 1, name.as_bytes(), val.as_bytes(), i as u8 == *sel, false, false);
        }
        row(d, 0, b"Full menu", b">", false, false, false);
        return;
    }
    if let Some(l) = d.ln() {
        l.push(b"hf freq=");
        mhz(l, app.settings().hf.freq);
        kq(l, "band", v.band.as_bytes());
        kq(l, "rx", v.rx.as_bytes());
        kq(l, "mode", v.mode.as_bytes());
        kq(l, "step", v.step.as_bytes());
        kq(l, "bw", v.bw.as_bytes());
        kq(l, "bfo", v.bfo.as_bytes());
        kq(l, "signal", v.signal.as_bytes());
        kq(l, "entry", v.entry.as_bytes());
        kn(l, "tx", v.tx as u32);
        kn(l, "seek", v.seeking as u32);
    }
}

/// The crossband repeater (XB-2, XB-17): `xb running=0 cell=N`, each side (`row 0 "Side 1" "A 145.49500"`), the
/// grid as rows and XB-2's checks (`check ok|warn|fail "text"`); running, `xb running=1 state=N by=WHO side1=A
/// side2=B pass=S passes=N listen=0|1 ab=N ba=N` (XB-20.6, XB-15, XB-16: the strip's columns each way) and the four
/// cells as rows.
fn xb_lines<S: ChannelStore>(app: &App, cx: &Cx<S>, c: screens::xb::Crossband, d: &mut Desc) {
    use screens::xb;
    if xb::running(app) {
        let x = match cx.live.mode {
            ferrite_core::msg::ModeLive::Xb(x) => x,
            _ => Default::default(),
        };
        let cfg = app.settings().xb;
        if let Some(l) = d.ln() {
            // XB-20.6: who started it; XB-15: SIDE1 listening; XB-16: the strip's columns each way
            l.push(b"xb running=1");
            kn(l, "state", x.state as u32);
            let _ = uwrite!(l, " by={} side1={} side2={}", xb::by_word(app.radio.xb_by), letter(cfg.side1) as char, letter(cfg.side2) as char);
            kn(l, "pass", x.pass_s as u32);
            kn(l, "passes", x.passes as u32);
            kn(l, "listen", (app.radio.xb_by & xb::LISTEN != 0) as u32);
            kn(l, "ab", x.strip.iter().filter(|&&v| v > 0).count() as u32);
            kn(l, "ba", x.strip.iter().filter(|&&v| v < 0).count() as u32);
        }
        for (name, v) in xb::CELLS.iter().zip(xb::cells(&cfg, &x)) {
            row(d, 0, name, v.as_bytes(), false, false, false);
        }
        return;
    }
    let (sides, grid, checks) = xb::start_shown(app, cx.dev.battery_pct);
    if let Some(l) = d.ln() {
        l.push(b"xb running=0");
        kn(l, "cell", c.cell as u32);
    }
    for (i, s) in sides.iter().enumerate() {
        if let Some(l) = d.ln() {
            let _ = uwrite!(l, "row 0 \"Side {}\" \"{} ", i + 1, letter(*s) as char);
            mhz(l, app.slot(*s).freq());
            l.push(b"\"");
            if c.cell == i as u8 {
                l.push(b" sel");
            }
        }
    }
    for (i, (name, v)) in xb::grid_names().iter().zip(&grid).enumerate() {
        row(d, 0, name.as_bytes(), v.as_bytes(), c.cell as usize == 2 + i, false, false);
    }
    for (k, t) in &checks {
        if let Some(l) = d.ln() {
            l.push(["check ok ", "check warn ", "check fail "][*k as usize % 3].as_bytes());
            q(l, t.as_bytes());
        }
    }
}

/// The zones page (CHAN-27 to CHAN-30): `head`, the Nearby row and the zone rows on the page (`row 0 "name" "value"`
/// with `kind=nearby` or `kind=zone` and `chips="A SCAN HID"`), the preview (`preview "NAME" "state"`, a `pvrow
/// "name" "place" "freq"` per channel, `pvfoot "..."`); the zone editor's rows as a list's.
fn zones_lines<S: ChannelStore>(app: &App, cx: &Cx<S>, z: screens::zones::Zones, d: &mut Desc) {
    use screens::zones::*;
    let st = cx.store;
    if let Some(r) = z.edit() {
        for i in 0..EDITOR_ROWS {
            let (label, v, dot) = editor_row(st, z.cur.sel as u8, i);
            row(d, 0, label.as_bytes(), v.as_bytes(), i as u8 == r, dot, false);
        }
        return;
    }
    if let Some(l) = d.ln() {
        l.push(b"head ");
        q(l, head_text(st).as_bytes());
    }
    let near = nearby_now(app, st);
    let on_near = z.on_nearby() && near.is_some();
    if let Some(n) = near {
        if let Some(l) = d.ln() {
            let mut p = Buf::<12>::new();
            crate::screens::lists::place_text(n.m, n.deg, app.settings().units, &mut p);
            l.push(b"row 0 \"Nearby\" ");
            q(l, p.as_bytes());
            if on_near {
                l.push(b" sel");
            }
            l.push(b" kind=nearby");
        }
    }
    let n = st.zones() as u16;
    let keep = |i: u16| st.zone_name(i as u8).is_some();
    let vis = zone_rows(near.is_some());
    let cur = z.cur.shown(n, vis, &keep);
    cur.rows(n, vis, &keep, |_, i| {
        let r = ZoneRow::of(app, st, i as u8);
        if let Some(l) = d.ln() {
            l.push(b"row 0 ");
            q(l, r.name.as_bytes());
            let _ = uwrite!(l, " \"{}\"", r.count);
            if !on_near && i == cur.sel {
                l.push(b" sel");
            }
            if r.count == 0 {
                l.push(b" grey");
            }
            l.push(b" kind=zone");
            let mut c = Buf::<16>::new();
            r.chip_words(&mut c);
            kq(l, "chips", c.as_bytes());
        }
    });
    let p = if on_near {
        preview_nearby(app, st)
    } else if (0..n).any(keep) {
        preview_zone(app, st, cur.sel as u8)
    } else {
        return;
    };
    if let Some(l) = d.ln() {
        l.push(b"preview ");
        q(l, p.head.as_bytes());
        l.push(b" ");
        q(l, p.state.as_bytes());
    }
    for r in p.rows.iter().flatten() {
        if let Some(l) = d.ln() {
            l.push(b"pvrow ");
            q(l, r.name.as_bytes());
            l.push(b" ");
            q(l, r.place.as_bytes());
            l.push(b" ");
            q(l, r.freq.as_bytes());
        }
    }
    if let Some(l) = d.ln() {
        l.push(b"pvfoot ");
        q(l, p.foot.as_bytes());
    }
}

/// A question with its options in the hint bar: `ask "title" ...`.
fn ask(d: &mut Desc, what: &[u8], tail: &[u8], text: &[u8]) {
    if let Some(l) = d.ln() {
        l.push(b"ask ");
        let mut t = Buf::<48>::new();
        t.push(what);
        t.push(tail);
        q(l, t.as_bytes());
        kq(l, "text", text);
    }
}

/// ` slot=A` (or HF).
fn slot_word(l: &mut Line, s: u8) {
    if (s as usize) < SLOTS {
        let _ = uwrite!(l, " slot={}", letter(s) as char);
    } else {
        l.push(b" slot=HF");
    }
}

/// Home's card for slot `s`, as [`screens::home`] draws it (HOME-4): the header, the state, the frequency or
/// the channel's name, the chips and the S-meter.
fn card<S: ChannelStore>(app: &App, cx: &Cx<S>, s: u8, d: &mut Desc) {
    let Some(l) = d.ln() else { return };
    let lv = cx.live.slots[s as usize];
    let cfg = *app.slot(s).cfg();
    let _ = uwrite!(l, "card {}", letter(s) as char);
    kn(l, "active", (s == app.active) as u32);
    let mut head = Buf::<32>::new();
    app.head_text(s, cx.live, cx.store, &mut head);
    kq(l, "head", head.as_bytes());
    let state = app.state(s, cx.live).0;
    kq(l, "state", state.as_bytes());
    if state == "heard" {
        // TSRCH-13: the tone hint as the card draws it ("Heard 88.5")
        let mut h = Buf::<24>::new();
        screens::shared::heard_text(lv.heard_tone, cfg.rx_tone, &mut h);
        kq(l, "hint", h.as_bytes());
    }
    l.push(b" freq=");
    mhz(l, app.shown_freq(s, cx.live));
    kq(l, "name", app.chan_name(s, cx.live).as_bytes());
    let kind = screens::home::card_kind(app, s);
    if kind == 1 {
        // APRS-17: the APRS card shows no mode, tone or shift chips, but the last heard and the beacon lines and
        // the stations heard; C's own VFO (AM, say) is put aside meanwhile, so its chips would mislead.
        let data = cx.store.data();
        let mut last = Buf::<80>::new();
        crate::aprs::last_text(data, &mut last);
        let mut beacon = Buf::<28>::new();
        app.beacon_text(data, &mut beacon);
        kq(l, "last", last.as_bytes());
        kq(l, "beacon", beacon.as_bytes());
        kn(l, "heard", data.stations() as u32);
    }
    let (mut m, mut t, mut o) = (Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new());
    mode_chip(&cfg, &mut m);
    card_tone(&cfg, &mut t);
    shift_chip(&cfg, &mut o);
    if kind != 1 {
        kq(l, "mode", m.as_bytes());
        kq(l, "tone", t.as_bytes());
        kq(l, "shift", o.as_bytes());
        let pw: &[u8] = if app.capped(s) { b"0.25 W" } else { [b"LOW".as_slice(), b"MID", b"HIGH"][cfg.power as usize % 3] };
        kq(l, "power", pw);
    }
    kq(l, "s", w::s_text(lv.rssi).as_bytes());
    kn(l, "meter", w::meter_level(lv.rssi) as u32);
    kn(l, "rx", lv.squelch_open as u32);
    kn(l, "scan", lv.scanning as u32);
    l.push(match kind {
        1 => b" kind=aprs".as_slice(),
        2 => b" kind=broadcast",
        _ => b" kind=slot",
    });
}

/// A broadcast card's report after its `card` line (BCR-26): `bcast A "RDS text" stereo=1 dbuv=48 snr=24` (the
/// text empty: "No RDS"; while another slot plays it, where it plays).
fn bcast_line<S: ChannelStore>(app: &App, cx: &Cx<S>, s: u8, d: &mut Desc) {
    if screens::home::card_kind(app, s) != 2 {
        return;
    }
    let Some(l) = d.ln() else { return };
    let (stereo, rssi, snr, text, _, _) = screens::home::bcast_shown(app, cx.live, s);
    let _ = uwrite!(l, "bcast {} ", letter(s) as char);
    q(l, text.as_bytes());
    kn(l, "stereo", stereo as u32);
    kn(l, "dbuv", rssi as u32);
    kn(l, "snr", snr as u32);
}

/// The screen's stable id: its [`Screen`] variant.
pub fn id(s: Screen) -> &'static str {
    match s {
        Screen::Home => "Home",
        Screen::Edit(_) => "Edit",
        Screen::Quick(_) => "Quick",
        Screen::Advanced(_) => "Advanced",
        Screen::Status(_) => "Status",
        Screen::Entry(_) => "Entry",
        Screen::ChEdit(_) => "ChEdit",
        Screen::Value(_) => "Value",
        Screen::Tones(_) => "Tones",
        Screen::Text(_) => "Text",
        Screen::Save(_) => "Save",
        Screen::Confirm(_) => "Confirm",
        Screen::Shortcut(_) => "Shortcut",
        Screen::Transmit(_) => "Transmit",
        Screen::TxRefused(_) => "TxRefused",
        Screen::Emergency(_) => "Emergency",
        Screen::Scan(_) => "Scan",
        Screen::Scope(_) => "Scope",
        Screen::ToneSearch(_) => "ToneSearch",
        Screen::Counter(_) => "Counter",
        Screen::Crossband(_) => "Crossband",
        Screen::Squelch(_) => "Squelch",
        Screen::Group(_) => "Group",
        Screen::Setup(_) => "Setup",
        Screen::ExtendTx(_) => "ExtendTx",
        Screen::Pin(_) => "Pin",
        Screen::Info(_) => "Info",
        Screen::KeyTest(_) => "KeyTest",
        Screen::Link(_) => "Link",
        Screen::Fm(_) => "Fm",
        Screen::Hf(_) => "Hf",
        Screen::Alert(_) => "Alert",
        Screen::Coming(_) => "Coming",
        Screen::Log(_) => "Log",
        Screen::Options(_) => "Options",
        Screen::Aprs(_) => "Aprs",
        Screen::Msgs(_) => "Msgs",
        Screen::Compose(_) => "Compose",
        Screen::QuickMsgs(_) => "QuickMsgs",
        Screen::Gps(_) => "Gps",
        Screen::Browse(_) => "Browse",
        Screen::Zones(_) => "Zones",
        Screen::ScanLists(_) => "ScanLists",
        Screen::Recall(_) => "Recall",
        Screen::PosEntry(_) => "PosEntry",
        Screen::SameCodes(_) => "SameCodes",
        Screen::Dtmf(_) => "Dtmf",
    }
}

// ---- Key names (the terminal's `key`, `ui keys`; tools/ferrite_link.py has the same) ----

/// A key's name: MENU (OK too), BACK, UP, DOWN, LEFT, RIGHT, SLOT, VM, 0-9, STAR, HASH, SIDE1, SIDE2.
pub fn key_name(code: u8) -> &'static str {
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    match k::key(code) {
        Some(Key::Menu) => "MENU",
        Some(Key::Exit) => "BACK",
        Some(Key::Up) => "UP",
        Some(Key::Down) => "DOWN",
        Some(Key::Left) => "LEFT",
        Some(Key::Right) => "RIGHT",
        Some(Key::Slot) => "SLOT",
        Some(Key::VfoMem) => "VM",
        Some(Key::Digit(d)) => DIGITS[d as usize % 10],
        Some(Key::Star) => "STAR",
        Some(Key::Hash) => "HASH",
        Some(Key::Side(1)) => "SIDE1",
        Some(Key::Side(_)) => "SIDE2",
        Some(Key::Ptt(_)) | None => "",
    }
}

/// Other names a key answers to.
const ALIASES: [(&str, u8); 8] = [("OK", 6), ("EXIT", 9), ("*", 13), ("#", 21), ("PF1", 0), ("PF2", 1), ("ABC", 3), ("V/M", 2)];

/// A key's stock code by name, any case.
pub fn key_code(name: &[u8]) -> Option<u8> {
    let is = |n: &str| n.as_bytes().eq_ignore_ascii_case(name);
    (0..=ferrite_core::link::key::LAST_CODE).find(|&c| is(key_name(c))).or_else(|| ALIASES.iter().find(|(n, _)| is(n)).map(|a| a.1))
}

/// `ui keys`, line `i`: each key's name, its stock code and its other names; then the knob.
pub fn key_line(i: u16, out: &mut Line) -> bool {
    out.clear();
    let codes = ferrite_core::link::key::LAST_CODE as u16 + 1;
    if i == codes {
        out.push(b"knob +N|-N (1-20 steps; + clockwise)");
        return true;
    }
    if i > codes {
        return false;
    }
    let c = i as u8;
    let _ = uwrite!(out, "{} {}", key_name(c), c);
    for (n, a) in ALIASES {
        if a == c {
            out.push(b" ");
            out.push(n.as_bytes());
        }
    }
    true
}
