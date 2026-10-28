//! Themes (0 7 1; manual chapter 31, DISP-9): dark is the token table in [`crate::gfx`] as drawn; daylight and
//! night red change colours where they enter the drawing primitives ([`crate::gfx::Gfx::fill`], `text4_rows`,
//! `mask4`), once per call, never per pixel. Text is blended from the mapped colours, so anti-aliasing stays right.
//!
//! - **Daylight**: the dark tokens swapped for the look-review's daylight palette (HomeADay, MenuQuickDay,
//!   TransmitDay, ScopeDay). A colour that isn't a token passes through (the scope's waterfall stays dark, as drawn).
//! - **Cyberpunk**: the owner's mock-up palette (Oct 15, 2026): deep violet grounds, cyan for the accent and the
//!   frequency, magenta for receive, the meter and the bars' rules, yellow for the battery and slot C. The role
//!   tokens ([`FREQ`], [`KEYS`], [`RULE`], [`BATT`]) take their own colours here and their base token's elsewhere.
//! - **Night red** (DISP-18, strict): red or black only. Grounds, surfaces and card backgrounds are black; text,
//!   digits, lines, outlines and meters full red; a filled highlight (a selected row, the active slot's letter, a
//!   tag) solid red with black text on it ([`ink_on`]). A dim fill (LINE, DIM: an empty meter segment, a track)
//!   is drawn as a red outline, black inside ([`outlined`]). Only colours that aren't tokens (the scope's
//!   spectrum and waterfall gradient) keep red levels, at their own brightness ([`graded`]). Hue can't carry
//!   meaning here; DISP-10 already gives every state a shape and a word.

use crate::gfx::*;

pub const DARK: u8 = 0;
pub const DAYLIGHT: u8 = 1;
pub const NIGHT_RED: u8 = 2;
pub const CYBERPUNK: u8 = 3;

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

/// A colour as the current theme draws it (an ink: text, lines, a mask, a thin or a highlight fill).
#[inline]
pub fn map(c: u16) -> u16 {
    match get() {
        CYBERPUNK => cyber(c),
        DAYLIGHT => day(base(c)),
        NIGHT_RED => night(base(c)),
        _ => base(c),
    }
}

/// A role token as the token it is drawn as outside cyberpunk; other colours as they are.
#[inline]
pub fn base(c: u16) -> u16 {
    match c {
        FREQ | KEYS => TEXT,
        RULE => LINE,
        BATT => GREEN,
        c => c,
    }
}

/// The cyberpunk palette (0 7 1, DISP-18): the owner's mock-up (Oct 15, 2026), colour only, no glows.
pub fn cyber(c: u16) -> u16 {
    const GROUND: u16 = rgb565(0x0A, 0x04, 0x14);
    const CYAN: u16 = rgb565(0x00, 0xF0, 0xFF);
    const MAGENTA: u16 = rgb565(0xFF, 0x2B, 0xD6);
    const YELLOW: u16 = rgb565(0xFF, 0xE6, 0x00);
    match c {
        BG => GROUND,
        SURFACE => rgb565(0x12, 0x08, 0x26),
        LINE => rgb565(0x3A, 0x24, 0x66),
        TEXT => rgb565(0xF2, 0xE9, 0xFF),
        SUB => rgb565(0xB9, 0xA6, 0xD9),
        DIM => rgb565(0x6C, 0x5A, 0x8E),
        BLUE | FREQ | KEYS => CYAN,
        BLUE_FG => GROUND,
        GREEN | RULE | AMBER => MAGENTA,
        VIOLET | BATT => YELLOW,
        RED => rgb565(0xFF, 0x3B, 0x5C),
        FOCUS => rgb565(0x24, 0x10, 0x48),
        TOAST => rgb565(0x1E, 0x0E, 0x3A),
        ALERT_BG => rgb565(0x2A, 0x08, 0x20),
        ALERT_TONE => MAGENTA,
        PASS_FILL => rgb565(0x08, 0x26, 0x2C),
        _ => c,
    }
}

