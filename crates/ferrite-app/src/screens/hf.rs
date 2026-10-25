//! HF mode (B12, B13; MENU 0 3 4 only; manual chapter 20a, HF-1 to HF-30) and its recent list (C18, # in HF
//! mode; HF-28). Everything below 64 MHz on one screen: the SI4732 (AM; USB, LSB and CW with the SSB patch) and
//! BK4829 chip 1 through the relay (18-64 MHz, FM and AM). While it is open, or under the screen in view (its
//! options, the full menu from it), everything else pauses (HF-6): [`open`] starts the exclusive mode
//! ([`super::activity::Mode::Hf`]) and leaving home stops it, and the status bar shows `HF` without TW/DW/SCN
//! and with the alerts-paused bell when SAME is on (HF-5, SBAR-18).
//!
//! Keys (HF-12, HF-13): knob or ▲▼ tune by the step (SIDE2 + knob: 1 MHz), ◀▶ band, * mode (a point while
//! typing, then OK once a digit follows it, VFO-24), digits a frequency (kHz, or MHz with a point; OK enters), # the recent list, hold MENU the options,
//! BACK leaves HF mode, one level back; hold 9 saves it as a channel (CHAN-26, KEY-23). SLOT, V/M (held too)
//! and SIDE2 alone do nothing here. PTT transmits on the HF frequency without going home
//! (HF-23), and refuses on the SI4732's frequencies and modes (HF-24).

use super::activity::Mode;
use super::groups::agc_text;
use super::transmit::{self, HF_SLOT};
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_core::msg::{LiveAll, ModeLive, RadioCmd};
use ferrite_core::txplan::Refusal;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};
use ufmt::uwrite;

/// Modes, as [`ferrite_core::msg::Param::Mode`] numbers, in the order * cycles them.
pub const FM: u8 = 0;
const AM: u8 = 2;
const USB: u8 = 3;
const LSB: u8 = 4;
const CW: u8 = 5;
const MODE_ORDER: [u8; 5] = [AM, USB, LSB, CW, FM];

pub fn mode_name(m: u8) -> &'static str {
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

/// The HF bands, low to high: name, start, end, where it starts, its mode. ◀▶ walks one of two lists picked by
/// the mode (HF-32): broadcast or amateur ([`HAM`]).
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

/// HF-32: the amateur list (CB with it); the rest are the broadcast list.
const HAM: [bool; BANDS.len()] = [
    false, false, true, true, false, true, false, false, true, false, true, false, true, true, false, true, true, true, true,
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
/// Fine tune (BFO) steps, Hz (SSB, not built yet: HF-35).
const BFO_STEP: i16 = 25;
/// A frequency counts as used after 10 s (HF-28).
const USED_MS: u32 = 10_000;
pub const RECENT: usize = 10;

/// HF mode's VFO, options and recent list (HF-27 to HF-29). Lives with the settings; written to its own SPI
/// sector (0x084000) on leaving, once the Storage task does that. What changes while tuning (when the
/// frequency was tuned, a frequency being typed) is the App's ([`super::activity::RadioUi`]).
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
    /// HF-27: each band's own VFO, as it was left: frequency (23 bits, 10 Hz) | mode << 23 | step << 26 |
    /// bandwidth << 28; 0 = never visited (the band's start). Its fine tune in 25 Hz units beside it.
    pub bands: [u32; BANDS.len()],
    pub band_bfo: [i8; BANDS.len()],
}

impl HfMem {
    pub const DEFAULT: HfMem = HfMem {
        freq: 958_000,
        mode: AM,
        step: 1, // 5 kHz on 31 m (HF-34)
        bw: 0,
        agc: 0,
        lna: false,
        bfo: 0,
        recent: [0; RECENT],
        bands: [0; BANDS.len()],
        band_bfo: [0; BANDS.len()],
    };

