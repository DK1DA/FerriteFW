//! Transmitting (manual chapter 15): the checks at PTT (TX-1 to TX-5, TX-20, TX-35), the Transmit screen
//! (TX-12, TX-22, TX-30; Transmit mock-up), the refused screen with its fix (TX-20; TxRefused mock-up) and the
//! emergency-frequency prompt (TX-3).
//!
//! PTT goes home first (KEY-7) and runs the checks the UI can make: the crossband repeater (XB-14), the build,
//! the battery cut-off, the region's bands or Extend TX, busy lockout, the emergency frequencies. The Radio
//! task checks again when it keys (TX-17) and any refusal of its own comes back in `LiveAll::tx`.

use super::activity::{mmss, txt};
use super::nav::Dest;
use super::parts::{self, wrap};
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_name, mode_chip, tone_text};
use crate::{App, Screen};
use ferrite_core::freq::Freq10;
use ferrite_core::keys::Key;
use ferrite_core::msg::{LiveAll, RadioCmd, SLOTS};
use ferrite_core::txplan::{self, Refusal};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// TX timeout (MENU 0 8 1 1, 30-600 s, default 120 s) and pre-alert (0 8 1 2, default 10 s), until the
/// settings store holds them (TX-30).
pub const TIMEOUT_S: u32 = 120;
pub const PREALERT_S: u32 = 10;
/// The emergency prompt waits this long for the second PTT (TX-3).
pub const EMERGENCY_MS: u32 = 5000;
/// "HF": the slot number HF mode transmits as.
pub const HF_SLOT: u8 = 3;

/// The frequency a slot transmits on: the offset applied unless reversed (TX-6 checks this one).
pub fn tx_freq(app: &App, slot: u8) -> Freq10 {
    if slot == HF_SLOT {
        return app.set.hf.freq;
    }
    let s = app.slot(slot);
    let c = s.cfg();
    let f = s.freq() as i64;
    (if c.reverse { f } else { f + c.offset as i64 }).max(0) as Freq10
}

/// Why this slot can't transmit now, by the checks the UI can make.
pub fn refusal(app: &App, slot: u8) -> Option<Refusal> {
    if app.radio.mode == super::activity::Mode::Xb {
        return Some(Refusal::Crossband);
    }
    if app.radio.keyed.is_some() {
        return Some(Refusal::InProgress);
    }
    if !app.radio.tx_built {
        return Some(Refusal::RxOnlyBuild);
    }
    if app.radio.batt_cut {
        return Some(Refusal::Battery);
    }
    // CHAN-4 (safety): a receive-only channel never transmits.
    if slot < SLOTS as u8 && app.slot(slot).on_channel() && app.slot(slot).ch.is_some_and(|c| c.flags & crate::chan::FLAG_RX_ONLY != 0) {
        return Some(Refusal::RxOnlyChannel);
    }
    if let Err(r) = txplan::check(tx_freq(app, slot), app.set.tx_region(), app.set.extend_tx) {
        return Some(r);
    }
    if txplan::locked_125m(tx_freq(app, slot)) {
        return Some(Refusal::Unmeasured125); // TX-10: safety, until the bench measures the 0.25 W ceiling
    }
    // TX-21: busy lockout. Carrier: refuse while the squelch is open. Wrong tone: refuse while it is open with a
    // tone other than the slot's RX tone (with no RX tone set, any carrier is another group's).
    if slot < SLOTS as u8 && app.rx_mask & 1 << slot != 0 {
        let c = app.slot(slot).cfg();
        let other_tone = c.rx_tone == 0 || app.tone_mask & 1 << slot == 0;
        if c.busy == 1 || c.busy == 2 && other_tone {
            return Some(Refusal::Busy);
        }
    }
    None
}

