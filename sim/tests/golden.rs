//! Screenshot regression: every screen the simulator's jump list offers, and home in each layout and theme,
//! drawn by the firmware's own UI code at 240 x 320 and compared pixel for pixel with the goldens in
//! `sim/golden/`. Each screen starts from a fresh radio at a fixed time, so the fake radio's traffic and the clock
//! are the same on every run.
//!
//! A difference fails the test and writes `target/golden-diff/<name>.png`: the new screen dimmed, with the
//! changed pixels in magenta. When a change is meant, bless it:
//!
//! ```text
//! FERRITE_BLESS=1 cargo test -p ferrite-sim --test golden
//! ```
//!
//! which rewrites the goldens that changed (and adds new ones); review them in the commit like any other change.

mod common;

use common::Radio;
use ferrite_sim::simcore::{jump_names, Event};
use std::path::{Path, PathBuf};

const W: usize = 240;
const H: usize = 320;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("golden")
}

fn diff_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/golden-diff")
}

fn bless() -> bool {
    std::env::var_os("FERRITE_BLESS").is_some_and(|v| v != "0")
}

/// A name for a file: "D6 Status page" -> "d6-status-page".
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn write_png(path: &Path, px: &[u32]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data: Vec<u8> = px.iter().flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).collect();
    let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), W as u32, H as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_compression(png::Compression::Best);
    enc.write_header().unwrap().write_image_data(&data).unwrap();
}

fn read_png(path: &Path) -> Option<Vec<u32>> {
    let dec = png::Decoder::new(std::fs::File::open(path).ok()?);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    assert_eq!((info.width as usize, info.height as usize, info.color_type), (W, H, png::ColorType::Rgb), "{}", path.display());
    Some(buf[..info.buffer_size()].chunks(3).map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect())
}

/// Compares `px` with the golden `name`; returns a failure line, or None. Blessing writes the golden instead.
fn check(name: &str, px: &[u32]) -> Option<String> {
    let path = golden_dir().join(format!("{name}.png"));
    let old = read_png(&path);
    if old.as_deref() == Some(px) {
        return None;
    }
    if bless() {
        write_png(&path, px);
        println!("blessed {name}");
        return None;
    }
    let Some(old) = old else {
        return Some(format!("{name}: no golden (run with FERRITE_BLESS=1 to add it)"));
    };
    let changed = old.iter().zip(px).filter(|(a, b)| a != b).count();
    let diff: Vec<u32> = old.iter().zip(px).map(|(&a, &b)| if a != b { 0xFF00FF } else { (b >> 2) & 0x3F3F3F }).collect();
    let out = diff_dir().join(format!("{name}.png"));
    write_png(&out, &diff);
    write_png(&diff_dir().join(format!("{name}.new.png")), px);
    Some(format!("{name}: {changed} pixels differ (see {})", out.display()))
}

/// A screen of the jump list on a fresh radio, a second after it opened.
fn shot_jump(i: usize) -> Vec<u32> {
    let mut r = Radio::new();
    r.sim.jump(i);
    r.wait(1000);
    r.sim.screen_rgb()
}

/// Home in layout `layout` (0-2) and theme `theme` (0 dark, 1 daylight, 2 night red).
fn shot_home(layout: u8, theme: u8) -> Vec<u32> {
    let mut r = Radio::new();
    r.sim.app_mut().settings_mut().theme = theme;
    for _ in 0..layout {
        r.event(Event::Layout);
    }
    r.home();
    r.wait(1000);
    r.sim.screen_rgb()
}

/// Every screen as reviewed, pixel for pixel: the goldens hold the approved look, so a change to the fonts and
/// sizes (DISP-6), the corners (DISP-19) or the centring of text in boxes (DISP-20) fails here until it is
/// reviewed and blessed.
#[test]
fn every_screen_matches_its_golden() {
    let names: Vec<String> = jump_names().collect();
    let mut failures = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let name = format!("{:03}-{}", i, slug(n));
        failures.extend(check(&name, &shot_jump(i)));
    }
    assert!(failures.is_empty(), "{} of {} screens changed:\n{}", failures.len(), names.len(), failures.join("\n"));
}