/// A colour as the current theme draws it as a ground behind text or a mask, or a band's background: night red
/// puts LINE and DIM grounds to black, as [`outlined`] fills are inside.
#[inline]
pub fn map_bg(c: u16) -> u16 {
    match get() {
        NIGHT_RED if matches!(c, LINE | DIM) => BLACK,
        _ => map(c),
    }
}

/// Night red's full red, the one colour besides black.
pub const NIGHT_INK: u16 = 0xF800;

/// The ink drawn over a ground already themed: black on night red's solid red (a filled highlight's text), else
/// `fg` as it is.
#[inline]
pub fn ink_on(fg: u16, bg: u16) -> u16 {
    if bg == NIGHT_INK && get() == NIGHT_RED {
        BLACK
    } else {
        fg
    }
}

/// Night red draws a fill of this colour, more than 2 px both ways, as a red outline with black inside (an empty
/// meter segment, a bar's track; DISP-18): the dim fills that separate by shade in the other themes.
#[inline]
pub fn outlined(c: u16) -> bool {
    matches!(c, LINE | DIM) && get() == NIGHT_RED
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

/// Night red (DISP-18): the grounds black, every other token full red; a colour that isn't a token (the scope's
/// gradient) red at its own brightness.
pub fn night(c: u16) -> u16 {
    match c {
        BG | SURFACE | TOAST | BLUE_FG | BLACK | ALERT_BG | PASS_FILL => BLACK,
        LINE | TEXT | SUB | DIM | BLUE | GREEN | AMBER | RED | VIOLET | FOCUS | WHITE | ALERT_TONE => NIGHT_INK,
        _ => graded(c),
    }
}

/// Red at the colour's luminance (Rec. 601 weights), scaled so the brightest text reaches full red: the levels
/// night red keeps for colours that aren't tokens (the scope's spectrum and waterfall).
pub fn graded(c: u16) -> u16 {
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

    /// DISP-18: night red is red or black only: grounds black, every other token full red; only colours that
    /// aren't tokens (the scope's gradient) keep levels, red alone.
    #[test]
    fn night_is_red_or_black() {
        for c in [BG, SURFACE, TOAST, BLUE_FG, BLACK, ALERT_BG, PASS_FILL] {
            assert_eq!(night(c), BLACK, "{c:04x}: a ground is black");
        }
        for c in [LINE, DIM, SUB, TEXT, BLUE, GREEN, AMBER, RED, VIOLET, FOCUS, WHITE, ALERT_TONE] {
            assert_eq!(night(c), NIGHT_INK, "{c:04x}: full red");
        }
        let wf = rgb565(0x1A, 0x55, 0x80); // a waterfall level
        assert_eq!(night(wf) & 0x07FF, 0, "green and blue off");
        assert!(night(wf) > 0 && night(wf) < NIGHT_INK, "the scope's gradient keeps a level");
        set(NIGHT_RED);
        assert_eq!(ink_on(night(TEXT), night(FOCUS)), BLACK, "text on a filled highlight is black");
        assert_eq!(ink_on(night(TEXT), night(SURFACE)), NIGHT_INK);
        assert!(outlined(LINE) && outlined(DIM) && !outlined(SUB));
        assert_eq!(map_bg(LINE), BLACK);
        set(DARK);
        assert!(!outlined(LINE));
        assert_eq!(ink_on(TEXT, NIGHT_INK), TEXT, "only night red blacks it");
    }

    #[test]
    fn daylight_swaps_tokens_and_leaves_the_rest() {
        assert_eq!(day(BG), WHITE);
        assert_eq!(day(TEXT), rgb565(0x0B, 0x0D, 0x10));
        let other = rgb565(0x0F, 0x2A, 0x45); // the scope's waterfall
        assert_eq!(day(other), other);
    }

    /// The role tokens are distinct from every token and draw as their base token outside cyberpunk.
    #[test]
    fn role_tokens() {
        let all = [BG, SURFACE, LINE, TEXT, SUB, BLUE, BLUE_FG, GREEN, AMBER, RED, VIOLET, FOCUS, TOAST, DIM, WHITE, BLACK, ALERT_BG, ALERT_TONE, PASS_FILL];
        for r in [FREQ, KEYS, RULE, BATT] {
            assert!(!all.contains(&r), "{r:04x} is a token's value");
        }
        for (t, f) in [(DARK, (|c| c) as fn(u16) -> u16), (DAYLIGHT, day), (NIGHT_RED, night)] {
            set(t);
            assert_eq!((map(FREQ), map(KEYS), map(RULE), map(BATT)), (f(TEXT), f(TEXT), f(LINE), f(GREEN)), "theme {t}");
        }
        set(CYBERPUNK);
        assert_eq!(map(FREQ), rgb565(0x00, 0xF0, 0xFF));
        assert_eq!(map(RULE), rgb565(0xFF, 0x2B, 0xD6));
        assert_eq!(map(BATT), rgb565(0xFF, 0xE6, 0x00));
        set(DARK);
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

    /// WCAG contrast ratio of two RGB565 colours.
    fn contrast(a: u16, b: u16) -> f32 {
        let lin = |v: u16, bits: u32| {
            let c = v as f32 / ((1u32 << bits) - 1) as f32;
            if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        let lum = |c: u16| 0.2126 * lin(c >> 11, 5) + 0.7152 * lin((c >> 5) & 0x3F, 6) + 0.0722 * lin(c & 0x1F, 5);
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// DISP-8: text is at least 4.5:1 against its ground in the dark, daylight and cyberpunk themes (main and secondary
    /// text, the state colours, text on the selected row and on a toast), and night red draws all text at full
    /// red on black. Greyed text (DIM, a setting that doesn't apply) is an inactive control, outside the rule.
    #[test]
    fn text_contrast() {
        let pairs = [(TEXT, BG), (SUB, BG), (BLUE, BG), (GREEN, BG), (AMBER, BG), (RED, BG), (TEXT, SURFACE), (TEXT, TOAST), (BLUE_FG, BLUE)];
        for (f, g) in pairs {
            assert!(contrast(f, g) >= 4.5, "dark: {f:04x} on {g:04x} is {:.2}", contrast(f, g));
            assert!(contrast(day(f), day(g)) >= 4.5, "daylight: {f:04x} on {g:04x} is {:.2}", contrast(day(f), day(g)));
            assert!(contrast(cyber(f), cyber(g)) >= 4.5, "cyberpunk: {f:04x} on {g:04x} is {:.2}", contrast(cyber(f), cyber(g)));
        }
        for (f, g) in [(FREQ, SURFACE), (KEYS, BG), (VIOLET, SURFACE), (TEXT, FOCUS)] {
            assert!(contrast(cyber(f), cyber(g)) >= 4.5, "cyberpunk: {f:04x} on {g:04x} is {:.2}", contrast(cyber(f), cyber(g)));
        }
        assert_eq!(night(TEXT), NIGHT_INK, "night red: text at full red");
        assert_eq!((night(SUB), night(DIM)), (NIGHT_INK, NIGHT_INK), "night red: secondary text too (DISP-18)");
    }

    /// DISP-8 on the screens' own tints: text on the alert screen's ground and on the crossband "passing" card,
    /// and the alert's tone on its ground, stay readable in every theme, and each theme gives the tints colours of
    /// its own; a colour that isn't a token (the scope's waterfall) passes through cyberpunk as drawn.
    #[test]
    fn the_screens_tints_in_every_theme() {
        for (name, f) in [("dark", (|c| c) as fn(u16) -> u16), ("daylight", day), ("cyberpunk", cyber)] {
            for (fg, g) in [(TEXT, ALERT_BG), (TEXT, PASS_FILL), (TEXT, FOCUS), (SUB, SURFACE)] {
                assert!(contrast(f(fg), f(g)) >= 4.5, "{name}: {fg:04x} on {g:04x} is {:.2}", contrast(f(fg), f(g)));
            }
            assert!(contrast(f(ALERT_TONE), f(ALERT_BG)) >= 3.0, "{name}: the alert's tone");
        }
        for f in [day as fn(u16) -> u16, cyber] {
            assert!([ALERT_BG, ALERT_TONE, PASS_FILL, DIM, LINE, TOAST].iter().all(|&c| f(c) != c), "a colour of its own");
        }
        let other = rgb565(0x0F, 0x2A, 0x45);
        assert_eq!(cyber(other), other);
    }
}