/// PTT down for a slot (KEY-7): home first, then refuse, prompt (emergency) or key.
pub fn ptt(app: &mut App, slot: u8) {
    if app.radio.mode == super::activity::Mode::Xb {
        // XB-14: local PTT doesn't transmit while the repeater runs; its screen stays.
        return refuse(app, slot, Refusal::Crossband);
    }
    app.go_home();
    if let Some(r) = refusal(app, slot) {
        return refuse(app, slot, r);
    }
    if txplan::emergency(tx_freq(app, slot)) {
        app.radio.t0 = app.now.wrapping_add(EMERGENCY_MS);
        app.screen = Screen::Emergency(Emergency { slot });
        return;
    }
    key(app, slot);
}

/// Key up (after the checks, or the emergency prompt's second PTT): the Transmit screen over where it was.
pub fn key(app: &mut App, slot: u8) {
    if let Some(r) = refusal(app, slot) {
        return refuse(app, slot, r);
    }
    app.radio.keyed = Some(slot);
    app.radio.t0 = app.now;
    app.out.push(RadioCmd::Transmit { slot, on: true });
    app.open(Dest::Transmit);
    if let Screen::Transmit(t) = &mut app.screen {
        t.slot = slot;
    }
}

/// The refused screen (TX-20; the beep is the Radio task's), over the screen BACK returns to.
pub fn refuse(app: &mut App, slot: u8, why: Refusal) {
    app.open(Dest::TxRefused);
    app.screen = Screen::TxRefused(TxRefused { why, slot });
}

/// TX-35: an automatic transmission (beacon, KISS frame, VOX, remote KEY) on this slot may go, or is refused
/// with a toast saying why. Only a person pressing PTT twice transmits on an emergency frequency.
pub fn auto_tx(app: &mut App, slot: u8, what: &[u8]) -> bool {
    let f = tx_freq(app, slot);
    let why: &[u8] = if txplan::emergency(f) {
        b"emergency frequency"
    } else if app.set.callsign.as_bytes().is_empty() {
        b"no callsign set"
    } else if let Some(r) = refusal(app, slot) {
        short(r).as_bytes()
    } else {
        return true;
    };
    let mut h = Buf::<20>::new();
    h.push(what);
    h.push(b" not sent:");
    app.show_toast(h.as_bytes(), why, false);
    false
}

/// A refusal in a few words (toasts).
pub fn short(r: Refusal) -> &'static str {
    match r {
        Refusal::OutOfBand => "outside the band plan",
        Refusal::NotCalibrated => "not calibrated there",
        Refusal::RxOnlyChannel => "receive-only channel",
        Refusal::Busy => "channel busy",
        Refusal::Battery => "battery low",
        Refusal::Locked => "transmit locked",
        Refusal::InProgress => "already transmitting",
        Refusal::NoCallsign => "no callsign set",
        Refusal::RxOnlyBuild => "receive-only build",
        Refusal::ReceiveOnlyHere => "receive only here",
        Refusal::Crossband => "repeater running",
        Refusal::Unmeasured125 => "1.25 m locked",
    }
}

/// The screen and menu path that fix a refusal (TX-20), if one does.
fn fix(app: &App, r: Refusal) -> Option<(&'static str, &'static str, Dest)> {
    match r {
        Refusal::OutOfBand if app.set.region().is_none() => Some(("MENU 0 9 4", "Region", Dest::Group(9))),
        Refusal::OutOfBand => Some(("MENU 0 8 9", "Extend TX", Dest::Group(8))),
        Refusal::NotCalibrated => Some(("MENU 0 9 7 1", "Calibration", Dest::Calibration)),
        Refusal::RxOnlyChannel => Some(("MENU 8", "Channel editor", Dest::ChannelEdit)),
        Refusal::Busy => Some(("MENU 0 8 8", "Busy lockout", Dest::Group(8))),
        Refusal::Battery => Some(("#", "Status page", Dest::Status)),
        Refusal::Locked | Refusal::RxOnlyBuild => Some(("MENU 0 9 9", "About", Dest::About)),
        Refusal::NoCallsign => Some(("MENU 0 8 7 1", "Callsign", Dest::Setup)),
        Refusal::InProgress | Refusal::ReceiveOnlyHere | Refusal::Crossband | Refusal::Unmeasured125 => None,
    }
}