#[test]
fn home_layouts_and_themes_match_their_goldens() {
    let mut failures = Vec::new();
    for layout in 0..3u8 {
        for (theme, tname) in ["dark", "daylight", "night"].iter().enumerate() {
            let name = format!("home-layout{layout}-{tname}");
            failures.extend(check(&name, &shot_home(layout, theme as u8)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// HELP-1 to HELP-5: About's Help pages, ▶ by ▶ from About: the manual's QR code and the key card pages.
#[test]
fn help_pages_match_their_goldens() {
    let about = jump_names().position(|n| n.ends_with("About")).expect("About in the jump list");
    let mut failures = Vec::new();
    for (i, name) in ["manual", "home-keys", "side-keys", "shortcuts", "menu-numbers"].iter().enumerate() {
        let mut r = Radio::new();
        r.sim.jump(about);
        r.wait(200);
        for _ in 0..=i {
            r.press(ferrite_core::keys::Key::Right);
        }
        r.wait(1000);
        failures.extend(check(&format!("help-{name}"), &r.sim.screen_rgb()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// TSRCH-13, TSRCH-16: the tone hint on the active slot's card (the T event: a station sending 88.5 where the
/// slot has 100.0), and quick settings' Tones row offering it.
#[test]
fn tone_hint_matches_its_goldens() {
    let mut r = Radio::new();
    r.home();
    r.event(Event::ToneHint);
    r.wait(1000);
    let mut failures = Vec::new();
    failures.extend(check("tone-hint-home", &r.sim.screen_rgb()));
    r.press(ferrite_core::keys::Key::Menu);
    r.press(ferrite_core::keys::Key::Digit(4));
    r.wait(1000);
    failures.extend(check("tone-hint-quick", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BCR-26 to BCR-28: slot A tuned to 100.5 MHz plays broadcast FM: its card with the full radio text, stereo and
/// SNR; 99.1 (no radio text: the station name); slot B tuned in too takes it ("FM on B" on A); the FM screen, quick
/// settings and the other two layouts with the broadcast slot.
#[test]
fn broadcast_slot_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let mut failures = Vec::new();
    let mut r = Radio::new();
    r.home();
    r.event(Event::FmStation);
    r.wait(3000);
    failures.extend(check("bcast-home", &r.sim.screen_rgb()));
    r.press(Key::Menu).wait(1000);
    failures.extend(check("bcast-quick", &r.sim.screen_rgb()));
    r.home();
    r.menu("0 3 1").wait(3000);
    failures.extend(check("bcast-fm-screen", &r.sim.screen_rgb()));
    r.home();
    r.event(Event::FmStation); // 99.1: the station name only
    r.wait(3000);
    failures.extend(check("bcast-home-ps", &r.sim.screen_rgb()));
    r.press(Key::Slot);
    r.event(Event::FmStation); // B to 100.5: the broadcast moves to B
    r.wait(3000);
    failures.extend(check("bcast-home-handover", &r.sim.screen_rgb()));
    for l in 1..3 {
        r.event(Event::Layout);
        r.home().wait(1000);
        failures.extend(check(&format!("bcast-layout{l}"), &r.sim.screen_rgb()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// MODE-5 (decision 115): single + scope with B playing broadcast and A active: the strip sweeps around A.
#[test]
fn strip_beside_broadcast_matches_its_golden() {
    use ferrite_core::keys::Key;
    let mut r = Radio::new();
    r.event(Event::Layout);
    r.press(Key::Slot).event(Event::FmStation).wait(1000);
    r.press(Key::Slot).press(Key::Slot).wait(2000);
    if let Some(f) = check("bcast-strip", &r.sim.screen_rgb()) {
        panic!("{f}");
    }
}

/// HOME-15: quick mute's state word in each layout: A (active, the large card) muted with \*, and B muted while
/// its fake carrier is on ("RX muted" on a compact card or line).
#[test]
fn quick_mute_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let mut failures = Vec::new();
    for l in 0..3u8 {
        let mut r = Radio::new();
        for _ in 0..l {
            r.event(Event::Layout);
        }
        r.home();
        r.press(Key::Slot).press(Key::Star).press(Key::Slot).press(Key::Slot).press(Key::Star);
        // B's fake carrier: 20-22 s of every 25.
        while !r.sim.radio().live.slots[1].squelch_open {
            r.wait(20);
        }
        r.wait(1000);
        failures.extend(check(&format!("mute-layout{l}"), &r.sim.screen_rgb()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// HOME-17: the quick menu over home (hold *), in dark and in night red (PTT B in the menu turns it on).
#[test]
fn quick_menu_matches_its_goldens() {
    use ferrite_core::keys::{Key, PTT_B};
    let mut r = Radio::new();
    r.home();
    r.hold(Key::Star).wait(300);
    let mut failures: Vec<String> = check("quick-menu", &r.sim.screen_rgb()).into_iter().collect();
    r.press(Key::Ptt(PTT_B)).wait(300);
    failures.extend(check("quick-menu-night-red", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The same screen drawn twice from scratch is the same: the goldens can only change when the code does.
#[test]
fn shots_are_deterministic() {
    assert_eq!(shot_jump(0), shot_jump(0));
    let n = jump_names().count();
    assert_eq!(shot_jump(n / 2), shot_jump(n / 2));
}

/// CHAN-27 to CHAN-30: the zones page with location (B active on Repeaters: its chip, the places in the preview),
/// on the Nearby row (its preview), and with the GPS off and no fixed position (the plain list, no places).
#[test]
fn zones_page_matches_its_goldens() {
    use ferrite_core::keys::Key;
    let mut failures = Vec::new();
    let mut r = Radio::new();
    r.home();
    r.press(Key::Slot); // B, on "Club repeater" in Repeaters
    r.menu("0 2 3").press(Key::Down).wait(1000);
    failures.extend(check("zones-located", &r.sim.screen_rgb()));
    r.press(Key::Up).press(Key::Up).wait(1000);
    failures.extend(check("zones-nearby", &r.sim.screen_rgb()));
    r.sim.app_mut().settings_mut().gps_on = false; // no fix, no fixed position: no location anywhere
    r.home();
    r.menu("0 2 3").press(Key::Down).wait(1000);
    failures.extend(check("zones-no-fix", &r.sim.screen_rgb()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