    /// HF-27: keeps this VFO as its band's (when it is in one).
    fn keep_band(&mut self) {
        if let Some(b) = self.band() {
            self.bands[b] = self.freq | (self.mode as u32 & 7) << 23 | (self.step as u32 & 3) << 26 | (self.bw as u32 & 3) << 28;
            self.band_bfo[b] = (self.bfo / BFO_STEP) as i8;
        }
    }

    /// HF-27: the band's VFO as it was left, or None the first time.
    fn band_vfo(&self, b: usize) -> Option<(Freq10, u8, u8, u8, i16)> {
        let v = self.bands[b];
        (v != 0).then(|| (v & 0x7F_FFFF, (v >> 23 & 7) as u8, (v >> 26 & 3) as u8, (v >> 28 & 3) as u8, self.band_bfo[b] as i16 * BFO_STEP))
    }

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

    /// HF-35: the SI4732's AM_CHANNEL_FILTER for the bandwidth ([`BW_AM`] is in its order: 6, 4, 3, 2 kHz).
    pub(crate) fn am_filter(&self) -> u8 {
        self.bw % 4
    }

    fn bw_text(&self) -> &'static str {
        match self.rx() {
            0 => BW_AM[self.bw as usize % 4],
            1 => BW_SSB[self.bw as usize % 4],
            _ => BW_FM[self.bw as usize % 2],
        }
    }

    /// HF-32: ◀▶ walks the amateur list in USB, LSB, CW and FM, and in AM inside an amateur band or CB;
    /// otherwise (AM) the broadcast list.
    fn ham_list(&self) -> bool {
        self.mode != AM || self.band().is_some_and(|b| HAM[b])
    }

    /// HF-32: the next band in the mode's list up or down from here, wrapping.
    fn next_band(&self, up: bool) -> usize {
        let ham = self.ham_list();
        let list = || (0..BANDS.len()).filter(move |&i| HAM[i] == ham);
        let cur = self.band().filter(|&b| HAM[b] == ham);
        let found = match (cur, up) {
            (Some(i), true) => list().find(|&b| b > i),
            (Some(i), false) => list().rfind(|&b| b < i),
            (None, true) => list().find(|&b| BANDS[b].1 > self.freq),
            (None, false) => list().rfind(|&b| BANDS[b].2 < self.freq),
        };
        found.unwrap_or_else(|| if up { list().next().unwrap_or(0) } else { list().next_back().unwrap_or(0) })
    }

    fn band(&self) -> Option<usize> {
        BANDS.iter().position(|b| (b.1..=b.2).contains(&self.freq))
    }

    /// A mode allowed here: FM only on chip 1's range; USB, LSB, CW only with the patch (HF-20).
    pub(crate) fn allowed(&self, m: u8, patch: bool) -> bool {
        match m {
            FM => self.freq >= CHIP1_FROM,
            USB | LSB | CW => patch,
            _ => true,
        }
    }

    /// Transmit allowed here: chip 1, FM or AM, 18-64 MHz (HF-22, HF-24). Region and Extend TX are the
    /// Radio task's checks (chapter 15).
    fn can_tx(&self) -> bool {
        self.freq >= CHIP1_FROM && matches!(self.mode, FM | AM)
    }
}

/// An HF channel's mode is one HF mode has at its frequency (CHAN-26): AM, USB, LSB and CW anywhere, FM from
/// 18 MHz (chip 1).
pub fn channel_mode_ok(f: Freq10, m: u8) -> bool {
    matches!(m, AM | USB | LSB | CW) || m == FM && f >= CHIP1_FROM
}

/// CHAN-26: an HF channel recalled (by number, from the browser, the home channel) opens HF mode on its
/// frequency and mode; without the SSB patch, USB, LSB and CW come up as AM.
pub fn recall(app: &mut App, c: &crate::chan::Channel) {
    let patch = app.radio.ssb_patch;
    let m = if app.set.hf.allowed(c.cfg.mode, patch) && channel_mode_ok(c.rx, c.cfg.mode) { c.cfg.mode } else { AM };
    if in_hf(app) {
        tune_to(app, c.rx, Some(m));
    } else {
        app.set.hf.freq = c.rx.clamp(BOTTOM, TOP - 1);
        app.set.hf.mode = m;
        app.go_home();
        app.open(Dest::HfMode);
    }
    let mut l = Buf::<32>::new();
    crate::slot::chan_label(c.num, &mut l);
    l.push(b" ");
    l.push(c.name());
    app.show_toast(b"HF mode", l.as_bytes(), false);
}

