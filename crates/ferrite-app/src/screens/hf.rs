//! HF mode (manual chapter 20a, HF-1 to HF-30; no mock-up): everything below 64 MHz on its own screen, from
//! MENU 0 3 5 only (HF-1). Entering loads the SSB patch behind a progress bar (HF-3, the B13 screen); BACK
//! during the load cancels. Then frequency, band, mode, receiver, step, bandwidth, signal and BFO (HF-15).
//!
//! Keys (HF-12): knob or ▲▼ tune by the step, SIDE2 + knob 1 MHz, ◀▶ band, \* mode, # the recent list, MENU
//! the HF options, BACK home (HF-4). SLOT, V/M and SIDE2 alone say why they do nothing (HF-13). PTT transmits
//! from here on chip 1 where allowed (HF-23) and refuses on the SI4732 (HF-24).

use super::activity::Mode;
use super::nav::Dest;
use super::parts;
use super::shared::*;
use super::transmit::{self, HF_SLOT};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_core::msg::{ModeLive, RadioCmd};
use ferrite_core::txplan::Refusal;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

pub const MODES: [&str; 5] = ["AM", "USB", "LSB", "CW", "FM"];
/// The patch states in `ModeLive::Hf::loading`.
pub const PATCH_NONE: u8 = 254;
pub const PATCH_DONE: u8 = 255;

/// The ◀▶ bands, low to high (an open point in the manual; broadcast and amateur bands interleaved).
pub const BANDS: [(&str, Freq10, Freq10); 19] = [
    ("LW", 15_300, 27_900),
    ("MW", 53_000, 170_000),
    ("160 m", mhz(1) + 80_000, mhz(2)),
    ("80 m", mhz(3) + 50_000, mhz(4)),
    ("49 m SW", mhz(5) + 90_000, mhz(6) + 20_000),
    ("40 m", mhz(7), mhz(7) + 20_000),
    ("41 m SW", mhz(7) + 20_000, mhz(7) + 45_000),
    ("31 m SW", mhz(9) + 40_000, mhz(9) + 90_000),
    ("25 m SW", mhz(11) + 60_000, mhz(12) + 10_000),
    ("20 m", mhz(14), mhz(14) + 35_000),
    ("19 m SW", mhz(15) + 10_000, mhz(15) + 80_000),
    ("17 m", mhz(18) + 6_800, mhz(18) + 16_800),
    ("15 m", mhz(21), mhz(21) + 45_000),
    ("13 m SW", mhz(21) + 45_000, mhz(21) + 85_000),
    ("12 m", mhz(24) + 89_000, mhz(24) + 99_000),
    ("CB", mhz(26) + 96_500, mhz(27) + 40_500),
    ("10 m", mhz(28), mhz(29) + 70_000),
    ("6 m", mhz(50), mhz(54)),
    ("HF", 15_000, mhz(64)),
];

/// Steps (10 Hz units): AM 1, 5, 9, 10 kHz; USB/LSB/CW 10, 50, 100 Hz, 1 kHz; FM 5, 12.5 kHz.
const STEPS_AM: [u32; 4] = [100, 500, 900, 1000];
const STEPS_SSB: [u32; 4] = [1, 5, 10, 100];
const STEPS_FM: [u32; 2] = [500, 1250];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hf {
    /// Index into the mode's steps.
    pub step: u8,
    /// The SSB patch as last reported (PATCH_NONE: not installed).
    pub patch: u8,
}

/// MENU 0 3 5 (HF-1); refused while transmitting or repeating (HF-2).
pub fn open(app: &mut App) -> Screen {
    if app.radio.keyed.is_some() || app.radio.mode == Mode::Xb {
        app.show_toast(b"HF mode", if app.radio.mode == Mode::Xb { b"stop the repeater first" } else { b"not while transmitting" }, false);
        return app.screen;
    }
    app.start_mode(Mode::Hf);
    app.out.push(RadioCmd::Hf { freq: app.radio.hf, mode: app.radio.hf_mode, on: true });
    // AM: 9 kHz, 10 kHz in the Americas; SSB and CW 100 Hz.
    Screen::Hf(Hf { step: default_step(app), patch: 0 })
}

fn default_step(app: &App) -> u8 {
    match app.radio.hf_mode {
        0 => if matches!(app.radio.region, Some(ferrite_core::txplan::Region::Us | ferrite_core::txplan::Region::Canada)) { 3 } else { 2 },
        4 => 0,
        _ => 2,
    }
}