// ---- Transmit ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Transmit {
    /// The slot keyed (3 = HF mode).
    pub slot: u8,
}

/// The level name (TX-12): LOW, MID, HIGH; "0.25 W cap" on 1.25 m.
pub fn power_name(app: &App, slot: u8) -> &'static str {
    if slot < SLOTS as u8 && app.capped(slot) {
        return "0.25 W cap";
    }
    let p = if slot < SLOTS as u8 { app.slot(slot).cfg().power } else { 0 };
    ["LOW", "MID", "HIGH"][p as usize % 3]
}

impl View for Transmit {
    fn name(self) -> &'static str {
        "Transmit"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("Transmit.png")
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"TX");
    }

    /// TX-22: while keyed, no key does anything (hold BACK still goes home, and PTT up unkeys).
    fn key(self, _: &mut App, _: KeyEvent, _: &impl ChannelStore) {}

    fn ptt(self, app: &mut App, down: bool) {
        if !down {
            app.back(); // home, or HF mode it was keyed from
        }
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        // The Radio task refused (its own checks: calibration, read-back, battery under load).
        if let Some(r) = live.tx.refused.filter(|_| live.tx.slot == self.slot && app.radio.keyed.is_some()) {
            app.radio.keyed = None;
            app.screen = Screen::TxRefused(TxRefused { why: r, slot: self.slot });
            return;
        }
        // TX-30: unkey at the timeout; PTT must come up before the next transmission.
        if app.radio.keyed.is_some() && app.now.wrapping_sub(app.radio.t0) >= TIMEOUT_S * 1000 {
            app.ptt_released();
            app.back();
            app.show_toast(b"TX timeout", b"release PTT", false);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let live = cx.live;
        let s = self.slot;
        let hf = s == super::transmit::HF_SLOT;
        let secs = if app.radio.keyed.is_some() { app.now.wrapping_sub(app.radio.t0) / 1000 } else { 0 };
        let (y, h) = (25, 164);
        if app.dirty(1, 1) {
            parts::card(lcd, 6, y, 228, h, RED);
            lcd.fill_r(7, y + 1, 226, 20, 5, RED);
            lcd.fill(7, y + 12, 226, 9, RED);
            lcd.text4_rows(15, y + 15, b"TRANSMIT", &UI_12B, WHITE, RED, y + 2, y + 20);
        }
        let mut t = Buf::<16>::new();
        mmss(secs, &mut t);
        if app.dirty(2, hash(&t)) {
            w::field(lcd, 160, 66, y + 2, y + 20, y + 15, Align::Right, t.as_bytes(), &UI_12B, WHITE, RED);
        }
        // Slot, VFO/channel and band; what it receives on.
        let f = tx_freq(app, s);
        let rx = if hf { f } else { app.slot(s).freq() };
        let mut head = Buf::<32>::new();
        if hf {
            head.set(b"HF mode");
        } else {
            app.head_text(s, live, &mut head);
        }
        let mut r = Buf::<16>::new();
        r.push(b"RX ");
        r.push(w::freq3(rx).as_bytes());
        if app.dirty(3, hash(&(head, r))) {
            if hf {
                lcd.fill(15, y + 26, 14, 14, SURFACE);
            } else {
                w::badge(lcd, 15, y + 26, s, true, SURFACE);
            }
            w::field(lcd, 35, 110, y + 24, y + 42, y + 38, Align::Left, head.as_bytes(), &UI_12, SUB, SURFACE);
            w::field(lcd, 145, 80, y + 24, y + 42, y + 38, Align::Right, r.as_bytes(), &UI_12, SUB, SURFACE);
        }
        if app.dirty(4, hash(&f)) {
            w::freq(lcd, 15, 226, y + 77, f, &DIG_46, &DIG_28, TEXT, SUB, SURFACE);
        }
        // Mode, the tone sent (red), the power level name (TX-12).
        let (mut m, mut tone) = (Buf::<16>::new(), Buf::<16>::new());
        if hf {
            m.set(super::hf::mode_name(app.set.hf.mode).as_bytes());
        } else {
            let c = app.slot(s).cfg();
            mode_chip(c, &mut m);
            tone_text(c.tx_tone, &mut tone);
        }
        let pw = power_name(app, s);
        if app.dirty(5, hash(&(m, tone, pw))) {
            lcd.fill(15, y + 89, 211, 17, SURFACE);
            let mut x = 15 + w::chip(lcd, 15, y + 89, m.as_bytes(), TEXT, SURFACE) + 4;
            if !tone.is_empty() {
                tone.push(b" sent");
                let tw = text4_width(tone.as_bytes(), &UI_12) + 10;
                w::field(lcd, x, tw, y + 89, y + 106, y + 102, Align::Center, tone.as_bytes(), &UI_12, RED, SURFACE);
                lcd.frame_r(x, y + 89, tw, 17, 3, RED);
                x += tw + 4;
            }
            w::chip(lcd, x, y + 89, pw.as_bytes(), if pw.len() > 4 { AMBER } else { TEXT }, SURFACE);
        }
        // PA drive in %, never watts (TX-12).
        let drive = if live.tx.keyed && live.tx.slot == s { live.tx.drive } else { 0 };
        if app.dirty(6, hash(&drive)) {
            w::field(lcd, 15, 40, y + 110, y + 127, y + 123, Align::Left, b"Drive", &UI_12, SUB, SURFACE);
            let lit = ((drive as u32 * 13 + 50) / 100) as u8;
            parts::segs(lcd, 56, y + 115, 13, lit, 8, 10, 8, RED, SURFACE);
            let mut d = Buf::<8>::new();
            let _ = uwrite!(d, "{}%", drive);
            w::field(lcd, 186, 40, y + 110, y + 127, y + 123, Align::Right, d.as_bytes(), &UI_12B, TEXT, SURFACE);
        }
        // Timeout: the bar fills to the limit; amber from the pre-alert (TX-30).
        if app.dirty(7, hash(&secs)) {
            w::field(lcd, 15, 40, y + 127, y + 144, y + 140, Align::Left, b"TOT", &UI_12, SUB, SURFACE);
            let c = if secs + PREALERT_S >= TIMEOUT_S { AMBER } else { SUB };
            parts::bar(lcd, 56, y + 132, 110, 6, secs, TIMEOUT_S, c, SURFACE);
            let mut b = Buf::<16>::new();
            b.push(b"of ");
            mmss(TIMEOUT_S, &mut b);
            w::field(lcd, 168, 58, y + 127, y + 144, y + 140, Align::Right, b.as_bytes(), &UI_12, TEXT, SURFACE);
        }
        let mv = cx.dev.battery_mv as u32;
        if app.dirty(8, hash(&mv)) {
            let mut b = Buf::<32>::new();
            let _ = uwrite!(b, "Battery under load {}.{} V", mv / 1000, mv % 1000 / 100);
            let c = if cx.dev.battery_low { AMBER } else { SUB };
            w::field(lcd, 15, 211, y + 144, y + 161, y + 157, Align::Left, b.as_bytes(), &UI_12, c, SURFACE);
        }
        // The other slots: watched ones pause while transmitting (TX-34).
        let others = (0..SLOTS as u8).filter(|&o| o != s).count() as i32;
        if app.dirty(9, hash(&(s, app.watch))) {
            let (by, bh) = (y + h + 6, 18 * others + 5);
            lcd.fill(6, by, 228, bh, BG);
            lcd.frame_r(6, by, 228, bh, 6, LINE);
            for (i, o) in (0..SLOTS as u8).filter(|&o| o != s).enumerate() {
                let ry = by + 3 + 18 * i as i32;
                w::badge(lcd, 12, ry + 2, o, false, BG);
                let sl = app.slot(o);
                let mut t = Buf::<40>::new();
                t.push(w::freq3(sl.freq()).as_bytes());
                if let Some(c) = sl.ch.filter(|_| sl.on_channel()) {
                    t.push(" \u{b7} ".as_bytes());
                    t.push(c.name());
                } else {
                    t.push(" \u{b7} ".as_bytes());
                    t.push(band_name(sl.freq()).as_bytes());
                }
                w::field(lcd, 32, 152, ry, ry + 17, ry + 13, Align::Left, t.as_bytes(), &UI_12, DIM, BG);
                let watched = app.watch_mask() & 1 << o != 0;
                let st: &[u8] = if watched { b"paused" } else { b"off" };
                w::field(lcd, 184, 44, ry, ry + 17, ry + 13, Align::Right, st, &UI_12, DIM, BG);
            }
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("PTT", "Release to stop")])
    }
}

