//! HF mode (B12, B13; MENU 0 3 5 only; manual chapter 20a, HF-1 to HF-30) and its recent list (C18, # in HF
//! mode; HF-28). Everything below 64 MHz on one screen: the SI4732 (AM; USB, LSB and CW with the SSB patch) and
//! BK4829 chip 1 through the relay (18-64 MHz, FM and AM). While it is open, or under the screen in view (its
//! options, the full menu from it), everything else pauses (HF-6): [`follow`] sends `HfMode(true)` on the way
//! in and `HfMode(false)` on the way out, and the status bar shows `HF` without TW/DW/SCN and with the
//! alerts-paused bell when SAME is on (HF-5, SBAR-18).
//!
//! Keys (HF-12, HF-13): knob or ▲▼ tune by the step (SIDE2 + knob: 1 MHz), ◀▶ band, * mode (a point while
//! typing), digits a frequency (kHz, or MHz with a point; OK enters), # the recent list, MENU the options, BACK
//! home. SLOT, V/M and SIDE2 alone do nothing here. PTT transmits on the HF frequency without going home
//! (HF-23), and refuses on the SI4732's frequencies and modes (HF-24).

use super::groups::agc_text;
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::ON_OFF;
use crate::{App, Screen};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_core::msg::{BroadcastCmd, LiveAll, RadioCmd};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};
use ufmt::uwrite;

/// Modes, as [`ferrite_core::msg::Param::Mode`] numbers, in the order * cycles them.
const FM: u8 = 0;
const AM: u8 = 2;
const USB: u8 = 3;
const LSB: u8 = 4;
const CW: u8 = 5;
const MODE_ORDER: [u8; 5] = [AM, USB, LSB, CW, FM];

fn mode_name(m: u8) -> &'static str {
    match m {
        FM => "FM",
        USB => "USB",
        LSB => "LSB",
        CW => "CW",
        _ => "AM",
    }
}

/// Chip 1 takes over from 18 MHz (HF-9, HF-10).
const CHIP1_FROM: Freq10 = mhz(18);
const TOP: Freq10 = mhz(64);
const BOTTOM: Freq10 = 15_000; // 150 kHz

/// The ◀▶ band list (open in the manual): name, start, end, where it starts, its mode.
const BANDS: [(&str, Freq10, Freq10, Freq10, u8); 19] = [
    ("LW", 15_300, 27_900, 19_800, AM),
    ("MW", 53_000, 171_000, 90_900, AM),
    ("160 m", 180_000, 200_000, 184_000, LSB),
    ("80 m", 350_000, 400_000, 370_000, LSB),
    ("49 m", 590_000, 620_000, 597_000, AM),
    ("40 m", 700_000, 720_000, 707_400, LSB),
    ("41 m", 720_000, 745_000, 730_000, AM),
    ("31 m", 940_000, 990_000, 958_000, AM),
    ("30 m", 1_010_000, 1_015_000, 1_013_600, CW),
    ("25 m", 1_160_000, 1_210_000, 1_177_000, AM),
    ("20 m", 1_400_000, 1_435_000, 1_407_400, USB),
    ("19 m", 1_510_000, 1_580_000, 1_540_000, AM),
    ("17 m", 1_806_800, 1_816_800, 1_810_000, USB),
    ("15 m", 2_100_000, 2_145_000, 2_107_400, USB),
    ("13 m", 2_145_000, 2_185_000, 2_150_000, AM),
    ("12 m", 2_489_000, 2_499_000, 2_491_500, USB),
    ("CB", 2_696_500, 2_740_500, 2_718_500, AM),
    ("10 m", 2_800_000, 2_970_000, 2_960_000, FM),
    ("6 m", 5_000_000, 5_400_000, 5_012_500, USB),
];

