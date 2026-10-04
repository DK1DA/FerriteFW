//! The alert (B17; manual chapters 23 and 24, WX-4 to WX-9; Alert mock-up): a SAME warning (also DSC distress
//! and the jamming alarm) over any screen, with a flashing border and the tone until OK (WX-6). It shows the
//! event, the areas, issue and expiry with minutes left, the originator, and the slot and frequency it was
//! heard on (WX-5). The decoders raise it with [`App::raise_alert`].

use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::Text;
use crate::{App, Screen};
use ferrite_core::freq::Freq10;
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::{Buf, Zero};
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// The alert's dark red fill.
const ALERT_BG: u16 = rgb565(0x1A, 0x0E, 0x10);
/// The border flashes at this half-period (WX-6).
const FLASH_MS: u32 = 500;
/// No time.
pub const NO_TIME: u16 = 0xFFFF;

/// What kind of alert: the header line.
pub const KINDS: [&str; 3] = ["WEATHER ALERT", "DSC DISTRESS", "JAMMING ALARM"];

/// The last alert, as the decoder reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertInfo {
    /// Index into [`KINDS`].
    pub kind: u8,
    pub event: Text<30>,
    pub areas: Text<20>,
    pub from: Text<14>,
    /// Minutes since midnight, or [`NO_TIME`].
    pub issued: u16,
    pub until: u16,
    /// The slot it was heard on and the frequency.
    pub slot: u8,
    pub freq: Freq10,
}

impl AlertInfo {
    pub const NONE: AlertInfo = AlertInfo {
        kind: 0,
        event: Text::EMPTY,
        areas: Text::EMPTY,
        from: Text::EMPTY,
        issued: NO_TIME,
        until: NO_TIME,
        slot: 0,
        freq: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Alert {
    /// OK pressed: the tone stops and the border stays (WX-6).
    pub silenced: bool,
}

fn hhmm(b: &mut Buf<28>, m: u16) {
    let _ = uwrite!(b, "{}:{}", Zero((m / 60) as u32, 2), Zero((m % 60) as u32, 2));
}

impl View for Alert {
    fn own_hold_menu(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Alert"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("Alert.png")
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match (e.key, e.press) {
            // WX-7: OK silences; MENU (one key with OK: held) opens the event log, as OK does once silenced.
            (Key::Menu, Press::Short) if !self.silenced => app.screen = Screen::Alert(Alert { silenced: true }),
            (Key::Menu, Press::Short | Press::Long) => {
                app.go_home();
                app.open(Dest::EventLog);
            }
            (Key::Exit, Press::Short) => app.back(), // KEY-5: back one level, to the screen it came over
            _ => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let a = *app.alert_info;
        let on = self.silenced || (app.now / FLASH_MS).is_multiple_of(2);
        if app.dirty(R_FRAME, hash(&on)) {
            let c = if on { RED } else { ALERT_BG };
            lcd.frame_r(4, 24, 232, 272, 6, c);
            lcd.frame_r(5, 25, 230, 270, 5, c);
        }
        if app.dirty(R_TOP, hash(&(a.kind, a.event, a.areas))) {
            lcd.fill_r(6, 26, 228, 268, 4, ALERT_BG);
            w::warn_icon(lcd, 16, 34, RED, ALERT_BG);
            lcd.text4(34, 45, KINDS[a.kind as usize % 3].as_bytes(), &UI_12B, RED, ALERT_BG);
            let ev: &[u8] = if a.event.is_empty() { b"Alert" } else { a.event.as_bytes() };
            w::wrap(lcd, 15, 210, 52, 96, 70, 21, ev, &UI_15B, TEXT, ALERT_BG);
            w::field(lcd, 15, 210, 98, 116, 112, Align::Left, a.areas.as_bytes(), &UI_12B, TEXT, ALERT_BG);
        }
        // Issued, until (minutes left), from, heard (WX-5).
        let now = cx.dev.clock_min;
        let mut rows = [Buf::<28>::new(); 4];
        if a.issued != NO_TIME {
            hhmm(&mut rows[0], a.issued);
        }
        if a.until != NO_TIME {
            hhmm(&mut rows[1], a.until);
            if let Some(n) = now {
                let left = (a.until as i32 - n as i32).rem_euclid(1440);
                let _ = uwrite!(rows[1], " ({} min)", left);
            }
        }
        rows[2].set(a.from.as_bytes());
        if a.freq != 0 {
            let _ = uwrite!(rows[3], "Slot {} \u{b7} ", w::LETTERS[a.slot as usize % 3] as char);
            rows[3].push(w::freq3(a.freq).as_bytes());
        }
        for (i, (label, v)) in ["Issued", "Until", "From", "Heard"].iter().zip(&rows).enumerate() {
            let y = 122 + 16 * i as i32;
            if app.dirty(R_ROW + i, hash(&v)) {
                w::field(lcd, 15, 52, y, y + 16, y + 12, Align::Left, label.as_bytes(), &UI_12, SUB, ALERT_BG);
                w::field(lcd, 70, 160, y, y + 16, y + 12, Align::Left, v.as_bytes(), &UI_12, TEXT, ALERT_BG);
            }
        }
        if app.dirty(R_HELP, hash(&self.silenced)) {
            let (c, t): (u16, &[u8]) = if self.silenced { (SUB, "Tone off \u{b7} kept in the event log".as_bytes()) } else { (rgb565(0xF2, 0x8B, 0x8E), b"Tone sounding") };
            w::bell(lcd, 16, 272, c, ALERT_BG, self.silenced);
            w::field(lcd, 32, 196, 268, 286, 282, Align::Left, t, &UI_12, c, ALERT_BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.silenced {
            Hints::new(&[("OK", "Log"), ("BACK", "Back")])
        } else {
            Hints::new(&[("OK", "Silence"), ("Hold MENU", "Log"), ("BACK", "Back")])
        }
    }
}