/// In HF mode now (the status bar, the Status page).
pub(crate) fn in_hf(app: &App) -> bool {
    app.radio.mode == Mode::Hf
}

/// MENU 0 3 4 (HF-1): HF mode starts, everything else pauses; refused while transmitting or repeating (HF-2).
pub fn open(app: &mut App) -> Screen {
    if app.radio.keyed.is_some() || app.radio.mode == Mode::Xb {
        app.show_toast(b"HF mode", if app.radio.mode == Mode::Xb { b"stop the repeater first" } else { b"not while transmitting" }, false);
        return app.screen;
    }
    Screen::Hf(Hf::new()) // MODE-2: the reconciler starts HF mode
}

/// PTT in HF mode (HF-23): transmit on the HF frequency on chip 1 without going home, or refuse on the
/// SI4732's frequencies and modes (HF-24).
pub fn transmit(app: &mut App) {
    if !app.set.hf.can_tx() {
        return transmit::refuse(app, HF_SLOT, Refusal::ReceiveOnlyHere);
    }
    match transmit::refusal(app, HF_SLOT) {
        Some(r) => transmit::refuse(app, HF_SLOT, r),
        None => transmit::key(app, HF_SLOT),
    }
}

/// The SSB patch load (0-100; 255 loaded, 254 none), as the Radio task reports it.
fn loading(live: &LiveAll) -> u8 {
    match live.mode {
        ModeLive::Hf { loading, .. } => loading,
        _ => 0,
    }
}

/// A seek that has found nothing by this gives up (the receiver stays where it stopped). Longer than the Radio
/// task's own limit (15 s, then cancelled where it got to, which it reports): giving up first, as at 15 s, missed
/// that report, so the screen kept the old frequency while the receiver sat elsewhere, and the next seek took
/// the stale report as its landing (HF-31 on the radio, Oct 15, 2026: down from 9.4 "landed" on 10.94 MHz).
const SEEK_MS: u32 = 17_000;

/// HF-31: seek is the SI4732's, in AM: below 30 MHz, not in SSB or CW (the patch has no seek).
fn seek(app: &mut App, up: bool) {
    let h = app.set.hf;
    if h.mode != AM || h.freq >= mhz(30) {
        return app.show_toast(b"Seek", b"AM below 30 MHz only", false);
    }
    app.radio.hf_seeking = true;
    app.radio.hf_tuned_at = app.now;
    app.out.push(RadioCmd::HfSeek { up });
}

pub(crate) fn send(app: &mut App) {
    (app.radio.hf_tuned_at, app.radio.hf_counted) = (app.now, false);
    let h = &app.set.hf;
    app.out.push(RadioCmd::Hf { freq: h.freq, mode: h.mode, on: true });
}

fn tune_to(app: &mut App, f: Freq10, mode: Option<u8>) {
    tune_vfo(app, f, mode, None)
}

/// A band's step the first time it is tuned (HF-34): MW 10 kHz in the Americas (region 0 US, 1 Canada) and
/// 9 kHz elsewhere, LW 9 kHz, shortwave broadcast 5 kHz; SSB and CW bands 100 Hz (index 2).
fn first_step(b: usize, region: u8) -> u8 {
    match BANDS[b].0 {
        "MW" if region <= 1 => 3,
        "LW" | "MW" => 2,
        _ if BANDS[b].4 == AM => 1,
        _ => 2,
    }
}

