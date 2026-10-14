//! Help, inside About (MENU 0 9 9; manual chapter 36, HELP-1 to HELP-5): the manual's QR code and the key card as
//! pages, reached with ◀▶ like the self-test. Read only, rarely opened: the text lives in slow flash.

use super::shared::*;
use crate::App;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::qr::{MANUAL_URL, QR_SIZE};
use ferrite_ui::widgets as w;

/// A key and what it does; a row with an empty key is a section heading.
pub(crate) type Row = (&'static str, &'static str);

/// Where the action column starts; keys are drawn in bold from x = 8, as in the hint bar.
pub(crate) const ACTION_X: i32 = 80;
/// The action column's width (to an 8 px right margin).
pub(crate) const ACTION_W: i32 = 240 - 8 - ACTION_X;
const ROW_H: i32 = 18;

/// HELP-3: home's keys (Appendix B, the key card), one line each.
pub(crate) const HOME_KEYS: [Row; 15] = [
    ("Knob \x18\x19", "Tune or step channels"),
    ("\x1b\x1a", "Band, or zone"),
    ("0-9", "Type freq or channel"),
    ("*", "Mute slot; point"),
    ("SLOT", "Next slot, A-B-C"),
    ("Hold SLOT", "Edit the slot"),
    ("V/M", "VFO or channel"),
    ("Hold V/M", "Save VFO as channel"),
    ("MENU", "Quick settings"),
    ("Hold MENU", "Advanced settings"),
    ("MENU 0", "Menu by number"),
    ("#", "Status; pause watch"),
    ("Hold #", "Key lock"),
    ("Hold *", "Quick menu"),
    ("Hold BACK", "Home, from anywhere"),
];

/// HELP-3: the side keys and PTTs.
pub(crate) const SIDE_KEYS: [Row; 13] = [
    ("", "SIDE1 (PF1)"),
    ("Tap", "Monitor on or off"),
    ("Hold", "Monitor while held"),
    ("+ Knob \x18\x19", "Squelch"),
    ("+ \x1b\x1a", "Clarifier"),
    ("", "SIDE2 (PF2)"),
    ("Tap", "Scan start or stop"),
    ("Hold", "Frequency counter"),
    ("+ Knob \x18\x19", "1 MHz or 10 channels"),
    ("", "PTT"),
    ("PTT A", "Transmit, active slot"),
    ("PTT B", "Transmit on slot B"),
    ("PTT+SIDE2", "Tone burst"),
];

/// HELP-5: the menu numbers most often wanted (MENU, then the digits).
pub(crate) const MENU_NOS: [Row; 14] = [
    ("0 0", "Status page"),
    ("0 2 1", "Channels"),
    ("0 2 3", "Zones"),
    ("0 3 1", "FM broadcast"),
    ("0 3 2 6", "Band scope"),
    ("0 3 3", "Frequency counter"),
    ("0 3 4", "HF mode"),
    ("0 3 6", "Tone search"),
    ("0 4 1", "Event log"),
    ("0 5 1", "APRS heard"),
    ("0 5 2", "Messages"),
    ("0 7 2", "Home layout"),
    ("0 9 2 1", "Bluetooth"),
    ("0 9 8 1", "Run setup again"),
];

/// One key | action line at baseline `b`, or a section heading when the key is empty.
fn row(lcd: &mut impl Panel, b: i32, (key, action): Row) {
    if key.is_empty() {
        w::field(lcd, 8, 224, b - 13, b + 4, b, Align::Left, action.as_bytes(), &UI_12B, BLUE, BG);
        return;
    }
    w::key_label(lcd, 8, b, key, "", BG, true);
    w::field(lcd, ACTION_X, ACTION_W, b - 13, b + 4, b, Align::Left, action.as_bytes(), &UI_12, SUB, BG);
}

/// A page of rows from the top of the content area, with an optional heading line.
fn rows(lcd: &mut impl Panel, head: &[u8], rows: impl Iterator<Item = Row>) {
    let mut b = 37;
    if !head.is_empty() {
        w::field(lcd, 8, 224, 22, 40, 36, Align::Left, head, &UI_12B, TEXT, BG);
        lcd.fill(0, 41, 240, 1, LINE);
        b = 58;
    }
    for r in rows {
        row(lcd, b, r);
        b += ROW_H;
    }
}

/// HELP-4: the ten hold digits, from the table the shortcut list (MENU 0 9 1 4) runs.
const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// HELP-2: the QR code, centred, and the address under it.
fn manual(lcd: &mut impl Panel) {
    const SCALE: i32 = 5;
    let side = (QR_SIZE + 8) * SCALE;
    let y = 30;
    w::manual_qr(lcd, (240 - side) / 2, y, SCALE);
    let t = y + side + 8;
    w::field(lcd, 8, 224, t, t + 17, t + 13, Align::Center, "The full manual".as_bytes(), &UI_12B, TEXT, BG);
    w::field(lcd, 8, 224, t + 18, t + 35, t + 31, Align::Center, MANUAL_URL.as_bytes(), &UI_12, SUB, BG);
}

/// Which help page, drawn once per visit (the page is static).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Help {
    Manual,
    HomeKeys,
    SideKeys,
    Digits,
    MenuNos,
}

pub(crate) fn render(app: &mut App, lcd: &mut impl Panel, h: Help) {
    if !app.dirty(R_TOP, hash(&h)) {
        return;
    }
    lcd.fill(0, 21, 240, 279, BG);
    match h {
        Help::Manual => manual(lcd),
        Help::HomeKeys => rows(lcd, b"", HOME_KEYS.iter().copied()),
        Help::SideKeys => rows(lcd, b"", SIDE_KEYS.iter().copied()),
        Help::Digits => {
            // a plain array: core's iterator adapters (a zip) would land in fast flash
            let mut all: [Row; 10] = [("", ""); 10];
            for (i, r) in all.iter_mut().enumerate() {
                *r = (DIGITS[i], super::shortcut::SHORTCUTS[i]);
            }
            rows(lcd, "Hold a digit on home".as_bytes(), all.iter().copied())
        }
        Help::MenuNos => rows(lcd, "MENU, then the number".as_bytes(), MENU_NOS.iter().copied()),
    }
}
