//! FM broadcast (B11; MENU 0 3 1; manual chapter 20, BCR-1 to BCR-30): the SI4732 on 64-108 MHz with the
//! RDS text, signal and SNR, stereo, and whether radio monitor is watching the slots (BCR-4). One FM state with
//! home's broadcast slot ([`crate::bcast`]): while a slot plays broadcast this screen shows and tunes that slot.
//!
//! Keys (BCR-18): the knob or ▲▼ tune by the step, ◀▶ seek, OK the presets, 1 the presets and 2-5 the options at
//! that row (so MENU 0 3 1 2 still reaches the step), hold MENU this screen's options (presets, step, AGC, radio
//! monitor, sleep timer; 0 the full menu: BCR-3, MENU-25), BACK leaves, PTT leaves and transmits from home
//! (BCR-16). The broadcast is an exclusive mode ([`super::activity::Mode::Fm`]): it stops when the screen goes
//! home, by any route, unless a slot plays broadcast there.

use super::activity::Mode;
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::bcast::rds_text;
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::FM_STEPS;
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::{LiveAll, ModeLive};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// FM tunes 64-108 MHz (BCR-17), in 10 kHz.
pub(crate) const LO: u16 = 6_400;
pub(crate) const HI: u16 = 10_800;

/// The options (BCR-3): label, help and the setting's menu path (0 3 1 1-5, kept for the terminal and muscle
/// memory). Quick settings shows the same rows on a broadcast slot (BCR-28).
pub(crate) const OPTIONS: [(&str, &str, &[u8]); 5] = [
    (slow!("Presets"), slow!("20 stations: OK picks one."), &[3, 1, 1]),
    (slow!("Step"), slow!("Tuning step: 50, 100 or 200 kHz."), &[3, 1, 2]),
    (slow!("AGC"), slow!("Automatic gain, or a fixed gain."), &[3, 1, 3]),
    (slow!("Radio monitor"), slow!("Slots keep watching; one opening interrupts."), &[3, 1, 4]),
    (slow!("Sleep timer"), slow!("End the broadcast after a time."), &[3, 1, 5]),
];
const ROWS: u8 = OPTIONS.len() as u8;

/// An option's value text (the presets row: ">").
pub(crate) fn option_text(app: &App, cx_dev: &ferrite_core::msg::Device, row: u8, b: &mut Buf<24>) -> bool {
    match row {
        0 => {
            b.set(b">");
            false
        }
        r => super::groups::wired(app, cx_dev, OPTIONS[r as usize % OPTIONS.len()].2, b).unwrap_or(false),
    }
}

