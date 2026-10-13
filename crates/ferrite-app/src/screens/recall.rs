//! Short frequency lists that tune something: HF recent (# in HF mode; chapter 20a, HF-28; C18), the last 10
//! HF frequencies with their mode, newest first; and FM presets (MENU 0 3 1 1; chapter 20; C19), 20 places.
//! OK tunes the entry: an FM preset goes to the broadcast receiver; an HF entry back to HF mode. FM presets are
//! stored records ([`crate::data`]); the HF list is HF mode's memory, kept with the settings (HF-29).

use super::lists::*;
use super::nav::Dest;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::{BroadcastCmd, RadioCmd};
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// FM presets: 20 places (0 3 1 1).
pub const FM_PLACES: u16 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Recall {
    /// FM presets, else HF recent.
    pub fm: bool,
    pub cur: Cursor,
}

const ALL: fn(u16) -> bool = |_| true;

fn n(app: &App, fm: bool) -> u16 {
    if fm { FM_PLACES } else { super::hf::recent(&app.set.hf).count() as u16 }
}

fn fm_text(f: u16, b: &mut Buf<16>) {
    b.clear();
    let _ = uwrite!(b, "{}.{} MHz", f / 100, f % 100 / 10);
}

impl View for Recall {
    fn name(self) -> &'static str {
        if self.fm { "FM presets" } else { "HF recent" }
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) if self.fm => {
                let Some(p) = d.fm_preset(self.cur.sel as u8) else {
                    return app.show_toast(b"Empty preset", b"", false);
                };
                app.set.fm_freq = p.freq;
                if app.radio.mode == super::activity::Mode::Fm {
                    app.out.push(RadioCmd::Broadcast(BroadcastCmd::Fm(p.freq))); // else FM's start takes it (MODE-2)
                }
                let mut f = Buf::<16>::new();
                fm_text(p.freq, &mut f);
                // Back to the FM screen it was picked from (replaced, not opened again: one BACK leaves FM);
                // picked from the menu (MENU 0 3 1 1), FM opens over the menu.
                app.back();
                match app.screen {
                    Screen::Fm(f) => app.screen = Screen::Fm(super::fm::Fm { menu: false, ..f }),
                    _ => app.open(Dest::FmBroadcast),
                }
                app.show_toast(f.as_bytes(), p.name.as_bytes(), false);
            }
            (Key::Menu, Short) => {
                // HF-28: OK tunes it, back in HF mode
                let hit = super::hf::recent(&app.set.hf).nth(self.cur.sel as usize);
                if let Some((f, m)) = hit {
                    app.back();
                    super::hf::tune_recent(app, f, m);
                }
            }
            (Key::Hash, Short) if !self.fm && n(app, false) > 0 => {
                app.set.hf.recent = [0; super::hf::RECENT];
                app.show_toast(b"Recent list", b"cleared", false);
                app.screen = Screen::Recall(Recall { cur: Cursor::default(), ..self });
            }
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let _ = store;
        let cur = self.cur.step(d, n(app, self.fm), ROWS1_FULL, &ALL);
        app.screen = Screen::Recall(Recall { cur, ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, self.fm);
        if !app.gate((self, data.version(), app.set.hf.recent)) {
            return;
        }
        let count = n(app, self.fm);
        let mut l = Buf::<24>::new();
        if self.fm {
            let used = (0..FM_PLACES).filter(|&i| data.fm_preset(i as u8).is_some()).count();
            let _ = uwrite!(l, "{} of {} used", used, FM_PLACES);
        } else {
            let _ = uwrite!(l, "{} of 10", count);
        }
        app.list_head(lcd, l.as_bytes(), if self.fm { b"MENU 0 3 1 1" } else { b"newest first" });
        let rows = self.cur.rows(count, ROWS1_FULL, &ALL, |row, i| {
            let (mut name, mut v) = (Buf::<20>::new(), Buf::<16>::new());
            let mut grey = false;
            if self.fm {
                match data.fm_preset(i as u8) {
                    Some(p) => {
                        let mut f = Buf::<16>::new();
                        fm_text(p.freq, &mut f);
                        let _ = uwrite!(name, "{} {}", i + 1, Txt(f.as_bytes()));
                        v.set(p.name.as_bytes());
                    }
                    None => {
                        let _ = uwrite!(name, "{} Empty", i + 1);
                        grey = true;
                    }
                }
            } else if let Some((freq, mode)) = super::hf::recent(&app.set.hf).nth(i as usize) {
                let mut f = Buf::<16>::new();
                mhz_text(freq, &mut f);
                let _ = uwrite!(name, "{} MHz", Txt(f.as_bytes()));
                v.set(super::hf::mode_name(mode).as_bytes());
            }
            app.row1(lcd, row, name.as_bytes(), v.as_bytes(), false, i == self.cur.sel, grey);
        });
        app.rows_end(lcd, rows, ROWS1_FULL, ROW1_H);
        app.list_cue(lcd, self.cur, count, ROWS1_FULL, ROW1_H, rows, &ALL);
        if count == 0 {
            app.empty(lcd, b"Nothing yet", b"10 s on a frequency adds it");
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.fm {
            Hints::new(&[(super::shared::ARROWS_UD, "Move"), ("OK", "Listen"), ("BACK", "Back")])
        } else {
            Hints::new(&[("OK", "Tune"), ("#", "Clear"), ("BACK", "Back")])
        }
    }
}