/// Steps in 10 Hz: AM 1, 5, 9, 10 kHz; USB/LSB/CW 10, 50, 100 Hz, 1 kHz; chip 1 FM the home steps.
const STEPS_AM: [u32; 4] = [100, 500, 900, 1_000];
const STEPS_SSB: [u32; 4] = [1, 5, 10, 100];
const STEPS_FM: [u32; 4] = [500, 1_000, 1_250, 2_500];
const STEP_NAMES: [[&str; 4]; 3] = [
    ["1 kHz", "5 kHz", "9 kHz", "10 kHz"],
    ["10 Hz", "50 Hz", "100 Hz", "1 kHz"],
    ["5 kHz", "10 kHz", "12.5 kHz", "25 kHz"],
];
/// Bandwidths: the SI4732's AM and SSB filters; chip 1 wide or narrow.
const BW_AM: [&str; 4] = ["6 kHz", "4 kHz", "3 kHz", "2 kHz"];
const BW_SSB: [&str; 4] = ["3 kHz", "2.2 kHz", "1.2 kHz", "0.5 kHz"];
const BW_FM: [&str; 2] = ["Wide", "Narrow"];
/// Fine tune (BFO), Hz.
const BFO_MAX: i16 = 2_000;
const BFO_STEP: i16 = 25;
/// A frequency counts as used after 10 s (HF-28).
const USED_MS: u32 = 10_000;
pub const RECENT: usize = 10;

/// HF mode's VFO, options and recent list (HF-27 to HF-29). Lives with the settings; written to its own SPI
/// sector (0x084000) on leaving, once the Storage task does that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HfMem {
    pub freq: Freq10,
    pub mode: u8,
    /// Index into the receiver's step and bandwidth lists.
    pub step: u8,
    pub bw: u8,
    pub agc: u8,
    pub lna: bool,
    /// Hz, in USB, LSB and CW.
    pub bfo: i16,
    /// Last 10 frequencies used, newest first: frequency | mode << 28 (0 = empty).
    pub recent: [u32; RECENT],
    /// When the frequency was tuned, and whether it is in the recent list yet.
    pub tuned_at: u32,
    pub counted: bool,
    /// The SSB patch is installed (as last drawn: the keys need it, the Device is the draw's).
    pub patch: bool,
    /// A frequency being typed, as a number (the screen state holds its length and point).
    pub entry: u32,
}

impl HfMem {
    pub const DEFAULT: HfMem = HfMem {
        freq: 958_000,
        mode: AM,
        step: 2,
        bw: 0,
        agc: 0,
        lna: false,
        bfo: 0,
        recent: [0; RECENT],
        tuned_at: 0,
        counted: false,
        patch: false,
        entry: 0,
    };

    fn ssb(&self) -> bool {
        matches!(self.mode, USB | LSB | CW)
    }

    /// 0 SI4732 AM, 1 SI4732 SSB/CW, 2 chip 1 (HF-10).
    fn rx(&self) -> usize {
        if self.ssb() {
            1
        } else if self.freq >= CHIP1_FROM && self.mode == FM {
            2
        } else {
            0
        }
    }

    fn chip1(&self) -> bool {
        self.rx() == 2
    }

    fn step(&self) -> u32 {
        [STEPS_AM, STEPS_SSB, STEPS_FM][self.rx()][self.step as usize % 4]
    }

    fn bw_text(&self) -> &'static str {
        match self.rx() {
            0 => BW_AM[self.bw as usize % 4],
            1 => BW_SSB[self.bw as usize % 4],
            _ => BW_FM[self.bw as usize % 2],
        }
    }

    fn band(&self) -> Option<usize> {
        BANDS.iter().position(|b| (b.1..=b.2).contains(&self.freq))
    }

    /// A mode allowed here: FM only on chip 1's range; USB, LSB, CW only with the patch (HF-20).
    fn allowed(&self, m: u8) -> bool {
        match m {
            FM => self.freq >= CHIP1_FROM,
            USB | LSB | CW => self.patch,
            _ => true,
        }
    }

    /// Transmit allowed here: chip 1, FM or AM, 18-64 MHz (HF-22, HF-24). Region and Extend TX are the
    /// Radio task's checks (chapter 15).
    fn can_tx(&self) -> bool {
        self.freq >= CHIP1_FROM && matches!(self.mode, FM | AM)
    }
}