// ---- Refused ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TxRefused {
    pub why: Refusal,
    pub slot: u8,
}

impl View for TxRefused {
    fn name(self) -> &'static str {
        "Transmit refused"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("TxRefused.png")
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"TX refused");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Menu, Press::Short) => {
                // The fix replaces this screen: its BACK returns to where the refusal came over.
                match fix(app, self.why) {
                    Some(_) if self.why == Refusal::NoCallsign => {
                        app.back();
                        super::setup::callsign(app); // just the callsign page
                    }
                    Some((_, _, d)) => {
                        app.back();
                        app.open(d);
                    }
                    None => {}
                }
            }
            (Key::Exit, Press::Short) => app.back(),
            _ => {}
        }
    }

    /// PTT again tries again (home first).
    fn ptt(self, app: &mut App, down: bool) {
        if down {
            if self.slot == HF_SLOT {
                app.back();
                return super::hf::transmit(app);
            }
            ptt(app, self.slot);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        if !app.dirty(1, hash(&self)) {
            return;
        }
        lcd.fill(0, w::SBAR_H, 240, w::HINT_Y - w::SBAR_H, BG);
        let f = tx_freq(app, self.slot);
        let mut a = Buf::<96>::new();
        let mut more: &[u8] = b"";
        let fq = w::freq3(f);
        match self.why {
            Refusal::OutOfBand => {
                let b = txplan::nearest(f, app.set.tx_region());
                let _ = uwrite!(a, "{} MHz is outside the {} band in {} (", txt(fq.as_bytes()), b.name, app.set.tx_region().name());
                mhz_text(b.lo, &mut a);
                a.push(b"-");
                mhz_text(b.hi, &mut a);
                a.push(b").");
                more = if app.set.region().is_none() {
                    b"Region isn't set. To set it:"
                } else {
                    b"To allow any band the radio is calibrated for:"
                };
            }
            Refusal::NotCalibrated => {
                let _ = uwrite!(a, "{} MHz is outside every band the radio is calibrated for.", txt(fq.as_bytes()));
                more = b"The calibration, read only:";
            }
            Refusal::RxOnlyChannel => a.set(b"This channel is receive only."),
            Refusal::Busy if self.slot < SLOTS as u8 && app.slot(self.slot).cfg().busy == 2 => a.set(b"Busy lockout: the channel is in use with another tone."),
            Refusal::Busy => a.set(b"Busy lockout: the channel is in use."),
            Refusal::Battery => {
                a.set(b"The battery is below the transmit cut-off.");
                more = b"Charge it or fit a fresh one. Its level:";
            }
            Refusal::Locked => {
                a.set(b"Transmit is locked: a calibration or hardware check failed.");
                more = b"Which check failed:";
            }
            Refusal::InProgress => a.set(b"Another transmission is in progress."),
            Refusal::NoCallsign => {
                a.set(b"Automatic transmit needs a callsign.");
                more = b"OK sets it, on its own page:";
            }
            Refusal::RxOnlyBuild => {
                a.set(b"This build of Ferrite has no transmit code.");
                more = b"Flash a transmit build. The version:";
            }
            Refusal::ReceiveOnlyHere => {
                a.set(b"Receive only here: below 18 MHz, USB, LSB and CW are on the SI4732.");
            }
            Refusal::Crossband => a.set(b"The crossband repeater is running. BACK on its screen stops it."),
            Refusal::Unmeasured125 => {
                a.set(b"1.25 m transmit is locked until its 0.25 W limit is measured on the bench.");
                more = b"Receive works as usual. (TX-10)";
            }
        }
        // The dialog sized to its text: title, the reason, what fixes it, the path.
        let n = parts::lines(a.as_bytes(), &UI_12, 204, 4);
        let m = parts::lines(more, &UI_12, 204, 2);
        let fixes = fix(app, self.why).is_some();
        let h = 38 + 17 * n + if m > 0 { 15 * m + 4 } else { 0 } + if fixes { 26 } else { 0 };
        let y = 150 - h / 2;
        parts::dialog(lcd, y, h, b"Can't transmit", RED);
        wrap(lcd, 19, 204, y + 45, 17, n, a.as_bytes(), &UI_12, TEXT, SURFACE);
        let mut ty = y + 45 + 17 * n + 4;
        if m > 0 {
            wrap(lcd, 19, 204, ty, 15, m, more, &UI_12, SUB, SURFACE);
            ty += 15 * m - 8;
        }
        if let Some((path, name, _)) = fix(app, self.why) {
            let wd = parts::path_chip(lcd, 19, ty, path.as_bytes(), SURFACE);
            lcd.text4_rows(19 + wd + 6, ty + 13, name.as_bytes(), &UI_12, TEXT, SURFACE, ty, ty + 18);
        }
    }

    fn hints(self, app: &App) -> Hints {
        if fix(app, self.why).is_some() {
            Hints::new(&[("OK", "Go there"), ("BACK", "Close")])
        } else {
            Hints::new(&[("BACK", "Close")])
        }
    }
}

