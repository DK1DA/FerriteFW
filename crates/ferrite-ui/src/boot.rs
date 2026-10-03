//! Boot screens: logo style (A) or self-test list (B). A failed check always shows the list.
//! Mirrors fw/ui/boot.c.
use crate::assets::*;
use crate::gfx::{Align, Gfx, AMBER, BG, BLUE, GREEN, LINE, RED, SUB, TEXT};
use crate::panel::Panel;
use crate::text::Buf;
use crate::v;

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Pending,
    Ok,
    Off,
    Fail,
}

#[derive(Clone, Copy)]
pub struct Check {
    pub label: &'static str,
    pub value: &'static str,
    pub state: State,
}

pub struct BootInfo<'a> {
    pub version: &'a str,
    /// The owner's callsign, shown on the logo screen; None until one is set.
    pub callsign: Option<&'a str>,
    pub checks: &'a [Check],
    pub tx_armed: bool,
}

#[derive(Clone, Copy)]
pub enum Style {
    Logo,
    List,
}

fn hint(lcd: &mut impl Panel, key: &str, what: &str) {
    lcd.fill(0, 300, 240, 1, LINE);
    let x = lcd.text4(8, 314, key.as_bytes(), &UI_12B, TEXT, BG);
    lcd.text4(8 + x + 4, 314, what.as_bytes(), &UI_12, SUB, BG);
}

fn draw_list(lcd: &mut impl Panel, b: &BootInfo) {
    lcd.mask4(8, 7, &LOGO40_RING, SUB, Some((&LOGO40_WIND, BLUE)), BG);
    lcd.text4(56, 38, b"FERRITE", &WORD_26, TEXT, BG);
    lcd.text4_aligned(232, 38, Align::Right, b.version.as_bytes(), &UI_12, SUB, BG);
    lcd.fill(0, 54, 240, 1, LINE);
    for (i, c) in b.checks.iter().enumerate() {
        let y = 58 + i as i32 * 24;
        let (icon, col) = match c.state {
            State::Ok => (&ICON_CHECK, GREEN),
            State::Fail => (&ICON_CROSS, RED),
            _ => (&ICON_DOT, SUB),
        };
        lcd.mask4(8, y + 3, icon, col, None, BG);
        lcd.text4(30, y + 15, c.label.as_bytes(), &UI_15, if c.state == State::Fail { RED } else { TEXT }, BG);
        let vc = match c.state {
            State::Ok => TEXT,
            State::Fail => RED,
            _ => SUB,
        };
        lcd.text4_aligned(232, y + 15, Align::Right, c.value.as_bytes(), &UI_15, vc, BG);
    }
    if !b.tx_armed {
        lcd.frame_r(8, 258, 224, 26, 4, AMBER);
        lcd.mask4(14, 264, &ICON_LOCK, AMBER, None, BG);
        let x = lcd.text4(34, 276, b"Transmit not armed", &UI_15, TEXT, BG);
        lcd.text4(34 + x + 5, 276, b"(Menu 11 1)", &UI_12, SUB, BG);
    }
    hint(lcd, "Any key", "skip");
}

fn draw_logo(lcd: &mut impl Panel, b: &BootInfo) {
    lcd.mask4(64, 28, &LOGO112_RING, SUB, Some((&LOGO112_WIND, BLUE)), BG);
    lcd.text4_aligned(120, 188, Align::Center, b"FERRITE", &WORD_40, TEXT, BG);
    lcd.text4_aligned(120, 209, Align::Center, b.version.as_bytes(), &UI_12, SUB, BG);
    if let Some(call) = b.callsign {
        lcd.text4_aligned(120, 243, Align::Center, call.as_bytes(), &UI_15B, TEXT, BG);
    }
    let n = b.checks.len() as i32;
    let done = b.checks.iter().filter(|c| c.state != State::Pending).count() as i32;
    let bar = if n > 0 { done * 120 / n } else { 120 };
    lcd.fill(60, 264, bar, 4, BLUE);
    lcd.fill(60 + bar, 264, 120 - bar, 4, LINE);
    let mut line = Buf::<31>::new();
    let text: &[u8] = if done < n {
        v!(line, "Self-test {} of {}", done, n)
    } else if b.tx_armed {
        b"Ready"
    } else {
        "Ready · transmit not armed".as_bytes()
    };
    lcd.fill(0, 276, 240, 20, BG);
    let col = if done < n || b.tx_armed { SUB } else { AMBER };
    lcd.text4_aligned(120, 290, Align::Center, text, &UI_12, col, BG);
}

/// Full screen; falls back to the list on a failure.
pub fn draw(lcd: &mut impl Panel, style: Style, b: &BootInfo) {
    lcd.fill(0, 0, 240, 320, BG);
    let failed = b.checks.iter().any(|c| c.state == State::Fail);
    match (style, failed) {
        (Style::List, _) | (_, true) => draw_list(lcd, b),
        (Style::Logo, false) => draw_logo(lcd, b),
    }
}
