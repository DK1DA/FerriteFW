//! Help, inside About (MENU 0 9 9; manual chapter 36, HELP-1 to HELP-5): the manual's QR code and the key card as
//! pages, reached with ◀▶ like the self-test. Read only, rarely opened: the text lives in slow flash.

use super::shared::*;
use super::table::slow_table;
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

slow_table! {
    /// HELP-3: home's keys (Appendix B, the key card), one line each.
    pub(crate) HOME_KEYS: [Row; 15] = [
        (slow!("Knob \x18\x19"), slow!("Tune or step channels")),
        (slow!("\x1b\x1a"), slow!("Band, or zone")),
        (slow!("0-9"), slow!("Type freq or channel")),
        (slow!("*"), slow!("Mute slot; point")),
        (slow!("SLOT"), slow!("Next slot, A-B-C")),
        (slow!("Hold SLOT"), slow!("Edit the slot")),
        (slow!("V/M"), slow!("VFO or channel")),
        (slow!("Hold V/M"), slow!("Save VFO as channel")),
        (slow!("MENU"), slow!("Quick settings")),
        (slow!("Hold MENU"), slow!("Advanced settings")),
        (slow!("MENU 0"), slow!("Menu by number")),
        (slow!("#"), slow!("Status; pause watch")),
        (slow!("Hold #"), slow!("Key lock")),
        (slow!("Hold *"), slow!("Quick menu")),
        (slow!("Hold BACK"), slow!("Home, from anywhere")),
    ];
}

slow_table! {
    /// HELP-3: the side keys and PTTs.
    pub(crate) SIDE_KEYS: [Row; 13] = [
        ("", slow!("SIDE1 (PF1)")),
        (slow!("Tap"), slow!("Monitor on or off")),
        (slow!("Hold"), slow!("Monitor while held")),
        (slow!("+ Knob \x18\x19"), slow!("Squelch")),
        (slow!("+ \x1b\x1a"), slow!("Clarifier")),
        ("", slow!("SIDE2 (PF2)")),
        (slow!("Tap"), slow!("Scan start or stop")),
        (slow!("Hold"), slow!("Frequency counter")),
        (slow!("+ Knob \x18\x19"), slow!("1 MHz or 10 channels")),
        ("", slow!("PTT")),
        (slow!("PTT A"), slow!("Transmit, active slot")),
        (slow!("PTT B"), slow!("Transmit on slot B")),
        (slow!("PTT+SIDE2"), slow!("Tone burst")),
    ];
}

slow_table! {
    /// HELP-5: the menu numbers most often wanted (MENU, then the digits).
    pub(crate) MENU_NOS: [Row; 14] = [
        (slow!("0 0"), slow!("Status page")),
        (slow!("0 2 1"), slow!("Channels")),
        (slow!("0 2 3"), slow!("Zones")),
        (slow!("0 3 1"), slow!("FM broadcast")),
        (slow!("0 3 2 6"), slow!("Band scope")),
        (slow!("0 3 3"), slow!("Frequency counter")),
        (slow!("0 3 4"), slow!("HF mode")),
        (slow!("0 3 6"), slow!("Tone search")),
        (slow!("0 4 1"), slow!("Event log")),
        (slow!("0 5 1"), slow!("APRS heard")),
        (slow!("0 5 2"), slow!("Messages")),
        (slow!("0 7 2"), slow!("Home layout")),
        (slow!("0 9 2 1"), slow!("Bluetooth")),
        (slow!("0 9 8 1"), slow!("Run setup again")),
    ];
}

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
    w::field(lcd, 8, 224, t, t + 17, t + 13, Align::Center, slow!("The full manual").as_bytes(), &UI_12B, TEXT, BG);
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
            rows(lcd, slow!("Hold a digit on home").as_bytes(), all.iter().copied())
        }
        Help::MenuNos => rows(lcd, slow!("MENU, then the number").as_bytes(), MENU_NOS.iter().copied()),
    }
}