/// [`tune_to`], with a band's remembered step, bandwidth and fine tune (HF-27).
fn tune_vfo(app: &mut App, f: Freq10, mode: Option<u8>, rest: Option<(u8, u8, i16)>) {
    let h = &mut app.set.hf;
    h.keep_band(); // HF-27: the band being left keeps this VFO
    if let Some((step, bw, bfo)) = rest {
        (h.step, h.bw, h.bfo) = (step, bw, bfo);
    }
    h.freq = f.clamp(BOTTOM, TOP - 1);
    if let Some(m) = mode {
        h.mode = m;
    }
    if !h.allowed(h.mode, app.radio.ssb_patch) {
        h.mode = if h.freq >= CHIP1_FROM { FM } else { AM };
    }
    if app.radio.mode == Mode::Hf {
        send(app); // MODE-2: stopped, the start takes the new frequency
    }
}

/// Adds the current frequency to the recent list: newest first, no duplicates (HF-28).
fn remember(h: &mut HfMem) {
    let e = h.freq | (h.mode as u32) << 28;
    let at = h.recent.iter().position(|&r| r & 0x0FFF_FFFF == h.freq).unwrap_or(RECENT - 1);
    h.recent.copy_within(0..at, 1);
    h.recent[0] = e;
}

/// The recent list's entries: frequency and mode, newest first (C18 lists them).
pub fn recent(h: &HfMem) -> impl Iterator<Item = (Freq10, u8)> + '_ {
    h.recent.iter().take_while(|&&r| r != 0).map(|&r| (r & 0x0FFF_FFFF, (r >> 28) as u8))
}

/// Tunes a recent entry (C18, OK), back in HF mode.
pub fn tune_recent(app: &mut App, f: Freq10, mode: u8) {
    tune_to(app, f, Some(mode));
}

/// The options (HF-14): name and help. LNA and fine tune (BFO) are left out until SSB is built (HF-35).
const OPTIONS: [(&str, &str); 4] = [
    ("Step", "Knob step: AM 1-10 kHz."),
    ("Bandwidth", "The receiver's filter."),
    ("AGC", "Auto, or a fixed gain: 1 the most, 10 the least."),
    ("Recent list", "The last 10 frequencies. # clears."),
];
/// The recent list's row.
const ROW_RECENT: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hf {
    /// The SSB patch has finished loading (or there is none): HF-3.
    pub loaded: bool,
    /// The options page and its row.
    pub menu: bool,
    pub row: u8,
    /// Typing a frequency (`RadioUi::hf_entry`): how many digits, and the digits after the point + 1 (0: no point).
    pub n: u8,
    pub point: u8,
    /// The HF receiver's frequency (kHz: seeks land on whole kHz) as the Radio task last reported it before a
    /// seek: a seek lands when the report moves off it (HF-31), not when it differs from the screen's, which a
    /// stale report always did. Here, not in `RadioUi`, as the screen's state has room for it (App RAM).
    pub heard: u16,
}

impl Hf {
    pub fn new() -> Self {
        Hf { loaded: false, menu: false, row: 0, n: 0, point: 0, heard: 0 }
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
            2 => h.agc = cyc(h.agc, crate::settings::AGC_MAX as i32 + 1),
            _ => {}
        }
        app.send_si_opts(); // HF-35: the bandwidth and AGC reach the SI4732
    }
}

/// The options page's values (HF-14), as drawn and read back.
fn option_values(h: &HfMem) -> [Buf<24>; 4] {
    let mut vals = [Buf::<24>::new(); 4];
    vals[0].set(STEP_NAMES[h.rx()][h.step as usize % 4].as_bytes());
    vals[1].set(h.bw_text().as_bytes());
    agc_text(h.agc, &mut vals[2]);
    let n = h.recent.iter().filter(|&&r| r != 0).count();
    let _ = uwrite!(vals[3], "{} >", n);
    vals
}

/// The signal under the meter (HF-15): the S-meter on chip 1, the SNR on the SI4732.
fn signal_text(chip1: bool, rssi: i16, snr: u8) -> Buf<24> {
    let mut t = Buf::<24>::new();
    if chip1 {
        t.set(w::s_text(rssi).as_bytes());
    } else {
        let _ = uwrite!(t, "SNR {} dB", snr);
    }
    t
}

