//! Help inside About (MENU 0 9 8; chapter 36, HELP-1 to HELP-5): the pages ◀▶ reach, the manual's QR code, and the
//! key card's rows fitting one line each.

use super::T;
use crate::screens::help::{Help, ACTION_W, ACTION_X, HOLD_KEYS, HOME_KEYS, MENU_NOS, SIDE_KEYS};
use crate::screens::info::{Info, Page};
use crate::screens::nav::Dest;
use crate::screens::shortcut::SHORTCUTS;
use crate::Screen;
use ferrite_core::keys::Key;
use ferrite_ui::assets::{UI_12, UI_12B};
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::gfx::{text4_width, BG, TEXT};
use ferrite_ui::theme::map;
use ferrite_ui::qr::{MANUAL_QR, MANUAL_URL, QR_SIZE};
use ferrite_ui::widgets::{hint_width, key_label};

fn page(t: &T) -> Page {
    match t.app.screen() {
        Screen::Info(Info { page }) => page,
        s => panic!("not on About: {s:?}"),
    }
}

/// HELP-1, SELF-9: ▶ from About goes through System and the Help pages to the self-test and round to About; ◀ goes back.
#[test]
fn about_pages_go_round() {
    let mut t = T::new();
    t.open(Dest::About);
    let order = [
        Page::System,
        Page::Help(Help::Manual),
        Page::Help(Help::HomeKeys),
        Page::Help(Help::SideKeys),
        Page::Help(Help::Digits),
        Page::Help(Help::MenuNos),
        Page::Checks,
        Page::About,
    ];
    for p in order {
        // the hint bar names the page ▶ goes to (its status bar name or its title)
        let h = t.app.screen().hints(&t.app);
        let (key, label) = h.as_slice()[0];
        t.tap(Key::Right);
        assert_eq!(page(&t), p);
        let name = t.app.screen().name();
        // the help pages' hint names #'s next page instead (HOME-10)
        assert!(key == "#" || label == name || label == crate::screens::short_title(name), "{p:?}: the hint said {label:?}, the page is {name:?}");
    }
    t.tap(Key::Left);
    assert_eq!(page(&t), Page::Checks, "◀ goes back");
    t.tap(Key::Exit);
    assert!(!matches!(t.app.screen(), Screen::Info(_)), "BACK leaves");
}

/// HELP-1: every page draws, and its hint bar (naming the next page) fits.
#[test]
fn help_pages_draw_and_hints_fit() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.open(Dest::About);
    for _ in 0..8 {
        t.tap(Key::Right);
        t.render(&mut fb);
        let lit = (21..300).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| fb.at(x, y) != ferrite_ui::gfx::BG).count();
        assert!(lit > 1000, "{:?} drew its content", page(&t));
        let name = t.app.screen().name();
        assert!(crate::screens::short_title(name).len() <= 8, "{name}: status bar name over 8 characters (SBAR-5)");
        let h = t.app.screen().hints(&t.app);
        assert!(hint_width(&mut fb, h.as_slice()) <= 224, "{:?}: hint bar overflows", page(&t));
    }
}