/// Which receiver mode the screen stack holds: 2 HF mode, 1 FM broadcast, 0 neither.
fn listen_in_stack(app: &App) -> u8 {
    if app.screen == Screen::Home {
        return 0; // home: nothing under it counts (a screen may go home without clearing BACK)
    }
    let mut m = 0;
    for s in core::iter::once(Some(app.screen)).chain(app.back.iter().copied()).flatten() {
        m = m.max(match s {
            Screen::Hf(_) | Screen::HfRecent(_) => 2,
            Screen::Fm(_) => 1,
            _ => 0,
        });
    }
    m
}

/// In HF mode now (the status bar, the Status page).
pub(crate) fn in_hf(app: &App) -> bool {
    app.set.listen == 2
}

/// Starts or stops FM broadcast and HF mode as the screens come and go (called on every tick): leaving resumes
/// what was paused (HF-4, HF-8), and the SI4732 is muted whenever it isn't heard (BCR-6).
pub(crate) fn follow(app: &mut App) {
    let want = listen_in_stack(app);
    if want == app.set.listen {
        return;
    }
    leave_listen(app);
    match want {
        1 => app.out.push(RadioCmd::Broadcast(BroadcastCmd::Fm(app.set.fm_freq))),
        2 => {
            app.out.push(RadioCmd::HfMode(true));
            send(app);
        }
        _ => {}
    }
    app.set.listen = want;
}

/// Stops the broadcast or HF mode now (before PTT keys up: BCR-16, HF-25).
pub(crate) fn leave_listen(app: &mut App) {
    match app.set.listen {
        1 => app.out.push(RadioCmd::Broadcast(BroadcastCmd::Off)),
        2 => app.out.push(RadioCmd::HfMode(false)),
        _ => {}
    }
    app.set.listen = 0;
}

fn send(app: &mut App) {
    let h = &mut app.set.hf;
    (h.tuned_at, h.counted) = (app.now, false);
    app.out.push(RadioCmd::HfTune { freq: h.freq, mode: h.mode });
}

fn tune_to(app: &mut App, f: Freq10, mode: Option<u8>) {
    let h = &mut app.set.hf;
    h.freq = f.clamp(BOTTOM, TOP - 1);
    if let Some(m) = mode {
        h.mode = m;
    }
    if !h.allowed(h.mode) {
        h.mode = if h.freq >= CHIP1_FROM { FM } else { AM };
    }
    send(app);
}

/// Adds the current frequency to the recent list: newest first, no duplicates (HF-28).
fn remember(h: &mut HfMem) {
    let e = h.freq | (h.mode as u32) << 28;
    let at = h.recent.iter().position(|&r| r & 0x0FFF_FFFF == h.freq).unwrap_or(RECENT - 1);
    h.recent.copy_within(0..at, 1);
    h.recent[0] = e;
    h.counted = true;
}