/// What HF mode shows (HF-15), for the UI readback (LINK-19): the same values its `render` draws.
pub(crate) struct Shown {
    pub loading: Option<u8>,
    pub menu: Option<(u8, [Buf<24>; 4])>,
    pub band: &'static str,
    pub rx: &'static str,
    pub mode: &'static str,
    pub step: &'static str,
    pub bw: &'static str,
    pub bfo: Buf<16>,
    pub signal: Buf<24>,
    pub entry: Buf<24>,
    pub tx: bool,
    pub seeking: bool,
}

/// The options' names, for the readback's rows.
pub(crate) fn option_names() -> impl Iterator<Item = &'static str> {
    OPTIONS.iter().map(|o| o.0)
}

impl Hf {
    /// The typed frequency as the screen shows it ("Go to 7074 kHz"), empty when nothing is typed.
    fn entry_text(self, app: &App) -> Buf<24> {
        let mut ent = Buf::<24>::new();
        if self.n > 0 {
            let mut digits = [0u8; 10];
            let mut v = app.radio.hf_entry;
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
        ent
    }

    /// The screen's values (HF-15) for the readback.
    pub(crate) fn shown(self, app: &App, live: &LiveAll) -> Shown {
        let h = app.set.hf;
        let pct = loading(live);
        let (rssi, snr) = match live.mode {
            ModeLive::Hf { rssi, snr, .. } => (rssi, snr),
            _ => (-127, 0),
        };
        let mut bfo = Buf::<16>::new();
        if h.ssb() {
            let _ = uwrite!(bfo, "BFO {}{}", if h.bfo > 0 { "+" } else { "" }, h.bfo);
        }
        Shown {
            loading: (!self.loaded && app.radio.ssb_patch && pct < 100).then_some(pct),
            menu: self.menu.then(|| (self.row, option_values(&h))),
            band: h.band().map_or("", |b| BANDS[b].0),
            rx: if h.chip1() { "CHIP 1" } else { "SI4732" },
            mode: mode_name(h.mode),
            step: STEP_NAMES[h.rx()][h.step as usize % 4],
            bw: h.bw_text(),
            bfo,
            signal: signal_text(h.chip1(), rssi, snr),
            entry: self.entry_text(app),
            tx: h.can_tx(),
            seeking: app.radio.hf_seeking,
        }
    }
}

impl View for Hf {
    /// KEY-19: ◀▶ step the HF band (and the option on its menu).
    fn own_arrows(self) -> bool {
        true
    }

    /// MENU-25: the options page, drawn in the shared options style (not while the patch loads or a frequency
    /// is being typed).
    fn options(self, app: &mut App) -> bool {
        if self.loaded && self.n == 0 {
            Hf { menu: true, row: 0, ..self }.put(app);
        }
        true
    }