fn steps(mode: u8) -> &'static [u32] {
    match mode {
        0 => &STEPS_AM,
        4 => &STEPS_FM,
        _ => &STEPS_SSB,
    }
}

/// The SI4732 takes USB, LSB, CW and everything below 18 MHz; chip 1 the rest (HF-10).
pub fn on_si4732(f: Freq10, mode: u8) -> bool {
    f < mhz(18) || matches!(mode, 1..=3)
}

fn band_index(f: Freq10) -> usize {
    BANDS.iter().position(|b| f >= b.1 && f < b.2).unwrap_or(BANDS.len() - 1)
}

fn retune(app: &mut App, f: Freq10) {
    app.radio.hf = f.clamp(15_000, mhz(64) - 1);
    app.out.push(RadioCmd::Hf { freq: app.radio.hf, mode: app.radio.hf_mode, on: true });
}

/// PTT in HF mode (HF-23): transmit on the HF frequency without going home, or refuse (HF-24).
pub fn transmit(app: &mut App) {
    if on_si4732(app.radio.hf, app.radio.hf_mode) {
        return transmit::refuse(app, HF_SLOT, Refusal::ReceiveOnlyHere);
    }
    match transmit::refusal(app, HF_SLOT) {
        Some(r) => transmit::refuse(app, HF_SLOT, r),
        None => transmit::key(app, HF_SLOT),
    }
}

fn patch(live: &ferrite_core::msg::LiveAll) -> u8 {
    match live.mode {
        ModeLive::Hf { loading, .. } => loading,
        _ => 0,
    }
}

