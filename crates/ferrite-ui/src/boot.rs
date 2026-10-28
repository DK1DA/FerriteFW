//! The boot screen: logo, wordmark, version and callsign, nothing else while the start-up is quick (BOOT-9,
//! decision 215: no progress bar, no "Self-test n of m"); a step that takes long gets one line naming it ([`BootInfo::note`]).
//! A failed check shows on the same screen, in red, naming the check; the radio then waits for a key (BOOT-10).
//! With the boot screen set to the owner card (MENU 0 7 3 3, SETUP-11), the card fills the screen instead and
//! the wordmark, version and self-test shrink to its bottom line (BootC mock-up, less its small logo); a failed
//! check still shows the logo screen, as the failure matters more.
use crate::assets::*;
use crate::gfx::{Align, Gfx, BG, BLUE, LINE, RED, SUB, TEXT};
use crate::panel::Panel;
use crate::text::Buf;
use crate::v;
use crate::widgets::{fit, hint_bar};

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

/// The owner card's lines (SETUP-11): user data, any bytes. Characters the fonts lack draw as '?', and a line
/// too long for the screen is cut.
#[derive(Clone, Copy, Default)]
pub struct Owner<'a> {
    pub name: &'a [u8],
    pub phone: &'a [u8],
    pub email: &'a [u8],
}

pub struct BootInfo<'a> {
    pub version: &'a str,
    /// The owner's callsign, on the owner card; None until one is set.
    pub callsign: Option<&'a [u8]>,
    /// The logo screen's line under the version: Power-on message (0 7 3 2) gives the callsign, a text or None.
    pub line: Option<&'a [u8]>,
    /// The owner card, when the boot screen is set to it (MENU 0 7 3 3); None: the logo.
    pub owner: Option<Owner<'a>>,
    pub checks: &'a [Check],
    /// A slow step's line ("Checking GPS…", "Installing SSB patch"); None: the status line stays empty.
    pub note: Option<&'a [u8]>,
}

/// The text column's width, 12 px margins.
const COL: i32 = 216;

/// The callsign in capitals (SETUP-12), at most 16 characters.
fn upper(call: &[u8], out: &mut [u8; 16]) -> usize {
    let n = call.len().min(out.len());
    for (d, s) in out.iter_mut().zip(call) {
        *d = s.to_ascii_uppercase();
    }
    n
}

/// Full screen.
pub fn draw(lcd: &mut impl Panel, b: &BootInfo) {
    paint(lcd, b, true)
}

/// Only what the self-test changes: the bar and the status line (the logo, wordmark and card stay). Redrawing the
/// whole screen per check flashed the logo on the radio (Oct 12, 2026). Call after [`draw`], with the same
/// owner/failed layout (a failure switches from the card to the logo screen: [`draw`] again then).
pub fn update(lcd: &mut impl Panel, b: &BootInfo) {
    paint(lcd, b, false)
}

/// Whether `b` shows the owner card (the logo screen otherwise): a change needs [`draw`].
pub fn shows_card(b: &BootInfo) -> bool {
    b.owner.is_some() && !b.checks.iter().any(|c| c.state == State::Fail)
}

fn paint(lcd: &mut impl Panel, b: &BootInfo, full: bool) {
    let n = b.checks.len() as i32;
    let done = b.checks.iter().filter(|c| c.state != State::Pending).count() as i32;
    let failed = b.checks.iter().find(|c| c.state == State::Fail);
    if let (Some(o), None) = (b.owner, failed) {
        return card(lcd, b, &o, done, n, full);
    }
    if full {
        lcd.fill(0, 0, 240, 320, BG);
        lcd.mask4(64, 28, &LOGO112_RING, SUB, Some((&LOGO112_WIND, BLUE)), BG);
        lcd.text4_aligned(120, 188, Align::Center, b"FERRITE", &WORD_40, TEXT, BG);
        lcd.text4_aligned(120, 209, Align::Center, b.version.as_bytes(), &UI_12, SUB, BG);
        if let Some(l) = b.line {
            lcd.text4_aligned(120, 243, Align::Center, fit(l, &UI_15B, COL), &UI_15B, TEXT, BG);
        }
    }
    let _ = (done, n);
    let mut line = Buf::<31>::new();
    let (text, col): (&[u8], u16) = if let Some(c) = failed {
        (v!(line, "{}: {}", c.label, if c.value.is_empty() { "failed" } else { c.value }), RED)
    } else {
        (b.note.unwrap_or(b""), SUB)
    };
    // Each line paints its own full width (no blank first).
    crate::widgets::field(lcd, 0, 240, 276, 298, 290, Align::Center, text, &UI_12, col, BG);
    let more: &[u8] = if failed.is_some() { b"Any key: continue" } else { b"" };
    crate::widgets::field(lcd, 0, 240, 298, 320, 308, Align::Center, more, &UI_12, SUB, BG);
}