    fn name(self) -> &'static str {
        "HF mode"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("HfMode.png")
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"HF"); // HF-5
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        if !self.loaded {
            if (e.key, e.press) == (Key::Exit, Short) {
                app.back(); // HF-3: BACK during the load cancels, one level back
            }
            return;
        }
        if self.menu {
            match (e.key, e.press) {
                (Key::Up, Short | Repeat) => Hf { row: (self.row + 3) % 4, ..self }.put(app),
                (Key::Down, Short | Repeat) => Hf { row: (self.row + 1) % 4, ..self }.put(app),
                (Key::Left | Key::Right, Short | Repeat) => self.option(app, if e.key == Key::Right { 1 } else { -1 }),
                (Key::Digit(d @ 1..=4), Short) => Hf { row: d - 1, ..self }.put(app),
                (Key::Digit(0), Short) => app.open(Dest::Advanced),
                (Key::Menu, Short) if self.row == ROW_RECENT => app.open(Dest::HfRecent),
                (Key::Menu, Short) => self.option(app, 1),
                (Key::Hash, Short) if self.row == ROW_RECENT => {
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
                    app.radio.hf_entry = app.radio.hf_entry * 10 + d as u32;
                    Hf { n: self.n + 1, point, ..self }.put(app);
                }
                (Key::Star, Short) if self.point == 0 => Hf { point: 1, ..self }.put(app),
                // VFO-24: \* after a digit past the point enters, as OK (7 \* 074 \*)
                (Key::Menu, Short) | (Key::Star, Short) if e.key == Key::Menu || self.point > 1 => {
                    match self.typed(app.radio.hf_entry) {
                        Some(f) => tune_to(app, f, None),
                        None => app.show_toast(b"HF mode", b"0.15 to 64 MHz", false),
                    }
                    Hf { n: 0, point: 0, ..self }.put(app);
                }
                (Key::Left, Short) | (Key::Exit, Short) => Hf { n: 0, point: 0, ..self }.put(app),
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
                // HF-32: the next band up or down in the mode's list
                let b = h.next_band(e.key == Key::Right);
                // HF-27: the band's own VFO as it was left; its start the first time.
                match app.set.hf.band_vfo(b) {
                    Some((f, m, step, bw, bfo)) => tune_vfo(app, f, Some(m), Some((step, bw, bfo))),
                    None => {
                        let h = &app.set.hf;
                        let rest = (first_step(b, app.set.region), h.bw, h.bfo);
                        tune_vfo(app, BANDS[b].3, Some(BANDS[b].4), Some(rest))
                    }
                }
            }
            (Key::Star, Short) => {
                // the next mode this frequency allows
                let i = MODE_ORDER.iter().position(|&m| m == h.mode).unwrap_or(0);
                let patch = app.radio.ssb_patch;
                let next = (1..=MODE_ORDER.len()).map(|k| MODE_ORDER[(i + k) % MODE_ORDER.len()]).find(|&m| h.allowed(m, patch));
                if let Some(m) = next {
                    app.set.hf.step = if matches!(m, USB | LSB | CW) { 2 } else { app.set.hf.step };
                    tune_to(app, h.freq, Some(m));
                }
            }
            (Key::Digit(d), Short) => {
                app.radio.hf_entry = d as u32;
                Hf { n: 1, point: 0, ..self }.put(app);
            }
            // HF-31: hold 3 seeks up, hold 1 down (nicFW's tuner keys), with the SI4732's own seek.
            (Key::Digit(d @ (1 | 3)), Long) => seek(app, d == 3),
            (Key::Hash, Short) => app.open(Dest::HfRecent),
            // CHAN-26, KEY-23: hold 9 saves the HF frequency and mode as a channel, as on a home VFO
            (Key::Digit(9), Long) => {
                app.open(Dest::SaveChannel);
                app.screen = Screen::Save(super::save::Save::hf());
            }
            // HF-13; hold V/M too (KEY-23: no zones in HF mode)
            (Key::Slot | Key::VfoMem, Short) | (Key::VfoMem, Long) | (Key::Side(2), Short) => {
                app.show_toast(b"Not in HF mode", b"BACK leaves", false)
            }
            (Key::Exit, Short) => app.back(), // HF-4: BACK leaves HF mode, one level back
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
        transmit(app); // HF-23, HF-24
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        let heard = match live.mode {
            ModeLive::Hf { freq, .. } => freq,
            _ => 0,
        };
        let khz = (heard / 100).min(u16::MAX as u32) as u16;
        let mut me = self;
        if app.radio.hf_seeking {
            // The seek's landing: the receiver's report moves off where it was before the seek; the frequency is
            // kept, and its band VFO and recent list follow as for tuning.
            if heard != 0 && khz != self.heard {
                (app.set.hf.freq, me.heard) = (heard, khz);
                (app.radio.hf_seeking, app.radio.hf_tuned_at, app.radio.hf_counted) = (false, app.now, false);
            }
            if app.now.wrapping_sub(app.radio.hf_tuned_at) >= SEEK_MS {
                app.radio.hf_seeking = false;
            }
        } else {
            me.heard = khz;
        }
        if !self.loaded && !(app.radio.ssb_patch && loading(live) < 100) {
            me.loaded = true;
            app.radio.hf_tuned_at = app.now;
        }
        if me != self {
            me.put(app);
        }
        if self.loaded && !app.radio.hf_counted && app.now.wrapping_sub(app.radio.hf_tuned_at) >= USED_MS {
            remember(&mut app.set.hf);
            app.radio.hf_counted = true;
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let pct = loading(cx.live);
        let loading = app.radio.ssb_patch && pct < 100;
        app.new_page(lcd, hash(&(self.loaded || !loading, self.menu)));
        if !self.loaded && loading {
            // B13: the patch streams in behind a bar (HF-3, HF-19).
            app.message(lcd, b"HF mode", b"Loading SSB patch", b"Streaming the patch into the SI4732 for USB, LSB and CW. It takes a while. BACK cancels.", TEXT);
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
            let vals = option_values(&h);
            for (i, ((name, _), v)) in OPTIONS.iter().zip(&vals).enumerate() {
                let r = Row { num: Some(i as u8 + 1), name: name.as_bytes(), value: v.as_bytes(), dot: false, selected: i as u8 == self.row, grey: false };
                app.list_row(lcd, i, 23 + 22 * i as i32, 22, &r);
            }
            let r = Row { num: Some(0), name: b"Full menu", value: b">", dot: false, selected: false, grey: false };
            app.list_row(lcd, 6, 23 + 22 * 6, 22, &r);
            app.help_box(lcd, 186, 46, OPTIONS[self.row as usize % OPTIONS.len()].1.as_bytes(), b"HF MENU");
            return;
        }
        // Band and receiver (HF-15).
        let band = h.band().map_or("", |b| BANDS[b].0);
        let chip1 = h.chip1();
        if app.dirty(R_HEAD, hash(&(band, chip1))) {
            w::field(lcd, 10, 120, 24, 42, 38, Align::Left, band.as_bytes(), &UI_12B, TEXT, BG);
            ferrite_ui::canvas::banded(lcd, 130, 24, 102, 18, BG, |lcd| {
                w::tag(lcd, 230, 25, if chip1 { b"CHIP 1".as_slice() } else { b"SI4732" }, SUB, BG);
            });
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
            ferrite_ui::canvas::banded(lcd, 12, 106, 220, 17, BG, |lcd| {
                w::chips(lcd, 12, 106, 220, &[mode_name(h.mode).as_bytes(), step.as_bytes(), h.bw_text().as_bytes(), bfo.as_bytes()], BG);
            });
        }
        // Signal: SNR on the SI4732, an S-meter on chip 1 (HF-15).
        let (rssi, snr) = match cx.live.mode {
            ModeLive::Hf { rssi, snr, .. } => (rssi, snr),
            _ => (-127, 0),
        };
        // dBuV = dBm + 107; the bar spans 0-70 dBuV.
        let lit = ((rssi + 107).clamp(0, 70) as u32 * w::METER_SEGS as u32 / 70) as u8;
        if app.dirty(R_SUB, hash(&(rssi, snr, chip1))) {
            let t = signal_text(chip1, rssi, snr);
            w::meter(lcd, 12, 140, None, lit, GREEN, BG);
            w::field(lcd, 214, 18, 132, 150, 146, Align::Right, b"", &UI_12, TEXT, BG);
            w::field(lcd, 12, 220, 152, 170, 166, Align::Left, t.as_bytes(), &UI_12, SUB, BG);
        }
        // Typing a frequency.
        let ent = self.entry_text(app);
        if app.dirty(R_NOTE, hash(&ent)) {
            w::field(lcd, 12, 220, 178, 196, 192, Align::Left, ent.as_bytes(), &UI_12B, AMBER, BG);
        }
        // What is paused (HF-6), and whether PTT can transmit here.
        let tx = h.can_tx();
        let patch = app.radio.ssb_patch;
        if app.dirty(R_HELP, hash(&(tx, patch))) {
            ferrite_ui::canvas::banded(lcd, 0, 204, 240, 96, BG, |lcd| {
                lcd.fill_r(8, 206, 224, 76, 2, SURFACE);
                let mut t = Buf::<140>::new();
                t.push(b"Paused: watch, scan, APRS, decoders, jobs and beacons. ");
                t.push(if tx { b"PTT transmits here on chip 1.".as_slice() } else { b"Receive only here." });
                if !patch {
                    t.push(b" No SSB patch: AM only.");
                }
                w::wrap(lcd, 16, 208, 209, 279, 222, 15, t.as_bytes(), &UI_12, SUB, SURFACE);
            });
        }
    }

    fn hints(self, _: &App) -> Hints {
        if !self.loaded {
            Hints::new(&[("BACK", "Cancel")])
        } else if self.menu {
            Hints::new(&[("Knob", "Change"), ("0", "Full menu"), ("BACK", "Back")])
        } else if self.n > 0 && self.point > 1 {
            Hints::new(&[("*/OK", "Go"), ("BACK", "Cancel")]) // VFO-24
        } else if self.n > 0 {
            Hints::new(&[("*", "Point"), ("OK", "Go"), ("BACK", "Cancel")])
        } else {
            Hints::new(&[("*", "Mode"), ("#", "Recent"), ("Hold MENU", "Options")])
        }
    }
}

#[cfg(test)]
mod band_list_tests {
    use super::*;

