//! The reader image's screens (BACKUP-9.4, decision 264): while Ferrite Console backs a radio up, the radio says so,
//! with the read's progress, instead of a blank or a diagnostics page. hwtest's reader screen, in Ferrite's fonts.
//!
//! ```text
//!        Ferrite Console
//!          Backing up
//!          your radio
//!   [==========----------]
//!          Reading 42%
//!      Keep the cable in.
//!      Don't turn it off.
//! ```
use crate::assets::{NAME_26, UI_15B};
use crate::gfx::{Align, Gfx, BG, BLUE, GREEN, LINE, SUB, TEXT};
use crate::panel::W;

pub const BAR_X: i32 = 24;
pub const BAR_Y: i32 = 196;
pub const BAR_W: i32 = W - 2 * BAR_X;
pub const BAR_H: i32 = 8;
const STATUS_Y: i32 = BAR_Y + 32;

pub const TITLE: &[u8] = b"Ferrite Console";
pub const BIG: [&[u8]; 2] = [b"Backing up", b"your radio"];
pub const SMALL: [&[u8]; 2] = [b"Keep the cable in.", b"Don't turn it off."];
/// Before Console asks for the backup screen (the reader just started).
pub const WAIT_BIG: [&[u8]; 2] = [b"Backup", b"reader"];
pub const WAIT_SMALL: [&[u8]; 2] = [b"Waiting for", b"Ferrite Console."];
pub const DONE: &[u8] = b"Read and checked";

/// What the bar shows: the read (`fdump`), then the radio's own CRC of the chip (`flashcrc`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Reading,
    Checking,
}

fn page(lcd: &mut impl Gfx, big: [&[u8]; 2], small: [&[u8]; 2]) {
    lcd.fill(0, 0, W, crate::panel::H, BG);
    lcd.text4_aligned(W / 2, 64, Align::Center, TITLE, &UI_15B, SUB, BG);
    for (i, s) in big.iter().enumerate() {
        lcd.text4_aligned(W / 2, 123 + 36 * i as i32, Align::Center, s, &NAME_26, TEXT, BG);
    }
    for (i, s) in small.iter().enumerate() {
        lcd.text4_aligned(W / 2, 262 + 22 * i as i32, Align::Center, s, &UI_15B, TEXT, BG);
    }
}

/// The reader just started: waiting for Console.
pub fn waiting(lcd: &mut impl Gfx) {
    page(lcd, WAIT_BIG, WAIT_SMALL);
}

/// The backup screen, the bar empty.
pub fn draw(lcd: &mut impl Gfx) {
    page(lcd, BIG, SMALL);
    lcd.fill(BAR_X, BAR_Y, BAR_W, BAR_H, LINE);
}

/// The bar and its line: "Reading 42%", "Checking 80%", then "Read and checked" (green) when the check ends.
pub fn progress(lcd: &mut impl Gfx, phase: Phase, done: u32, total: u32) {
    if total == 0 {
        return;
    }
    let pct = ferrite_core::reader::percent(done, total);
    let w = BAR_W * pct as i32 / 100;
    let end = pct == 100 && phase == Phase::Checking;
    lcd.fill(BAR_X, BAR_Y, w, BAR_H, if end { GREEN } else { BLUE });
    lcd.fill(BAR_X + w, BAR_Y, BAR_W - w, BAR_H, LINE);
    let mut t = crate::text::Buf::<24>::new();
    if end {
        t.push(DONE);
    } else {
        t.push(if phase == Phase::Reading { b"Reading " } else { b"Checking " });
        let _ = ufmt::uwrite!(t, "{}%", pct);
    }
    lcd.fill(0, BAR_Y + 14, W, 24, BG);
    lcd.text4_aligned(W / 2, STATUS_Y, Align::Center, t.as_bytes(), &UI_15B, SUB, BG);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::FrameBuffer;

    /// BACKUP-9.4: every character on the reader's screens is in its font (none draws as '?').
    #[test]
    fn every_glyph_is_in_its_font() {
        let ui: [&[u8]; 9] = [TITLE, SMALL[0], SMALL[1], WAIT_SMALL[0], WAIT_SMALL[1], DONE, b"Reading 0123456789%", b"Checking", b" "];
        for s in ui {
            for &c in s {
                assert!(UI_15B.glyph(c as u32).is_some_and(|g| g.adv != 0), "UI_15B lacks {:?}", c as char);
            }
        }
        for s in BIG.iter().chain(WAIT_BIG.iter()) {
            for &c in *s {
                assert!(NAME_26.glyph(c as u32).is_some_and(|g| g.adv != 0), "NAME_26 lacks {:?}", c as char);
            }
        }
    }

    /// BACKUP-9.4: the bar fills with the read, blue, and turns green with "Read and checked" when the check ends.
    #[test]
    fn bar_follows_the_read_and_the_check() {
        let mut fb = FrameBuffer::new();
        draw(&mut fb);
        let at = |fb: &FrameBuffer, pct: i32| fb.at(BAR_X + BAR_W * pct / 100, BAR_Y + 4);
        assert_eq!(at(&fb, 10), crate::theme::map(LINE));
        progress(&mut fb, Phase::Reading, 1 << 21, 4 << 20);
        assert_eq!(at(&fb, 10), crate::theme::map(BLUE));
        assert_eq!(at(&fb, 60), crate::theme::map(LINE));
        progress(&mut fb, Phase::Checking, 4 << 20, 4 << 20);
        assert_eq!(at(&fb, 60), crate::theme::map(GREEN));
        // The status line has ink; the big text is above the bar, the warnings below.
        assert!((STATUS_Y - 14..STATUS_Y).any(|y| (0..W).any(|x| fb.at(x, y) != crate::theme::map(BG))));
        assert!((100..123).any(|y| (0..W).any(|x| fb.at(x, y) != crate::theme::map(BG))));
        assert!((250..262).any(|y| (0..W).any(|x| fb.at(x, y) != crate::theme::map(BG))));
    }

    /// BACKUP-9.4: the waiting screen has no bar; the check at less than all stays blue with its percentage; a
    /// total of 0 (nothing to read yet) leaves the screen as it is.
    #[test]
    fn waiting_then_checking() {
        let mut fb = FrameBuffer::new();
        waiting(&mut fb);
        assert_eq!(fb.at(BAR_X + 10, BAR_Y + 4), crate::theme::map(BG), "no bar while waiting");
        draw(&mut fb);
        let before = fb.px.clone();
        progress(&mut fb, Phase::Checking, 5, 0);
        assert!(fb.px == before, "nothing to show yet");
        progress(&mut fb, Phase::Checking, 3, 4);
        assert_eq!(fb.at(BAR_X + BAR_W / 2, BAR_Y + 4), crate::theme::map(BLUE));
        assert_eq!(fb.at(BAR_X + BAR_W - 1, BAR_Y + 4), crate::theme::map(LINE));
    }
}