impl View for Hf {
    fn name(self) -> &'static str {
        "HF mode"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"HF"); // HF-5
    }

    fn key(self, app: &mut App, e: KeyEvent, s: &impl ChannelStore) {
        use Press::*;
        if (e.key, e.press) == (Key::Exit, Short) {
            app.stop_mode(); // HF-4; during the load it cancels (HF-3)
            return app.go_home();
        }
        let big = app.keys.is_held(Key::Side(2));
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => {
                if big {
                    app.keys.used(Key::Side(2));
                }
                self.knob(app, if e.key == Key::Up { 1 } else { -1 }, s)
            }
            (Key::Left | Key::Right, Short | Repeat) => {
                let i = band_index(app.radio.hf) as i32 + if e.key == Key::Right { 1 } else { -1 };
                let b = BANDS[i.rem_euclid(BANDS.len() as i32 - 1) as usize];
                retune(app, b.1);
            }
            (Key::Star, Short) => {
                // Without the patch, USB, LSB and CW are hidden (HF-20).
                let ssb = app.radio.hf_mode;
                let mut m = (ssb + 1) % MODES.len() as u8;
                if self.patch == PATCH_NONE {
                    while matches!(m, 1..=3) {
                        m = (m + 1) % MODES.len() as u8;
                    }
                }
                app.radio.hf_mode = m;
                app.screen = Screen::Hf(Hf { step: default_step(app), patch: 0 });
                retune(app, app.radio.hf);
            }
            (Key::Hash, Short) => app.open(Dest::HfRecent),
            (Key::Menu, Short) => app.show_toast(b"HF options", b"step, AGC, BFO: next", false),
            (Key::Digit(0), Short) => app.open(Dest::Advanced),
            (Key::Digit(_), Short) => app.show_toast(b"Frequency entry", b"comes with entry", false),
            (Key::Slot | Key::VfoMem | Key::Side(2), Short) => app.show_toast(b"Not in HF mode", b"BACK leaves it", false),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let big = app.keys.is_held(Key::Side(2));
        if big {
            app.keys.used(Key::Side(2));
        }
        let st = steps(app.radio.hf_mode);
        let step = if big { mhz(1) } else { st[self.step as usize % st.len()] };
        retune(app, (app.radio.hf as i32 + d * step as i32).max(0) as Freq10);
    }

    fn ptt(self, app: &mut App, down: bool) {
        if down {
            transmit(app);
        }
    }

    fn tick(self, app: &mut App, live: &ferrite_core::msg::LiveAll) {
        let p = patch(live);
        if p != self.patch {
            app.screen = Screen::Hf(Hf { patch: p, ..self });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let p = patch(cx.live);
        if p <= 100 {
            // HF-3: the patch streams in; BACK cancels.
            if app.dirty(1, hash(&(0u8, p))) {
                lcd.fill(0, 21, 240, 279, BG);
                w::field(lcd, 8, 224, 110, 128, 124, Align::Center, b"Loading the SSB patch", &UI_12B, TEXT, BG);
                parts::bar(lcd, 30, 140, 180, 8, p as u32, 100, BLUE, BG);
                let mut b = Buf::<8>::new();
                let _ = uwrite!(b, "{}%", p);
                w::field(lcd, 8, 224, 154, 172, 168, Align::Center, b.as_bytes(), &UI_12, SUB, BG);
                w::field(lcd, 8, 224, 186, 204, 200, Align::Center, b"This takes a while.", &UI_12, DIM, BG);
                w::field(lcd, 8, 224, 202, 220, 216, Align::Center, b"Watch, scan and APRS pause.", &UI_12, DIM, BG);
            }
            return;
        }
        let (f, mode) = (app.radio.hf, app.radio.hf_mode);
        let (rssi, snr, si) = match cx.live.mode {
            ModeLive::Hf { rssi, snr, si4732, .. } => (rssi, snr, si4732),
            _ => (-127, 0, on_si4732(f, mode)),
        };
        let band = BANDS[band_index(f)].0;
        if app.dirty(1, hash(&(1u8, band))) {
            lcd.fill(0, 21, 240, 279, BG);
            app.drawn.hash[2..10].fill(0);
            w::field(lcd, 8, 150, 22, 40, 35, Align::Left, band.as_bytes(), &UI_12B, TEXT, BG);
        }
        if app.dirty(2, hash(&(mode, si))) {
            let rx: &[u8] = if si { b"SI4732" } else { b"chip 1" };
            lcd.fill(150, 22, 90, 18, BG);
            let x = 232 - w::tag(lcd, 232, 23, MODES[mode as usize % MODES.len()].as_bytes(), BLUE, BG) - 6;
            w::field(lcd, 150, x - 150, 22, 40, 35, Align::Right, rx, &UI_12, SUB, BG);
        }
        if app.dirty(3, hash(&f)) {
            w::freq(lcd, 8, 232, 88, f, &DIG_46, &DIG_28, TEXT, SUB, BG);
        }
        // Signal: SNR on the SI4732, the S-meter on chip 1 (HF-15).
        let lit = w::meter_level(rssi);
        if app.dirty(4, hash(&(lit, snr, si))) {
            parts::segs(lcd, 8, 104, 16, (lit as u32 * 16 / 20) as u8, 8, 10, 8, GREEN, BG);
            let mut b = Buf::<16>::new();
            if si {
                let _ = uwrite!(b, "SNR {}", snr);
            } else {
                b.set(w::s_text(rssi).as_bytes());
            }
            w::field(lcd, 168, 64, 98, 116, 112, Align::Right, b.as_bytes(), &UI_12B, TEXT, BG);
            lcd.fill(0, 122, 240, 1, LINE);
        }
        let st = steps(mode);
        let step = st[self.step as usize % st.len()];
        let mut sb = Buf::<16>::new();
        if step >= 100 {
            let _ = uwrite!(sb, "{} kHz", step / 100);
        } else {
            let _ = uwrite!(sb, "{}0 Hz", step);
        }
        let bw: &[u8] = if si { if mode == 0 { b"4 kHz" } else { b"2.4 kHz" } } else { b"Narrow" };
        let bfo: &[u8] = if matches!(mode, 1..=3) { b"+0 Hz" } else { b"-" };
        let patch: &[u8] = if self.patch == PATCH_NONE { b"Not installed" } else { b"Loaded" };
        let rows: [(&[u8], &[u8]); 5] = [(b"Step", sb.as_bytes()), (b"Bandwidth", bw), (b"BFO", bfo), (b"AGC", b"Auto"), (b"SSB patch", patch)];
        for (i, (l, v)) in rows.iter().enumerate() {
            app.kv_row(lcd, 4 + i, 126 + 20 * i as i32, 20, l, v, TEXT, false);
        }
        // Transmit here: chip 1, 12 m and 10 m where the region allows (HF-22).
        let tx: &[u8] = if on_si4732(f, mode) { b"Receive only here" } else if transmit::refusal(app, HF_SLOT).is_none() { b"PTT transmits here" } else { b"Transmit not allowed here" };
        if app.dirty(10, hash(&tx)) {
            w::field(lcd, 8, 224, 236, 254, 249, Align::Left, tx, &UI_12, SUB, BG);
            w::field(lcd, 8, 224, 254, 272, 267, Align::Left, b"Watch, scan and APRS are paused.", &UI_12, DIM, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_LR, "Band"), ("*", "Mode"), ("#", "Recent"), ("BACK", "Home")])
    }
}