/// The options (HF-14): name and help.
const OPTIONS: [(&str, &str); 6] = [
    ("Step", "Knob step: AM 1-10 kHz, SSB 10 Hz-1 kHz."),
    ("Bandwidth", "The receiver's filter."),
    ("AGC", "Automatic gain, or a fixed gain."),
    ("LNA", "The preamp in the HF input."),
    ("Fine tune", "BFO offset in USB, LSB and CW."),
    ("Recent list", "The last 10 frequencies. # clears."),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hf {
    /// The SSB patch has finished loading (or there is none): HF-3.
    pub loaded: bool,
    /// The options page and its row.
    pub menu: bool,
    pub row: u8,
    /// Typing a frequency ([`HfMem::entry`]): how many digits, and the digits after the point + 1 (0: no point).
    pub n: u8,
    pub point: u8,
}

impl Hf {
    pub fn new() -> Self {
        Hf { loaded: false, menu: false, row: 0, n: 0, point: 0 }
    }

    fn put(self, app: &mut App) {
        app.screen = Screen::Hf(self);
    }

    /// The typed frequency: kHz without a point, MHz with one.
    fn typed(self, entry: u32) -> Option<Freq10> {
        if self.n == 0 {
            return None;
        }
        let v = entry as u64;
        let f = if self.point == 0 {
            v * 100
        } else {
            let dec = (self.point - 1) as u32;
            v * 100_000 / 10u64.pow(dec)
        };
        (f >= BOTTOM as u64 && f < TOP as u64).then_some(f as Freq10)
    }

    fn option(self, app: &mut App, d: i32) {
        let h = &mut app.set.hf;
        let cyc = |v: u8, n: i32| (v as i32 + d).rem_euclid(n) as u8;
        match self.row {
            0 => h.step = cyc(h.step, 4),
            1 => h.bw = cyc(h.bw, if h.rx() == 2 { 2 } else { 4 }),
            2 => h.agc = cyc(h.agc, 10),
            3 => h.lna = !h.lna,
            4 if h.ssb() => h.bfo = (h.bfo + d as i16 * BFO_STEP).clamp(-BFO_MAX, BFO_MAX),
            4 => app.show_toast(b"Fine tune", b"USB, LSB and CW only", false),
            _ => {}
        }
    }
}

impl View for Hf {
    fn name(self) -> &'static str {
        "HF mode"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"HF"); // HF-5
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        if !self.loaded {
            if (e.key, e.press) == (Key::Exit, Short) {
                app.go_home(); // HF-3: BACK during the load cancels and goes home
            }
            return;
        }
        if self.menu {
            match (e.key, e.press) {
                (Key::Up, Short | Repeat) => Hf { row: (self.row + 5) % 6, ..self }.put(app),
                (Key::Down, Short | Repeat) => Hf { row: (self.row + 1) % 6, ..self }.put(app),
                (Key::Left | Key::Right, Short | Repeat) => self.option(app, if e.key == Key::Right { 1 } else { -1 }),
                (Key::Digit(d @ 1..=6), Short) => Hf { row: d - 1, ..self }.put(app),
                (Key::Digit(0), Short) => app.open(Dest::Advanced),
                (Key::Menu, Short) if self.row == 5 => app.open(Dest::HfRecent),
                (Key::Menu, Short) => self.option(app, 1),
                (Key::Hash, Short) if self.row == 5 => {
                    app.set.hf.recent = [0; RECENT];
                    app.show_toast(b"Recent list", b"cleared", false);
                }
                (Key::Exit, Short) => Hf { menu: false, ..self }.put(app),
                _ => {}
            }
            return;
        }
        if self.n > 0 {
            // Typing a frequency.
            match (e.key, e.press) {
                (Key::Digit(d), Short) if self.n < 8 => {
                    let point = if self.point > 0 { self.point + 1 } else { 0 };
                    app.set.hf.entry = app.set.hf.entry * 10 + d as u32;
                    Hf { n: self.n + 1, point, ..self }.put(app);
                }
                (Key::Star, Short) if self.point == 0 => Hf { point: 1, ..self }.put(app),
                (Key::Left, Short) => Hf { n: 0, point: 0, ..self }.put(app),
                (Key::Menu, Short) => {
                    match self.typed(app.set.hf.entry) {
                        Some(f) => tune_to(app, f, None),
                        None => app.show_toast(b"HF mode", b"0.15 to 64 MHz", false),
                    }
                    Hf { n: 0, point: 0, ..self }.put(app);
                }
                (Key::Exit, Short) => Hf { n: 0, point: 0, ..self }.put(app),
                _ => {}
            }
            return;
        }
        let h = app.set.hf;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => {
                let s = h.step() as i32 * if e.key == Key::Up { 1 } else { -1 };
                tune_to(app, (h.freq as i32 + s) as Freq10, None);
            }
            (Key::Left | Key::Right, Short | Repeat) => {
                // the next band up or down from here
                let b = match (h.band(), e.key == Key::Right) {
                    (Some(i), true) => (i + 1) % BANDS.len(),
                    (Some(i), false) => (i + BANDS.len() - 1) % BANDS.len(),
                    (None, true) => BANDS.iter().position(|b| b.1 > h.freq).unwrap_or(0),
                    (None, false) => BANDS.iter().rposition(|b| b.2 < h.freq).unwrap_or(BANDS.len() - 1),
                };
                tune_to(app, BANDS[b].3, Some(BANDS[b].4));
            }
            (Key::Star, Short) => {
                // the next mode this frequency allows
                let i = MODE_ORDER.iter().position(|&m| m == h.mode).unwrap_or(0);
                let next = (1..=MODE_ORDER.len()).map(|k| MODE_ORDER[(i + k) % MODE_ORDER.len()]).find(|&m| h.allowed(m));
                if let Some(m) = next {
                    app.set.hf.step = if matches!(m, USB | LSB | CW) { 2 } else { app.set.hf.step };
                    tune_to(app, h.freq, Some(m));
                }
            }
            (Key::Digit(d), Short) => {
                app.set.hf.entry = d as u32;
                Hf { n: 1, point: 0, ..self }.put(app);
            }
            (Key::Hash, Short) => app.open(Dest::HfRecent),
            (Key::Menu, Short) => Hf { menu: true, row: 0, ..self }.put(app),
            (Key::Slot | Key::VfoMem, Short) | (Key::Side(2), Short) => app.show_toast(b"Not in HF mode", b"BACK leaves HF mode", false), // HF-13
            (Key::Exit, Short) => app.go_home(), // HF-4
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        if !self.loaded {
            return;
        }
        if self.menu {
            return self.option(app, d);
        }
        let h = app.set.hf;
        let big = app.keys.is_held(Key::Side(2));
        if big {
            app.keys.used(Key::Side(2));
        }
        let s = if big { 100_000 } else { h.step() as i32 };
        tune_to(app, (h.freq as i32 + d * s).max(BOTTOM as i32) as Freq10, None);
    }

    fn ptt(self, app: &mut App, down: bool) {
        if !down {
            return;
        }
        // HF-23: transmit on the HF frequency, on chip 1, without going home; HF-24: not on the SI4732's.
        if app.set.hf.can_tx() {
            app.open(Dest::Transmit);
        } else {
            app.show_toast(b"Receive only here", b"PTT: 18-64 MHz, FM or AM", false);
        }
    }

    fn tick(self, app: &mut App, _: &LiveAll) {
        let h = &mut app.set.hf;
        if self.loaded && !h.counted && app.now.wrapping_sub(h.tuned_at) >= USED_MS {
            remember(h);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let dev = cx.dev;
        app.set.hf.patch = dev.selftest.ssb_patch;
        let loading = dev.selftest.ssb_patch && dev.bcast.patch_pct < 100;
        if !self.loaded && !loading {
            Hf { loaded: true, ..self }.put(app);
            app.set.hf.tuned_at = app.now;
        }
        app.new_page(lcd, hash(&(self.loaded || !loading, self.menu)));
        if !self.loaded && loading {
            // B13: the patch streams in behind a bar (HF-3, HF-19).
            app.message(lcd, b"HF mode", b"Loading SSB patch", b"Streaming the patch into the SI4732 for USB, LSB and CW. It takes a while. BACK cancels.", TEXT);
            let pct = dev.bcast.patch_pct;
            if app.dirty(R_NOTE, hash(&pct)) {
                let mut t = Buf::<8>::new();
                let _ = uwrite!(t, "{} %", pct);
                w::progress(lcd, 30, 230, 180, 6, pct, BLUE);
                w::field(lcd, 10, 220, 242, 260, 256, Align::Center, t.as_bytes(), &UI_12B, TEXT, BG);
            }
            return;
        }
        let h = app.set.hf;
        let rx = h.rx();
        if self.menu {
            let mut vals = [Buf::<24>::new(); 6];
            vals[0].set(STEP_NAMES[rx][h.step as usize % 4].as_bytes());
            vals[1].set(h.bw_text().as_bytes());
            agc_text(h.agc, &mut vals[2]);
            vals[3].set(ON_OFF[h.lna as usize].as_bytes());
            if h.ssb() {
                let _ = uwrite!(vals[4], "{}{} Hz", if h.bfo > 0 { "+" } else { "" }, h.bfo);
            } else {
                vals[4].set(b"SSB only");
            }
            let n = h.recent.iter().filter(|&&r| r != 0).count();
            let _ = uwrite!(vals[5], "{} >", n);
            for (i, ((name, _), v)) in OPTIONS.iter().zip(&vals).enumerate() {
                let r = Row { num: Some(i as u8 + 1), name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: i as u8 == self.row, grey: i == 4 && !h.ssb() };
                app.list_row(lcd, i, 23 + 22 * i as i32, 22, &r);
            }
            let r = Row { num: Some(0), name: b"Full menu", value: b">", dot: false, selected: false, grey: false };
            app.list_row(lcd, 6, 23 + 22 * 6, 22, &r);
            app.help_box(lcd, 186, 46, OPTIONS[self.row as usize % 6].1.as_bytes(), b"HF MENU");
            return;
        }
        // Band and receiver (HF-15).
        let band = h.band().map_or("", |b| BANDS[b].0);
        let chip1 = h.chip1();
        if app.dirty(R_HEAD, hash(&(band, chip1))) {
            w::field(lcd, 10, 120, 24, 42, 38, Align::Left, band.as_bytes(), &UI_12B, TEXT, BG);
            lcd.fill(130, 24, 102, 18, BG);
            w::tag(lcd, 230, 25, if chip1 { b"CHIP 1".as_slice() } else { b"SI4732" }, SUB, BG);
        }
        if app.dirty(R_FREQ, hash(&h.freq)) {
            w::freq(lcd, 12, 232, 96, h.freq, &DIG_46, &DIG_28, TEXT, SUB, BG);
        }
        // Mode, step, bandwidth, BFO in USB/LSB/CW.
        let mut bfo = Buf::<16>::new();
        if h.ssb() {
            let _ = uwrite!(bfo, "BFO {}{}", if h.bfo > 0 { "+" } else { "" }, h.bfo);
        }
        let step = STEP_NAMES[rx][h.step as usize % 4];
        if app.dirty(R_INFO, hash(&(h.mode, step, h.bw_text(), bfo))) {
            lcd.fill(12, 106, 220, 17, BG); // chips leave their gaps unpainted
            w::chips(lcd, 12, 106, 220, &[mode_name(h.mode).as_bytes(), step.as_bytes(), h.bw_text().as_bytes(), bfo.as_bytes()], BG);
        }
        // Signal: SNR on the SI4732, an S-meter on chip 1 (HF-15).
        let b = dev.bcast;
        let lit = (b.rssi as u32 * w::METER_SEGS as u32 / 70).min(w::METER_SEGS as u32) as u8;
        if app.dirty(R_SUB, hash(&(b.rssi, b.snr, chip1))) {
            let mut t = Buf::<24>::new();
            if chip1 {
                let dbm = b.rssi as i16 - 127;
                t.set(w::s_text(dbm).as_bytes());
            } else {
                let _ = uwrite!(t, "SNR {} dB", b.snr);
            }
            w::meter(lcd, 12, 140, None, lit, GREEN, BG);
            w::field(lcd, 214, 18, 132, 150, 146, Align::Right, b"", &UI_12, TEXT, BG);
            w::field(lcd, 12, 220, 152, 170, 166, Align::Left, t.as_bytes(), &UI_12, SUB, BG);
        }
        // Typing a frequency.
        let mut ent = Buf::<24>::new();
        if self.n > 0 {
            let mut digits = [0u8; 10];
            let mut v = app.set.hf.entry;
            let mut k = self.n as usize;
            while k > 0 {
                k -= 1;
                digits[k] = b'0' + (v % 10) as u8;
                v /= 10;
            }
            let n = self.n as usize;
            let int = if self.point > 0 { n + 1 - self.point as usize } else { n };
            ent.push(b"Go to ");
            ent.push(&digits[..int]);
            if self.point > 0 {
                ent.push(b".");
                ent.push(&digits[int..n]);
            }
            ent.push(if self.point > 0 { b" MHz" } else { b" kHz" });
        }
        if app.dirty(R_NOTE, hash(&ent)) {
            w::field(lcd, 12, 220, 178, 196, 192, Align::Left, ent.as_bytes(), &UI_12B, AMBER, BG);
        }
        // What is paused (HF-6), and whether PTT can transmit here.
        let tx = h.can_tx();
        if app.dirty(R_HELP, hash(&(tx, h.patch))) {
            lcd.fill(0, 204, 240, 96, BG);
            lcd.fill_r(8, 206, 224, 76, 4, SURFACE);
            let mut t = Buf::<140>::new();
            t.push(b"Paused: watch, scan, APRS, decoders, jobs and beacons. ");
            t.push(if tx { b"PTT transmits here on chip 1.".as_slice() } else { b"Receive only here." });
            if !h.patch {
                t.push(b" No SSB patch: AM only.");
            }
            w::wrap(lcd, 16, 208, 209, 279, 222, 15, t.as_bytes(), &UI_12, SUB, SURFACE);
        }
    }

    fn hints(self, _: &App) -> Hints {
        if !self.loaded {
            Hints::new(&[("BACK", "Cancel")])
        } else if self.menu {
            Hints::new(&[("Knob", "Change"), ("0", "Full menu"), ("BACK", "Back")])
        } else if self.n > 0 {
            Hints::new(&[("*", "Point"), ("OK", "Go"), ("BACK", "Cancel")])
        } else {
            Hints::new(&[("*", "Mode"), ("#", "Recent"), ("MENU", "Options")])
        }
    }
}

