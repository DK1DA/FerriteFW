//! The placeholder for every screen not built yet: its name, "coming", and where the manual describes it, so
//! each way in ([`super::nav`]) can be tried in the simulator. Settings groups and third-level lists follow
//! their MENU 0 paths with the digits (MENU-5), so a path like MENU 0 9 1 2 lands on the key test's
//! placeholder. BACK returns where it was opened from.

use super::nav::{self, Dest};
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Coming {
    pub dest: Dest,
}

impl View for Coming {
    fn name(self) -> &'static str {
        self.dest.name()
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        let mut h = Buf::<32>::new();
        self.dest.heading(&mut h);
        let s = h.as_bytes();
        // Whole words only: up to the last space that fits.
        let cut = if s.len() <= 10 { s.len() } else { s[..11].iter().rposition(|&c| c == b' ').unwrap_or(10) };
        b.set(&s[..cut]);
    }

    fn timeout(self) -> Option<u32> {
        matches!(self.dest, Dest::Group(_) | Dest::Sub(..) | Dest::Value(_) | Dest::Tones).then_some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Exit, Press::Short) => app.back(),
            (Key::Digit(d), Press::Short) => {
                let mut path = [0u8; 3];
                if let Some(n) = self.dest.menu_path(&mut path) {
                    path[n] = d;
                    match nav::menu0(&path[..n + 1]) {
                        Some(to) => app.open(to),
                        None => {
                            let mut p = Buf::<20>::new();
                            p.push(b"MENU 0");
                            for x in &path[..n + 1] {
                                let _ = uwrite!(p, " {}", x);
                            }
                            app.show_toast(p.as_bytes(), b"setting comes next", false);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        match (self.dest, down) {
            (Dest::Transmit, false) => app.go_home(), // released: back home
            (Dest::Transmit, true) => {}
            (_, true) => nav::transmit(app),
            _ => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        // One placeholder can open another (a group, then an item): the whole content area is redrawn.
        if app.dirty(R_TOP, Mix::new().any(&self.dest).get()) {
            lcd.fill(0, w::SBAR_H, 240, w::HINT_Y - w::SBAR_H, BG);
            let mut h = Buf::<32>::new();
            self.dest.heading(&mut h);
            let mut t = Buf::<40>::new();
            t.push(h.as_bytes());
            t.push(b": coming");
            w::field(lcd, 8, 224, 130, 147, 143, t.as_bytes(), const { &w::st(&UI_12B, TEXT, BG, Align::Center) });
            let (_, ch, id) = self.dest.info();
            let mut m = Buf::<40>::new();
            let _ = uwrite!(m, "Manual chapter {} \u{b7} {}", ch, id);
            w::field(lcd, 8, 224, 152, 169, 165, m.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Center) });
            if self.dest.menu_path(&mut [0; 3]).is_some() {
                w::field(lcd, 8, 224, 174, 191, 187, b"Digits follow the menu path", const { &w::st(&UI_12, DIM, BG, Align::Center) });
            }
        }
    }

    fn hints(self, _: &App) -> Hints {
        match self.dest {
            Dest::Transmit => Hints::new(&[("PTT", "Release: home")]),
            Dest::Group(_) | Dest::Sub(..) => Hints::new(&[("1-9", "Open"), ("BACK", "Back")]),
            _ => Hints::new(&[("BACK", "Back")]),
        }
    }
}
