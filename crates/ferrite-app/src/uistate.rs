//! What the screen shows, as text (manual chapter 29, LINK-19; decision 150): the link's `UI` and the terminal's
//! `ui`. A PC script presses keys over the link and reads this back to check the radio's UI as a person would.
//!
//! It is built from the App's own state, the same values the drawing code draws (each screen's rows come from the
//! functions its `render` uses), never from pixels. One line at a time, by number ([`line`]): nothing is held, so
//! it costs no RAM, and a page of an answer is the next lines. The lines, in this order:
//!
//! - `screen ID "Name" title="..."`: the screen (ID is stable: its variant), its name and the status bar's title.
//! - `state frame=N ago=MS dark=0 locked=0 active=A keyed=- remote=0`: redraws that changed something, the time
//!   since the last, the backlight off, the key lock, the active slot, the slot transmitting, remote PTT.
//! - `bar lock=0 scan=0 watch=0 inbox=0 alert=0 gps=fix bt=0 battery=80 low=0 clock=14:32`: the status bar.
//! - `toast "head" "body"` or `popup "head" "body"` while one shows; `qmenu "PTT A" "Reverse" "off"` per row of the
//!   quick menu while it is up.
//! - The screen's own lines: home's `card` per slot; a list's `row N "name" "value"` (with `sel`, `dot` changed
//!   from its default, `grey`), its `help`; an editor's `value`; a question's `ask`; and so on. A screen without
//!   its own lines yet shows only the rest.
//! - `hints "KEY" "label" ...`: the hint bar's pairs (under a toast, what it will show again).
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
        match app.radio.keyed {
            Some(s) if (s as usize) < SLOTS => {
                let _ = uwrite!(l, " keyed={}", letter(s) as char);
            }
            Some(_) => l.push(b" keyed=HF"),
            None => l.push(b" keyed=-"),
        }
        kn(l, "remote", app.remote_keyed() as u32);
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
        Screen::Text(_) => {
            if let Some(l) = d.ln() {
                l.push(b"text ");
                q(l, app.text.as_bytes());
            }
        }
        _ => {}
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
    app.head_text(s, cx.live, &mut head);
    kq(l, "head", head.as_bytes());
    kq(l, "state", app.state(s, cx.live).0.as_bytes());
    l.push(b" freq=");
    mhz(l, app.shown_freq(s, cx.live));
    kq(l, "name", app.chan_name(s, cx.live).as_bytes());
    let (mut m, mut t, mut o) = (Buf::<16>::new(), Buf::<16>::new(), Buf::<16>::new());
    mode_chip(&cfg, &mut m);
    card_tone(&cfg, &mut t);
    shift_chip(&cfg, &mut o);
    kq(l, "mode", m.as_bytes());
    kq(l, "tone", t.as_bytes());
    kq(l, "shift", o.as_bytes());
    let pw: &[u8] = if app.capped(s) { b"0.25 W" } else { [b"LOW".as_slice(), b"MID", b"HIGH"][cfg.power as usize % 3] };
    kq(l, "power", pw);
    kq(l, "s", w::s_text(lv.rssi).as_bytes());
    kn(l, "meter", w::meter_level(lv.rssi) as u32);
    kn(l, "rx", lv.squelch_open as u32);
    kn(l, "scan", lv.scanning as u32);
    l.push(match screens::home::card_kind(app, s) {
        1 => b" kind=aprs".as_slice(),
        2 => b" kind=broadcast",
        _ => b" kind=slot",
    });
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
