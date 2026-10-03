//! FM broadcast (manual chapter 20, BCR-1 to BCR-17; no mock-up): hold 2 on home. The SI4732 alone, 64-108
//! MHz, with the RDS station name and text, signal and SNR (BCR-2).
//!
//! The manual leaves this screen's keys open; these follow home's: the knob or ▲▼ tune by the step (MENU 0 3 1
//! 2, 100 kHz), ◀▶ seek, MENU the receiver's options (MENU 0 3 1, BCR-3), 0 the full menu, BACK home. PTT
//! leaves the broadcast and transmits from home (BCR-16: the default PTT, home first).

use super::activity::Mode;
use super::nav::Dest;
use super::parts;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::{BroadcastCmd, LiveAll, ModeLive, RadioCmd, SLOTS};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// 64-108 MHz in 10 kHz units (BCR-17); the step (100 kHz).
pub const LO: u16 = 6_400;
pub const HI: u16 = 10_800;
pub const STEP: u16 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fm;

/// Hold 2 (BCR-1).
pub fn open(app: &mut App) -> Screen {
    app.start_mode(Mode::Fm);
    app.out.push(RadioCmd::Broadcast(BroadcastCmd::Fm(app.radio.fm)));
    Screen::Fm(Fm)
}

fn tune(app: &mut App, f: u16) {
    app.radio.fm = f;
    app.out.push(RadioCmd::Broadcast(BroadcastCmd::Fm(f)));
}

impl View for Fm {
    fn name(self) -> &'static str {
        "FM broadcast"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"FM");
    }

    fn key(self, app: &mut App, e: KeyEvent, s: &impl ChannelStore) {
        use Press::*;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { 1 } else { -1 }, s),
            (Key::Left | Key::Right, Short) => app.out.push(RadioCmd::Broadcast(BroadcastCmd::Seek { up: e.key == Key::Right })),
            (Key::Menu, Short) => app.open(Dest::Sub(3, 1)),
            (Key::Digit(0), Short) => app.open(Dest::Advanced),
            (Key::Exit, Short) => {
                app.stop_mode();
                app.back();
            }
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let span = (HI - LO) as i32;
        let f = (app.radio.fm as i32 - LO as i32 + d * STEP as i32).rem_euclid(span + STEP as i32) + LO as i32;
        tune(app, f as u16);
    }

    fn tick(self, app: &mut App, live: &LiveAll) {
        // A seek lands where the receiver says.
        if let ModeLive::Fm { freq, .. } = live.mode {
            if freq != 0 {
                app.radio.fm = freq;
            }
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (ps, rt, rssi, snr, stereo) = match cx.live.mode {
            ModeLive::Fm { ps, rt, rssi, snr, stereo, .. } => (ps, rt, rssi, snr, stereo),
            _ => ([0; 8], [0; 32], 0, 0, false),
        };
        let f = app.radio.fm;
        // Frequency, MHz, stereo.
        if app.dirty(1, hash(&(f, stereo))) {
            let mut b = Buf::<16>::new();
            let _ = uwrite!(b, "{}.{}{}", f / 100, f % 100 / 10, f % 10);
            lcd.fill(0, 24, 240, 50, BG);
            let wd = lcd.text4_rows(16, 70, b.as_bytes(), &DIG_46, TEXT, BG, 30, 72);
            lcd.text4_rows(16 + wd + 6, 70, b"MHz", &UI_12, SUB, BG, 56, 74);
            if stereo {
                w::tag(lcd, 226, 28, b"STEREO", GREEN, BG);
            }
        }
        // RDS: station name, then the text over two lines.
        let ps = super::activity::trim0(&ps);
        let rt = super::activity::trim0(&rt);
        if app.dirty(2, hash(&(ps, rt))) {
            lcd.fill(0, 78, 240, 60, BG);
            parts::card(lcd, 6, 80, 228, 58, LINE);
            let name: &[u8] = if ps.is_empty() { b"No RDS" } else { ps };
            w::field(lcd, 15, 160, 84, 102, 98, Align::Left, name, &UI_12B, if ps.is_empty() { DIM } else { TEXT }, SURFACE);
            w::field(lcd, 175, 50, 84, 102, 98, Align::Right, b"RDS", &UI_12, SUB, SURFACE);
            parts::wrap(lcd, 15, 210, 116, 16, 2, rt, &UI_12, SUB, SURFACE);
        }
        // Signal and SNR (BCR-2).
        if app.dirty(3, hash(&(rssi, snr))) {
            w::field(lcd, 8, 60, 146, 164, 159, Align::Left, b"Signal", &UI_12, SUB, BG);
            parts::segs(lcd, 68, 151, 10, (rssi / 6).min(10), 8, 10, 8, GREEN, BG);
            let mut b = Buf::<16>::new();
            let _ = uwrite!(b, "{} dBuV", rssi);
            w::field(lcd, 170, 62, 146, 164, 159, Align::Right, b.as_bytes(), &UI_12, TEXT, BG);
            let mut s = Buf::<16>::new();
            let _ = uwrite!(s, "{} dB", snr);
            parts::kv(lcd, 8, 68, 232, 181, b"SNR", s.as_bytes(), TEXT, BG);
        }
        // Radio monitor (BCR-4): which slots keep watching; 18-64 MHz ones can't (BCR-15).
        let m = app.watch_mask() | 1 << app.active;
        if app.dirty(4, hash(&m)) {
            let mut b = Buf::<32>::new();
            for s in (0..SLOTS as u8).filter(|s| m & 1 << s != 0) {
                if app.slot(s).freq() < ferrite_core::freq::mhz(64) {
                    continue;
                }
                if !b.is_empty() {
                    b.push(b", ");
                }
                b.push(&[w::LETTERS[s as usize]]);
            }
            parts::kv(lcd, 8, 68, 232, 199, b"Monitor", if b.is_empty() { b"Off" } else { b.as_bytes() }, TEXT, BG);
            parts::kv(lcd, 8, 68, 232, 217, b"Step", b"100 kHz", TEXT, BG);
            w::field(lcd, 8, 224, 232, 250, 245, Align::Left, b"An open slot interrupts it.", &UI_12, DIM, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("Knob", "Tune"), (ARROWS_LR, "Seek"), ("OK", "Options")])
    }
}