/// The owner card (D13, BootC mock-up): who owns the radio and how to reach them, with the logo, version and
/// self-test in one line at the bottom. Any key skips it (the firmware's UI task).
fn card(lcd: &mut impl Panel, b: &BootInfo, o: &Owner, done: i32, n: i32, full: bool) {
    if full {
        card_static(lcd, b, o);
    }
    // The bottom line: wordmark and version, the self-test on the right (the version gives way); the bar.
    let x = 12 + crate::gfx::text4_width(b"FERRITE", &UI_12B);
    // A slow step's note on the right (the version gives way); nothing while the start-up is quick (decision 215).
    let _ = (done, n);
    let status: &[u8] = b.note.unwrap_or(b"");
    let right = 228 - crate::gfx::text4_width(status, &UI_12);
    crate::widgets::field(lcd, x + 6, 222 - x, 265, 281, 277, Align::Left, fit(b.version.as_bytes(), &UI_12, right - x - 8), &UI_12, SUB, BG);
    crate::widgets::field(lcd, right, 228 - right, 265, 281, 277, Align::Left, status, &UI_12, SUB, BG);
}

/// The card's fixed part: drawn once.
fn card_static(lcd: &mut impl Panel, b: &BootInfo, o: &Owner) {
    lcd.fill(0, 0, 240, 320, BG);
    lcd.text4(12, 32, b"THIS RADIO BELONGS TO", &UI_12, SUB, BG);
    if let Some(call) = b.callsign {
        let mut up = [0u8; 16];
        let k = upper(call, &mut up);
        lcd.text4(12, 66, fit(&up[..k], &NAME_26, COL), &NAME_26, TEXT, BG); // 13 characters fit (decision 220)
    }
    lcd.text4(12, 93, fit(o.name, &UI_12B, COL), &UI_12B, TEXT, BG);
    if !o.phone.is_empty() || !o.email.is_empty() {
        lcd.fill(12, 113, COL, 1, LINE);
        lcd.text4(12, 137, b"IF FOUND, PLEASE CONTACT", &UI_12, SUB, BG);
        let mut y = 155;
        for line in [o.phone, o.email].into_iter().filter(|l| !l.is_empty()) {
            lcd.text4(12, y, fit(line, &UI_12B, COL), &UI_12B, TEXT, BG);
            y += 17;
        }
    }
    lcd.text4(12, 277, b"FERRITE", &UI_12B, TEXT, BG);
    hint_bar(lcd, &[("Any key", "skip"), ("", "Set in the web app")]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::FrameBuffer;

    fn count(fb: &FrameBuffer, y0: i32, y1: i32, c: u16) -> usize {
        (y0..y1).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| fb.at(x, y) == c).count()
    }

    fn info<'a>(owner: Option<Owner<'a>>, checks: &'a [Check]) -> BootInfo<'a> {
        BootInfo { version: "v0.9.0", callsign: Some(b"va7fer"), line: Some(b"Hello"), owner, checks, note: None }
    }

    /// BOOT-10: a failed check names itself in red ("failed" when it has no value of its own) with "Any key:
    /// continue" under it, on the logo screen even when the owner card is set.
    #[test]
    fn a_failed_check_shows_in_red_on_the_logo_screen() {
        let owner = Owner { name: b"Alex", phone: b"", email: b"" };
        let bad = [Check { label: "Flash", value: "", state: State::Fail }];
        let b = info(Some(owner), &bad);
        assert!(!shows_card(&b));
        let mut fb = FrameBuffer::new();
        draw(&mut fb, &b);
        assert!(count(&fb, 276, 298, RED) > 50, "the check's line in red");
        assert!((298..320).flat_map(|y| (0..240).map(move |x| (x, y))).any(|(x, y)| fb.at(x, y) != BG), "any key: continue");
        assert!(count(&fb, 28, 140, BLUE) > 100, "the logo");
    }

    /// SETUP-11: the owner card with only a name has no contact block (no rule, no "If found"); with a phone or an
    /// email it has. The callsign shows in capitals.
    #[test]
    fn the_owner_card_shows_contact_lines_only_when_set() {
        let mut fb = FrameBuffer::new();
        let ok = [Check { label: "Flash", value: "", state: State::Ok }];
        let b = info(Some(Owner { name: b"Alex", phone: b"", email: b"" }), &ok);
        assert!(shows_card(&b));
        draw(&mut fb, &b);
        assert_eq!(count(&fb, 113, 114, LINE), 0);
        assert_eq!(count(&fb, 130, 190, TEXT) + count(&fb, 130, 190, SUB), 0, "no contact block");
        draw(&mut fb, &info(Some(Owner { name: b"Alex", phone: b"", email: b"a@example.ca" }), &ok));
        assert_eq!(count(&fb, 113, 114, LINE), COL as usize, "the rule over the contact block");
        assert!(count(&fb, 150, 170, TEXT) > 50, "the email on the first contact line");
        let mut up = [0u8; 16];
        let n = upper(b"va7fer-9", &mut up);
        assert_eq!(&up[..n], b"VA7FER-9");
    }

    /// BOOT-9: an update repaints only the status line (the logo stays as drawn: redrawing it flashed on the radio).
    #[test]
    fn an_update_leaves_the_logo_alone() {
        let mut fb = FrameBuffer::new();
        let pending = [Check { label: "GPS", value: "", state: State::Pending }];
        let b = BootInfo { note: Some(b"Checking GPS"), ..info(None, &pending) };
        draw(&mut fb, &b);
        let before = fb.writes.clone();
        update(&mut fb, &BootInfo { note: None, ..b });
        for y in 0..270 {
            for x in 0..240 {
                let i = (y * 240 + x) as usize;
                assert_eq!(fb.writes[i], before[i], "({x}, {y}) written again");
            }
        }
        assert_eq!(count(&fb, 276, 298, SUB), 0, "the note cleared");
    }
}
