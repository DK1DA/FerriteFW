//! The boot screen: logo, wordmark, version, callsign, and the self-test as a bar and one status line. A failed
//! check shows on the same screen, in red, naming the check; the radio then waits for a key.
use crate::assets::*;
use crate::gfx::{Align, Gfx, AMBER, BG, BLUE, LINE, RED, SUB, TEXT};
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

/// Full screen.
pub fn draw(lcd: &mut impl Panel, b: &BootInfo) {
    lcd.fill(0, 0, 240, 320, BG);
    lcd.mask4(64, 28, &LOGO112_RING, SUB, Some((&LOGO112_WIND, BLUE)), BG);
    lcd.text4_aligned(120, 188, Align::Center, b"FERRITE", &WORD_40, TEXT, BG);
    lcd.text4_aligned(120, 209, Align::Center, b.version.as_bytes(), &UI_12, SUB, BG);
    if let Some(call) = b.callsign {
        lcd.text4_aligned(120, 243, Align::Center, call.as_bytes(), &UI_15B, TEXT, BG);
    }
    let n = b.checks.len() as i32;
    let done = b.checks.iter().filter(|c| c.state != State::Pending).count() as i32;
    let failed = b.checks.iter().find(|c| c.state == State::Fail);
    let bar = if n > 0 { done * 120 / n } else { 120 };
    lcd.fill(60, 264, bar, 4, if failed.is_some() { RED } else { BLUE });
    lcd.fill(60 + bar, 264, 120 - bar, 4, LINE);
    let mut line = Buf::<31>::new();
    let (text, col): (&[u8], u16) = if let Some(c) = failed {
        (v!(line, "{}: {}", c.label, if c.value.is_empty() { "failed" } else { c.value }), RED)
    } else if done < n {
        (v!(line, "Self-test {} of {}", done, n), SUB)
    } else if b.tx_armed {
        (b"Ready", SUB)
    } else {
        ("Ready · transmit not armed".as_bytes(), AMBER)
    };
    lcd.fill(0, 276, 240, 44, BG);
    lcd.text4_aligned(120, 290, Align::Center, text, &UI_12, col, BG);
    if failed.is_some() {
        lcd.text4_aligned(120, 308, Align::Center, b"Any key: continue", &UI_12, SUB, BG);
    }
}