/// HELP-2: the QR code is drawn in the theme's text colour on its background with its quiet zone, and the address shows as
/// text; the matrix is the generated one (finder patterns in three corners).
#[test]
fn manual_qr_is_scannable() {
    let n = QR_SIZE as usize;
    let dark = |x: usize, y: usize| MANUAL_QR[y] >> (n - 1 - x) & 1 != 0;
    for (cx, cy) in [(0, 0), (n - 7, 0), (0, n - 7)] {
        for i in 0..7 {
            assert!(dark(cx + i, cy) && dark(cx + i, cy + 6) && dark(cx, cy + i) && dark(cx + 6, cy + i), "finder at {cx},{cy}");
        }
        assert!(dark(cx + 3, cy + 3), "finder centre");
        assert!(!dark(cx + 1, cy + 1), "finder ring");
    }
    assert!(MANUAL_URL.starts_with("https://"));
    assert!(text4_width(MANUAL_URL.as_bytes(), &UI_12) <= 224, "the address fits one line under the code");
    for theme in 0..3 {
        let mut t = T::new();
        t.app.settings_mut().theme = theme;
        let mut fb = FrameBuffer::new();
        t.open(Dest::About);
        t.tap(Key::Right); // System (SELF-9)
        t.tap(Key::Right);
        t.render(&mut fb);
        let side = (QR_SIZE + 8) * 5;
        let (x0, y0) = ((240 - side) / 2, 30);
        for i in 0..side {
            for (x, y) in [(x0 + i, y0), (x0 + i, y0 + side - 1), (x0, y0 + i), (x0 + side - 1, y0 + i)] {
                assert_eq!(fb.at(x, y), map(BG), "quiet zone is the background at {x},{y} (theme {theme})");
            }
        }
        // The top-left finder's corner module, 4 modules in from the quiet zone's edge.
        assert_eq!(fb.at(x0 + 20 + 2, y0 + 20 + 2), map(TEXT), "dark module in the text colour (theme {theme})");
        assert_ne!(map(TEXT), map(BG));
    }
}

/// HELP-3, HELP-5: every key card row fits its columns on one line at 240 px (texts shortened, never wrapped).
#[test]
fn key_rows_fit_one_line() {
    let mut fb = FrameBuffer::new();
    for (key, action) in HOME_KEYS.iter().chain(SIDE_KEYS.iter()).chain(MENU_NOS.iter()) {
        if key.is_empty() {
            assert!(text4_width(action.as_bytes(), &UI_12B) <= 224, "heading {action}");
            continue;
        }
        assert!(key_label(&mut fb, 0, 0, key, "", 0, false) <= ACTION_X - 8 - 4, "key {key:?} reaches the action column");
        assert!(text4_width(action.as_bytes(), &UI_12) <= ACTION_W, "{key:?}: {action} is too long");
    }
}

/// HELP-1, HELP-4: the Holds page lists the hold digits from the shortcut list's own table (MENU 0 9 1 3, KEY-21),
/// then hold * and hold #, one row each; its name fits the status bar unshortened.
#[test]
fn help_digits_are_the_shortcut_table() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.open(Dest::About);
    for _ in 0..5 {
        t.tap(Key::Right); // System (SELF-9), then the Help pages
    }
    assert_eq!(page(&t), Page::Help(Help::Digits));
    t.render(&mut fb);
    for s in SHORTCUTS {
        assert!(text4_width(s.as_bytes(), &UI_12) <= ACTION_W, "{s} fits");
    }
    assert_eq!(SHORTCUTS.len(), 10);
    assert_eq!(HOLD_KEYS.map(|r| r.0), ["ABC", "V/M", "MENU", "BACK", "*", "#"], "the key holds (decision 376)");
    for (_, a) in HOLD_KEYS {
        assert!(text4_width(a.as_bytes(), &UI_12) <= ACTION_W, "{a} fits");
    }
    assert_eq!(crate::screens::short_title("Holds"), "Holds");
}


/// HOME-10 (decision 376): # on home opens the quick help on the holds; # goes on through the help pages and
/// round (holds, home keys, side keys, menu numbers, the manual), the hint bar naming the next; BACK goes home.
/// While a watched slot receives, # still pauses watch (WATCH-6). HOME-10.1: home's hint bar starts with Hold ABC
/// Edit (the SLOT key, as marked).
#[test]
fn hash_is_the_quick_help() {
    let mut t = T::new();
    assert_eq!(t.app.screen().hints(&t.app).as_slice()[0], ("Hold ABC", "Edit"));
    t.tap(Key::Hash);
    assert_eq!(page(&t), Page::Help(Help::Digits), "the holds first");
    for p in [Help::HomeKeys, Help::SideKeys, Help::MenuNos, Help::Manual, Help::Digits] {
        let h = t.app.screen().hints(&t.app);
        assert_eq!(h.as_slice()[0].0, "#");
        t.tap(Key::Hash);
        assert_eq!(page(&t), Page::Help(p));
    }
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), crate::Screen::Home, "BACK: home");
}