/// "430" or "24.89": a band edge in MHz, as short as it goes.
fn mhz_text(f: Freq10, b: &mut Buf<96>) {
    let (m, k) = (f / 100_000, f % 100_000 / 1000);
    let _ = uwrite!(b, "{}", m);
    if k != 0 {
        let _ = uwrite!(b, ".{}{}", k / 10, k % 10);
    }
}

// ---- Emergency prompt ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Emergency {
    pub slot: u8,
}

impl View for Emergency {
    fn name(self) -> &'static str {
        "Emergency frequency"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Emergency");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        if (e.key, e.press) == (Key::Exit, Press::Short) {
            app.back();
        }
    }

    /// TX-3: the second PTT within 5 s transmits; the first one's release does nothing.
    fn ptt(self, app: &mut App, down: bool) {
        if down {
            app.screen = Screen::Home;
            key(app, self.slot);
        }
    }

    fn tick(self, app: &mut App, _: &LiveAll) {
        if crate::after(app.now, app.radio.t0) {
            app.go_home();
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let (y, h) = (80, 120);
        let f = tx_freq(app, self.slot);
        if app.dirty(1, hash(&f)) {
            parts::dialog(lcd, y, h, b"Emergency frequency", AMBER);
            let mut a = Buf::<64>::new();
            let _ = uwrite!(a, "{} MHz is kept for distress and rescue.", txt(w::freq3(f).as_bytes()));
            wrap(lcd, 19, 204, y + 45, 17, 2, a.as_bytes(), &UI_12, TEXT, SURFACE);
            w::field(lcd, 19, 204, y + 70, y + 92, y + 86, Align::Left, b"PTT again to transmit", &UI_12B, AMBER, SURFACE);
        }
        let left = app.radio.t0.wrapping_sub(app.now).min(EMERGENCY_MS);
        let s = left.div_ceil(1000);
        if app.dirty(2, hash(&(left / 100))) {
            parts::bar(lcd, 19, y + 100, 170, 6, left, EMERGENCY_MS, AMBER, SURFACE);
            let mut b = Buf::<8>::new();
            let _ = uwrite!(b, "{} s", s);
            w::field(lcd, 192, 30, y + 95, y + 111, y + 107, Align::Right, b.as_bytes(), &UI_12, SUB, SURFACE);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("PTT", "Transmit"), ("BACK", "Cancel")])
    }
}