/// An option changed by the knob or ◀▶ (the presets row opens instead).
pub(crate) fn option_change(app: &mut App, row: u8, d: i32) {
    if row == 0 {
        app.open(Dest::FmPresets);
    } else {
        super::groups::change(app, OPTIONS[row as usize % OPTIONS.len()].2, d);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fm {
    /// The options page is open, and its row.
    pub menu: bool,
    pub row: u8,
    /// When the broadcast started, for the sleep timer (BCR-5).
    pub since: u32,
}

/// MENU 0 3 1 (BCR-1): the broadcast starts on the last station, or plays the broadcast slot's.
pub fn open(app: &mut App) -> Screen {
    Screen::Fm(Fm { since: app.now, ..Fm::new() })
}

impl Fm {
    pub fn new() -> Self {
        Fm { menu: false, row: 0, since: 0 }
    }

    /// A step up or down, wrapping in the range: the broadcast band while a slot has the broadcast (BCR-25).
    fn tune(app: &mut App, d: i32) {
        let step = FM_STEPS[app.set.fm_step as usize % 3].0 as i32;
        let (lo, hi) = app.fm_range();
        let span = (hi - lo) as i32 + step;
        let f = (app.fm_freq() as i32 - lo as i32 + d * step).rem_euclid(span) + lo as i32;
        app.fm_set((f as u16).clamp(lo, hi)); // MODE-2: the reconciler retunes a running receiver
    }
}

impl View for Fm {
    /// KEY-19: ◀▶ seek (and change the option on its menu).
    fn own_arrows(self) -> bool {
        true
    }

    /// MENU-25: the options page, drawn in the shared options style.
    fn options(self, app: &mut App) -> bool {
        app.screen = Screen::Fm(Fm { menu: true, row: 0, ..self });
        true
    }

    fn name(self) -> &'static str {
        "FM broadcast"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"FM");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        if self.menu {
            match (e.key, e.press) {
                (Key::Up, Short | Repeat) => app.screen = Screen::Fm(Fm { row: (self.row + ROWS - 1) % ROWS, ..self }),
                (Key::Down, Short | Repeat) => app.screen = Screen::Fm(Fm { row: (self.row + 1) % ROWS, ..self }),
                (Key::Left | Key::Right, Short | Repeat) => option_change(app, self.row, if e.key == Key::Right { 1 } else { -1 }),
                (Key::Digit(d @ 1..=5), Short) => app.screen = Screen::Fm(Fm { row: d - 1, ..self }),
                (Key::Digit(0), Short) => app.open(Dest::Advanced), // MENU-25: 0 the full menu
                (Key::Menu, Short) => option_change(app, self.row, 1),
                (Key::Exit, Short) => app.screen = Screen::Fm(Fm { menu: false, ..self }),
                _ => {}
            }
            return;
        }
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => Fm::tune(app, if e.key == Key::Up { 1 } else { -1 }),
            (Key::Left | Key::Right, Short) => app.fm_seek(e.key == Key::Right),
            (Key::Menu | Key::Digit(1), Short) => app.open(Dest::FmPresets), // OK: pick a station
            // MENU 0 3 1 2-5 as before the screen took 0 3 1: the options at that row (BCR-3)
            (Key::Digit(d @ 2..=5), Short) => app.screen = Screen::Fm(Fm { menu: true, row: d - 1, ..self }),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        if self.menu {
            option_change(app, self.row, d);
        } else {
            Fm::tune(app, d);
        }
    }

    /// A seek's station is taken as it lands ([`App::bcast_tick`]).
    fn tick(self, app: &mut App, _: &LiveAll) {
        // BCR-5: the sleep timer ends the broadcast once its time is up.
        let min = crate::settings::SLEEP_TIMES[app.set.fm_sleep as usize % crate::settings::SLEEP_TIMES.len()] as u32;
        if min != 0 && app.now.wrapping_sub(self.since) >= min * 60_000 {
            app.go_home();
            app.show_toast(b"Sleep timer", b"broadcast off", false);
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        if down {
            super::nav::transmit(app); // BCR-16: its way home stops the broadcast before the key-up (MODE-1)
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.new_page(lcd, hash(&self.menu));
        if self.menu {
            let mut vals = [Buf::<24>::new(); OPTIONS.len()];
            for (i, v) in vals.iter_mut().enumerate() {
                option_text(app, cx.dev, i as u8, v);
            }
            // The shared options style (MENU-25), the help under its "0 Full menu" row.
            super::opts::options_page(app, lcd, OPTIONS.iter().zip(&vals).map(|((name, ..), v)| (name.as_bytes(), v.as_bytes())), self.row);
            let help = OPTIONS[(self.row % ROWS) as usize].1;
            app.help_box(lcd, 23 + 22 * (ROWS as i32 + 1) + 6, 46, help.as_bytes(), b"FM MENU");
            return;
        }
        let (ps, rt, rssi, snr, stereo) = match cx.live.mode {
            ModeLive::Fm { ps, rt, rssi, snr, stereo, .. } if app.radio.mode == Mode::Fm => (ps, rt, rssi, snr, stereo),
            _ => ([b' '; 8], [0; 64], 0, 0, false),
        };
        let f = app.fm_freq();
        // BCR-27: the slot playing it, when one does.
        let mut title = Buf::<24>::new();
        title.set(b"FM broadcast");
        if let Some(s) = app.bcast_slot() {
            let _ = uwrite!(title, " \u{b7} slot {}", w::LETTERS[s as usize % 3] as char);
        }
        if app.dirty(R_FREQ, hash(&(f, stereo, title))) {
            w::field(lcd, 10, 150, 24, 42, 38, Align::Left, title.as_bytes(), &UI_12, SUB, BG);
            ferrite_ui::canvas::banded(lcd, 160, 24, 72, 18, BG, |lcd| {
                if stereo {
                    w::tag(lcd, 230, 25, b"STEREO", SUB, BG);
                }
            });
            let mut t = Buf::<8>::new();
            fm_mhz(f, &mut t);
            let tw = text4_width(t.as_bytes(), &DIG_46) + 6 + text4_width(b"MHz", &UI_12B);
            let x = 120 - tw / 2;
            ferrite_ui::canvas::banded(lcd, 0, 48, 240, 52, BG, |lcd| {
                let x2 = x + lcd.text4_rows(x, 96, t.as_bytes(), &DIG_46, TEXT, BG, 48, 100);
                lcd.text4(x2 + 6, 96, b"MHz", &UI_12B, SUB, BG);
            });
        }
        // RDS (BCR-26): the radio text, whole (it is shown once complete, BCR-30), wrapped; the station name only
        // when there is no radio text; and the preset this frequency is stored in. Without RDS, no empty box.
        let text = rds_text(&ps, &rt);
        let is_rt = !crate::bcast::trim(&rt).is_empty();
        let data = cx.store.data();
        let mut pre = Buf::<24>::new();
        if let Some(i) = (0..super::recall::FM_PLACES as u8).find(|&i| data.fm_preset(i).is_some_and(|p| p.freq == f)) {
            let _ = uwrite!(pre, "Preset {}", i + 1);
            if let Some(p) = data.fm_preset(i).filter(|p| !p.name.is_empty()) {
                pre.push(" \u{b7} ".as_bytes());
                pre.push(p.name.as_bytes());
            }
        }
        if app.dirty(R_INFO, hash(&(text, pre))) {
            ferrite_ui::canvas::banded(lcd, 0, 108, 240, 80, BG, |lcd| {
                if text.is_empty() {
                    w::field(lcd, 16, 208, 114, 136, 132, Align::Center, pre.as_bytes(), &UI_15B, TEXT, BG);
                    w::field(lcd, 16, 208, 138, 156, 152, Align::Center, b"No RDS", &UI_12, DIM, BG);
                } else {
                    lcd.fill_r(8, 110, 224, 74, 2, SURFACE);
                    if is_rt {
                        w::wrap(lcd, 16, 208, 113, 166, 126, 15, text, &UI_12, TEXT, SURFACE);
                    } else {
                        w::field(lcd, 16, 208, 114, 136, 132, Align::Left, text, &UI_15B, TEXT, SURFACE);
                    }
                    w::field(lcd, 16, 208, 166, 182, 178, Align::Left, pre.as_bytes(), &UI_12, DIM, SURFACE);
                }
            });
        }
        // Signal and SNR.
        let lit = (rssi as u32 * w::METER_SEGS as u32 / 70).min(w::METER_SEGS as u32) as u8;
        if app.dirty(R_SUB, hash(&(rssi, snr))) {
            w::field(lcd, 14, 120, 194, 210, 206, Align::Left, b"Signal", &UI_12, SUB, BG);
            let mut t = Buf::<24>::new();
            let _ = uwrite!(t, "{} dBuV \u{b7} SNR {} dB", rssi, snr);
            w::field(lcd, 90, 142, 194, 210, 206, Align::Right, t.as_bytes(), &UI_12, TEXT, BG);
            w::meter(lcd, 20, 216, None, lit, GREEN, BG);
        }
        // Radio monitor (BCR-4): which slots still watch, and whether one is interrupting now.
        let heard = app.rx_mask & (app.watch_mask() | 1 << app.active);
        let mut m = Buf::<40>::new();
        let mut c = SUB;
        if !app.set.fm_monitor {
            m.set(b"Monitor off: slots not heard");
        } else if heard != 0 {
            let s = heard.trailing_zeros() as usize;
            let _ = uwrite!(m, "Slot {} heard: broadcast paused", w::LETTERS[s % 3] as char);
            c = AMBER;
        } else {
            m.set(b"Monitor: the slots keep watching");
        }
        if app.dirty(R_NOTE, hash(&(m, c))) {
            w::field(lcd, 10, 220, 238, 256, 252, Align::Center, m.as_bytes(), &UI_12, c, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.menu {
            Hints::new(&[("Knob", "Change"), ("0", "Full menu"), ("BACK", "Back")])
        } else {
            Hints::new(&[(ARROWS_LR, "Seek"), ("OK", "Presets"), ("BACK", "Back")]) // hold MENU: options, as everywhere
        }
    }
}

/// "100.5", "100.55" (10 kHz units).
pub(crate) fn fm_mhz<const N: usize>(f: u16, t: &mut Buf<N>) {
    let _ = uwrite!(t, "{}.{}", f / 100, f % 100 / 10);
    if !f.is_multiple_of(10) {
        let _ = uwrite!(t, "{}", f % 10);
    }
}