    fn at(f: Freq10, mode: u8) -> HfMem {
        HfMem { freq: f, mode, ..HfMem::DEFAULT }
    }

    fn walk(mut h: HfMem, up: bool, n: usize) -> std::vec::Vec<&'static str> {
        (0..n)
            .map(|_| {
                let b = h.next_band(up);
                h.freq = BANDS[b].3;
                BANDS[b].0
            })
            .collect()
    }

    /// HF-32: AM walks the broadcast list, SSB the amateur list (CB in it), each wrapping.
    #[test]
    fn mode_picks_the_list() {
        assert_eq!(walk(at(90_900, AM), true, 8), ["49 m", "41 m", "31 m", "25 m", "19 m", "13 m", "LW", "MW"]);
        assert_eq!(walk(at(707_400, LSB), true, 11), ["30 m", "20 m", "17 m", "15 m", "12 m", "CB", "10 m", "6 m", "160 m", "80 m", "40 m"]);
        assert_eq!(walk(at(707_400, LSB), false, 2), ["80 m", "160 m"]);
    }

    /// HF-32: AM inside an amateur band or CB keeps the amateur list; AM between bands is broadcast.
    #[test]
    fn am_on_ham_keeps_the_ham_list() {
        assert_eq!(walk(at(2_718_500, AM), true, 2), ["10 m", "6 m"]);
        assert_eq!(walk(at(2_900_000, AM), false, 1), ["CB"]);
        assert_eq!(walk(at(1_300_000, AM), true, 1), ["19 m"]);
        assert_eq!(walk(at(1_300_000, USB), true, 1), ["20 m"]);
    }

    /// HF-34: a band's first step: shortwave broadcast 5 kHz, MW 10 kHz in the Americas and 9 kHz elsewhere.
    #[test]
    fn first_step_by_band() {
        let b = |n: &str| BANDS.iter().position(|x| x.0 == n).unwrap();
        let khz = |b: usize, r: u8| STEPS_AM[first_step(b, r) as usize] / 100;
        assert_eq!(khz(b("49 m"), 0), 5);
        assert_eq!(khz(b("MW"), 0), 10);
        assert_eq!(khz(b("MW"), 3), 9);
        assert_eq!(khz(b("LW"), 2), 9);
        assert_eq!(STEPS_SSB[first_step(b("40 m"), 0) as usize], 10);
        assert_eq!(STEPS_AM[HfMem::DEFAULT.step as usize], 500);
    }
}
