//! FM broadcast (B11; MENU 0 3 1 6; manual chapter 20, BCR-1 to BCR-17): the SI4732 on 64-108 MHz with the
//! RDS station name and text, signal and SNR, and whether radio monitor is watching the slots (BCR-4).
//!
//! Keys (the manual leaves them open; these follow HF mode's): the knob or ▲▼ tune by the step, ◀▶ seek, OK
//! the presets, hold MENU this screen's options (step, AGC, radio monitor, presets; 0 the full menu: BCR-3,
//! MENU-25), BACK leaves, PTT leaves and transmits from home (BCR-16). The broadcast is an exclusive mode
//! ([`super::activity::Mode::Fm`]): it stops when the screen goes home, by any route.

use super::groups::agc_text;
use super::activity::Mode;
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::{FM_STEPS, ON_OFF};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::{BroadcastCmd, LiveAll, ModeLive, RadioCmd};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// FM tunes 64-108 MHz (BCR-17), in 10 kHz.
const LO: u16 = 6_400;
const HI: u16 = 10_800;

/// The options page (MENU): step, AGC, radio monitor, presets.
const OPTIONS: [(&str, &str); 4] = [
    ("Step", "Tuning step: 50, 100 or 200 kHz."),
    ("AGC", "Automatic gain, or a fixed gain."),
    ("Radio monitor", "Slots keep watching; one opening interrupts."),
    ("Presets", "20 stations (MENU 0 3 1 1)."),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fm {
    /// The options page is open, and its row.
    pub menu: bool,
    pub row: u8,
}

/// MENU 0 3 1 6 (BCR-1): the broadcast starts on the last station.
pub fn open(app: &mut App) -> Screen {
    app.start_mode(Mode::Fm);
    app.out.push(RadioCmd::Broadcast(BroadcastCmd::Fm(app.set.fm_freq)));
    Screen::Fm(Fm::new())
}

impl Fm {
    pub fn new() -> Self {
        Fm { menu: false, row: 0 }
    }

    fn tune(app: &mut App, d: i32) {
        let step = FM_STEPS[app.set.fm_step as usize % 3].0 as i32;
        let span = (HI - LO) as i32 + step;
        let f = (app.set.fm_freq as i32 - LO as i32 + d * step).rem_euclid(span) + LO as i32;
        app.set.fm_freq = (f as u16).clamp(LO, HI);
        app.out.push(RadioCmd::Broadcast(BroadcastCmd::Fm(app.set.fm_freq)));
    }

    fn change(self, app: &mut App, d: i32) {
        let s = &mut app.set;
        match self.row {
            0 => s.fm_step = (s.fm_step as i32 + d).rem_euclid(3) as u8,
            1 => s.fm_agc = (s.fm_agc as i32 + d).rem_euclid(11) as u8,
            2 => s.fm_monitor = !s.fm_monitor,
            _ => {}
        }
    }
}

impl View for Fm {
    /// KEY-19: ◀▶ seek (and change the option on its menu).
    fn own_arrows(self) -> bool {
        true
    }

    /// MENU-25: the options page, drawn in the shared options style.
    fn options(self, app: &mut App) -> bool {
        app.screen = Screen::Fm(Fm { menu: true, row: 0 });
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
                (Key::Up, Short | Repeat) => app.screen = Screen::Fm(Fm { row: (self.row + 3) % 4, ..self }),
                (Key::Down, Short | Repeat) => app.screen = Screen::Fm(Fm { row: (self.row + 1) % 4, ..self }),
                (Key::Left | Key::Right, Short | Repeat) => self.change(app, if e.key == Key::Right { 1 } else { -1 }),
                (Key::Digit(d @ 1..=4), Short) => app.screen = Screen::Fm(Fm { row: d - 1, ..self }),
                (Key::Digit(0), Short) => app.open(Dest::Advanced), // MENU-25: 0 the full menu
                (Key::Menu, Short) if self.row == 3 => app.open(Dest::FmPresets),
                (Key::Menu, Short) => self.change(app, 1),
                (Key::Exit, Short) => app.screen = Screen::Fm(Fm { menu: false, ..self }),
                _ => {}
            }
            return;
        }
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => Fm::tune(app, if e.key == Key::Up { 1 } else { -1 }),
            (Key::Left | Key::Right, Short) => app.out.push(RadioCmd::Broadcast(BroadcastCmd::Seek { up: e.key == Key::Right })),
            (Key::Menu, Short) => app.open(Dest::FmPresets), // OK: pick a station
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        if self.menu {
            self.change(app, d);
        } else {
            Fm::tune(app, d);
        }
    }

    /// Seek moves the station: the receiver's frequency is the one kept (BCR-1: the last station).
    fn tick(self, app: &mut App, live: &LiveAll) {
        if let ModeLive::Fm { freq, .. } = live.mode {
            if freq != 0 {
                app.set.fm_freq = freq;
            }
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        if down {
            app.stop_mode(); // BCR-16: the SI4732 muted and switched away before key-up
            super::nav::transmit(app);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.new_page(lcd, hash(&self.menu));
        if self.menu {
            let s = *app.set;
            let mut vals = [Buf::<24>::new(); 4];
            vals[0].set(FM_STEPS[s.fm_step as usize % 3].1.as_bytes());
            agc_text(s.fm_agc, &mut vals[1]);
            vals[2].set(ON_OFF[s.fm_monitor as usize].as_bytes());
            vals[3].set(b">");
            // The shared options style (MENU-25).
            super::opts::options_page(app, lcd, OPTIONS.iter().zip(&vals).map(|((name, _), v)| (name.as_bytes(), v.as_bytes())), self.row);
            app.help_box(lcd, super::opts::OPTS_HELP_Y, 46, OPTIONS[self.row as usize % 4].1.as_bytes(), b"FM MENU");
            return;
        }
        let (ps, rt, rssi, snr, stereo) = match cx.live.mode {
            ModeLive::Fm { ps, rt, rssi, snr, stereo, .. } => (ps, rt, rssi, snr, stereo),
            _ => ([b' '; 8], [0; 32], 0, 0, false),
        };
        let f = app.set.fm_freq;
        if app.dirty(R_FREQ, hash(&(f, stereo))) {
            w::field(lcd, 10, 120, 24, 42, 38, Align::Left, b"FM broadcast", &UI_12, SUB, BG);
            lcd.fill(130, 24, 102, 18, BG);
            if stereo {
                w::tag(lcd, 230, 25, b"STEREO", SUB, BG);
            }
            let mut t = Buf::<8>::new();
            let _ = uwrite!(t, "{}.{}", f / 100, f % 100 / 10);
            if !f.is_multiple_of(10) {
                let _ = uwrite!(t, "{}", f % 10);
            }
            let tw = text4_width(t.as_bytes(), &DIG_46) + 6 + text4_width(b"MHz", &UI_12B);
            let x = 120 - tw / 2;
            lcd.fill(0, 48, 240, 52, BG);
            let x2 = x + lcd.text4_rows(x, 96, t.as_bytes(), &DIG_46, TEXT, BG, 48, 100);
            lcd.text4(x2 + 6, 96, b"MHz", &UI_12B, SUB, BG);
        }
        // RDS: the station name and radio text (BCR-2), and the preset this frequency is stored in. Without
        // RDS, no empty box: one quiet line.
        let ps = trim(&ps);
        let rt = trim(&rt);
        let data = cx.store.data();
        let mut pre = Buf::<24>::new();
        if let Some(i) = (0..super::recall::FM_PLACES as u8).find(|&i| data.fm_preset(i).is_some_and(|p| p.freq == f)) {
            let _ = uwrite!(pre, "Preset {}", i + 1);
            if let Some(p) = data.fm_preset(i).filter(|p| !p.name.is_empty()) {
                pre.push(" \u{b7} ".as_bytes());
                pre.push(p.name.as_bytes());
            }
        }
        if app.dirty(R_INFO, hash(&(ps, rt, pre))) {
            lcd.fill(0, 108, 240, 80, BG);
            if ps.is_empty() && rt.is_empty() {
                w::field(lcd, 16, 208, 114, 136, 132, Align::Center, pre.as_bytes(), &UI_15B, TEXT, BG);
                w::field(lcd, 16, 208, 138, 156, 152, Align::Center, b"No RDS", &UI_12, DIM, BG);
            } else {
                lcd.fill_r(8, 110, 224, 74, 2, SURFACE);
                w::field(lcd, 16, 208, 114, 136, 132, Align::Left, ps, &UI_15B, TEXT, SURFACE);
                w::wrap(lcd, 16, 208, 138, 166, 152, 15, rt, &UI_12, SUB, SURFACE);
                w::field(lcd, 16, 208, 166, 182, 178, Align::Left, pre.as_bytes(), &UI_12, DIM, SURFACE);
            }
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

/// Text up to its padding (zeros, trailing spaces).
fn trim(b: &[u8]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let mut s = &b[..n];
    while let [rest @ .., b' '] = s {
        s = rest;
    }
    s
}
