//! Themes (0 7 1; manual chapter 31, DISP-9): dark is the token table in [`crate::gfx`] as drawn; daylight and
//! night red change colours where they enter the drawing primitives ([`crate::gfx::Gfx::fill`], `text4_rows`,
//! `mask4`), once per call, never per pixel. Text is blended from the mapped colours, so anti-aliasing stays right.
//!
//! - **Daylight**: the dark tokens swapped for the look-review's daylight palette (HomeADay, MenuQuickDay,
//!   TransmitDay, ScopeDay). A colour that isn't a token passes through (the scope's waterfall stays dark, as drawn).
//! - **Night red**: every colour becomes red at its own brightness (luminance; green and blue off), so dark
//!   adaptation is kept. Hue can't carry meaning here; DISP-10 already gives every state a shape and a word.

use crate::gfx::*;

pub const DARK: u8 = 0;
pub const DAYLIGHT: u8 = 1;
pub const NIGHT_RED: u8 = 2;

// The radio: one global, set and read by the UI task only (never from interrupts). On the PC, one per thread, so
// parallel tests and simulators can't change each other's theme.
#[cfg(not(any(test, feature = "std")))]
static THEME: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(DARK);
#[cfg(any(test, feature = "std"))]
std::thread_local! {
    static THEME: core::cell::Cell<u8> = const { core::cell::Cell::new(DARK) };
}

/// The theme every following draw uses.
pub fn set(t: u8) {
    #[cfg(not(any(test, feature = "std")))]
    THEME.store(t, core::sync::atomic::Ordering::Relaxed);
    #[cfg(any(test, feature = "std"))]
    THEME.with(|c| c.set(t));
}

pub fn get() -> u8 {
    #[cfg(not(any(test, feature = "std")))]
    return THEME.load(core::sync::atomic::Ordering::Relaxed);
    #[cfg(any(test, feature = "std"))]
    THEME.with(|c| c.get())
}

/// A colour as the current theme draws it.
#[inline]
pub fn map(c: u16) -> u16 {
    match get() {
        DAYLIGHT => day(c),
        NIGHT_RED => night(c),
        _ => c,
    }
}

/// The alert screen's dark red ground and the crossband "passing" card's dark green fill (screens' own tints).
const ALERT_BG: u16 = rgb565(0x1A, 0x0E, 0x10);
const ALERT_TONE: u16 = rgb565(0xF2, 0x8B, 0x8E);
const PASS_FILL: u16 = rgb565(0x10, 0x22, 0x16);

/// The daylight palette: high contrast for sun, from the daylight boards.
pub fn day(c: u16) -> u16 {
    match c {
        BG => rgb565(0xFF, 0xFF, 0xFF),
        SURFACE => rgb565(0xEE, 0xF1, 0xF4),
        LINE => rgb565(0xB3, 0xBC, 0xC7),
        TEXT => rgb565(0x0B, 0x0D, 0x10),
        SUB => rgb565(0x3D, 0x46, 0x52),
        DIM => rgb565(0x7A, 0x83, 0x8E),
        BLUE => rgb565(0x00, 0x57, 0xC2),
        BLUE_FG => WHITE,
        GREEN => rgb565(0x0E, 0x7A, 0x2A),
        AMBER => rgb565(0x9A, 0x67, 0x00),
        RED => rgb565(0xC2, 0x1A, 0x1F),
        VIOLET => rgb565(0x6A, 0x3F, 0xD1),
        FOCUS => rgb565(0xE1, 0xEC, 0xFA),
        TOAST => rgb565(0xDC, 0xE7, 0xF5),
        ALERT_BG => rgb565(0xFC, 0xEC, 0xEC),
        ALERT_TONE => rgb565(0xC2, 0x1A, 0x1F),
        PASS_FILL => rgb565(0xE6, 0xF4, 0xEA),
        _ => c,
    }
}

/// Red at the colour's luminance (Rec. 601 weights), scaled so the brightest text reaches full red.
pub fn night(c: u16) -> u16 {
    let r = ((c >> 11) & 31) as u32 * 255 / 31;
    let g = ((c >> 5) & 63) as u32 * 255 / 63;
    let b = (c & 31) as u32 * 255 / 31;
    let l = (77 * r + 150 * g + 29 * b) >> 8;
    let red = (l * 268 / 255).min(255); // TEXT (luminance 243) -> full red
    ((red as u16) >> 3) << 11
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_is_red_only_and_keeps_order() {
        for c in [BG, SURFACE, LINE, DIM, SUB, TEXT, BLUE, GREEN, AMBER, RED, WHITE] {
            assert_eq!(night(c) & 0x07FF, 0, "{c:04x}: green and blue off");
        }
        let lvl = |c| night(c) >> 11;
        assert!(lvl(BG) <= 2, "the ground stays black");
        assert_eq!(lvl(TEXT), 31, "text at full red");
        assert!(lvl(BG) < lvl(LINE) && lvl(LINE) < lvl(DIM) && lvl(DIM) < lvl(SUB) && lvl(SUB) < lvl(TEXT));
        assert!(lvl(RED) < lvl(WHITE), "white on the TX banner stands out from its red fill");
    }

    #[test]
    fn daylight_swaps_tokens_and_leaves_the_rest() {
        assert_eq!(day(BG), WHITE);
        assert_eq!(day(TEXT), rgb565(0x0B, 0x0D, 0x10));
        let other = rgb565(0x0F, 0x2A, 0x45); // the scope's waterfall
        assert_eq!(day(other), other);
    }

    #[test]
    fn the_theme_switches_map() {
        set(NIGHT_RED);
        assert_eq!(map(BG), night(BG));
        set(DAYLIGHT);
        assert_eq!(map(BG), WHITE);
        set(DARK);
        assert_eq!(map(BG), BG);
    }
}
