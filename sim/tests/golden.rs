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

/// The same screen drawn twice from scratch is the same: the goldens can only change when the code does.
#[test]
fn shots_are_deterministic() {
    assert_eq!(shot_jump(0), shot_jump(0));
    let n = jump_names().count();
    assert_eq!(shot_jump(n / 2), shot_jump(n / 2));
}