/// The recent list (C18; # in HF mode; HF-28): the last 10 frequencies, newest first; OK tunes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HfRecent {
    pub row: u8,
}

impl View for HfRecent {
    fn name(self) -> &'static str {
        "HF recent"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"HF recent");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let n = app.set.hf.recent.iter().filter(|&&r| r != 0).count().max(1) as u8;
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => app.screen = Screen::HfRecent(HfRecent { row: (self.row + n - 1) % n }),
            (Key::Down, Short | Repeat) => app.screen = Screen::HfRecent(HfRecent { row: (self.row + 1) % n }),
            (Key::Menu, Short) => {
                let r = app.set.hf.recent[self.row as usize % RECENT];
                app.back();
                if r != 0 {
                    tune_to(app, r & 0x0FFF_FFFF, Some((r >> 28) as u8));
                }
            }
            (Key::Hash, Short) => {
                app.set.hf.recent = [0; RECENT];
                app.screen = Screen::HfRecent(HfRecent { row: 0 });
            }
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        Hf::new().ptt(app, down);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let rec = app.set.hf.recent;
        for (i, &r) in rec.iter().enumerate() {
            let mut name = Buf::<16>::new();
            let mut val = Buf::<24>::new();
            if r != 0 {
                let mut fb = [0u8; 16];
                let n = ferrite_core::freq::format(r & 0x0FFF_FFFF, &mut fb);
                name.set(&fb[..n]);
                val.set(mode_name((r >> 28) as u8).as_bytes());
            } else if i == 0 {
                name.set(b"Nothing yet");
            }
            let row = Row { num: (r != 0).then_some(((i + 1) % 10) as u8), name: name.as_bytes(), value: val.as_bytes(), dot: false, selected: r != 0 && i as u8 == self.row, grey: r == 0 };
            app.list_row(lcd, i, 23 + 22 * i as i32, 22, &row);
        }
        app.help_box(lcd, 248, 46, b"A frequency counts after 10 s on it or a transmission. # clears.", b"HF #");
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Tune"), ("#", "Clear"), ("BACK", "Back")])
    }
}
