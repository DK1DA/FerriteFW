//! The interactive simulator: a window running the firmware's own UI (`ferrite-app` drawing through
//! `ferrite-ui`), driven by the PC keyboard and a fake three-slot radio. Every pixel comes through [`Panel`],
//! exactly as on the radio.
//!
//! Tools for checking the drawing code against the look review:
//! - F2 flashes each region as it is redrawn (only what changed should flash, HOME-7);
//! - F3 blends the current screen's mock-up (`docs/img/screens/*.png`) over it;
//! - the title shows the pixels pushed by the last redraw and, with `--bus-mbps`, the time it would take on the
//!   radio's display bus;
//! - F12 saves a screenshot of the whole window to `target/sim/`.
//!
//! The window is a thin shell over [`ferrite_sim::simcore::SimCore`], which the web page (`sim-web`) shares.
//!
//! The front panel ([`ferrite_sim::face`]) is clickable: mouse down and up are key down and up, so holds and long
//! presses work; the channel knob steps on a click (left half down, right half up) or the wheel. Pressed keys
//! light up whichever way they were pressed. The indicator is green while the active slot receives and red while
//! a PTT is held (a fake transmit: the app has no transmit yet).
//!
//! Stand-ins for what the radio would report: M a message arrives, N an alert, F5 GPS off/searching/fix,
//! F6 Bluetooth, F7 battery normal/low/cut-off, F8 the power switch, F9 an empty battery, F10 an unknown SSB
//! patch at boot, F11 a write from a new Bluetooth device (allowed, it runs a fake write session). Each calls
//! the App hook the firmware calls.

use ferrite_sim::face::Face;
use ferrite_sim::simcore::{fit_scale, to_panel, Event, SimCore, KEYMAP, MAX_SCALE};
use ferrite_ui::panel::{H, W};
use minifb::{Key as K, KeyRepeat, MouseButton, MouseMode, ScaleMode, Window, WindowOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ferrite_core::keys::Key;

fn map_key(k: K) -> Option<Key> {
    Some(match k {
        K::Up => Key::Up,
        K::Down => Key::Down,
        K::Left => Key::Left,
        K::Right => Key::Right,
        K::Enter => Key::Menu,
        K::Escape => Key::Exit,
        K::S => Key::Slot,
        K::V => Key::VfoMem,
        K::Q => Key::Side(1),
        K::W => Key::Side(2),
        K::Space => Key::Ptt(0),
        K::Comma => Key::Star,
        K::Period => Key::Hash,
        K::Key0 | K::NumPad0 => Key::Digit(0),
        K::Key1 | K::NumPad1 => Key::Digit(1),
        K::Key2 | K::NumPad2 => Key::Digit(2),
        K::Key3 | K::NumPad3 => Key::Digit(3),
        K::Key4 | K::NumPad4 => Key::Digit(4),
        K::Key5 | K::NumPad5 => Key::Digit(5),
        K::Key6 | K::NumPad6 => Key::Digit(6),
        K::Key7 | K::NumPad7 => Key::Digit(7),
        K::Key8 | K::NumPad8 => Key::Digit(8),
        K::Key9 | K::NumPad9 => Key::Digit(9),
        _ => return None,
    })
}

/// The simulator's event keys.
fn map_event(k: K) -> Option<Event> {
    Some(match k {
        K::M => Event::Message,
        K::N => Event::Alert,
        K::F5 => Event::Gps,
        K::F6 => Event::Bluetooth,
        K::F7 => Event::Battery,
        K::F8 => Event::PowerSwitch,
        K::F9 => Event::EmptyBattery,
        K::F10 => Event::SsbPatch,
        K::F11 => Event::LinkWrite,
        K::X => Event::Crossband,
        K::L => Event::Layout,
        K::B => Event::Beacon,
        _ => return None,
    })
}

/// A 240 x 320 RGB image from a 2x mock-up PNG (480 x 640), or None.
fn load_mockup(path: &Path) -> Option<Vec<u32>> {
    let dec = png::Decoder::new(std::fs::File::open(path).ok()?);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    let ch = info.color_type.samples();
    let (w, h) = (info.width as i32, info.height as i32);
    let mut out = vec![0u32; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let (sx, sy) = (x * w / W, y * h / H);
            let i = ((sy * w + sx) as usize) * ch;
            out[(y * W + x) as usize] = (buf[i] as u32) << 16 | (buf[i + 1] as u32) << 8 | buf[i + 2] as u32;
        }
    }
    Some(out)
}

/// `scale` is a whole number (1 by default): every LCD pixel becomes scale x scale window pixels, never a fraction,
/// so text looks as it will on the radio. Resizing the window snaps to the largest whole scale that fits.
pub fn run(scale: usize, bus_mbps: Option<f64>, owner_card: bool) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let shots = repo.join("../docs/img/screens");
    let mut sim = SimCore::new(scale, owner_card);
    let utc = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    sim.set_clock(utc);

    let sizes: Vec<(usize, usize)> = (1..=MAX_SCALE).map(Face::size).collect();
    let (mut ww, mut wh) = sim.size();
    let opts = WindowOptions { resize: true, scale_mode: ScaleMode::Center, ..WindowOptions::default() };
    let mut win = Window::new("Ferrite sim", ww, wh, opts).expect("open window");
    // Input is polled every ~2 ms so a quick trackpad tap (down and up inside one 16 ms frame) is never missed;
    // the window is redrawn at 60 Hz.
    win.set_target_fps(0);
    let mut last_present = Instant::now() - Duration::from_secs(1);
    println!("{KEYMAP}");

    let start = Instant::now();
    let ms = || start.elapsed().as_millis() as u32;
    let (mut show_flash, mut overlay) = (false, false);
    let mut mockups: HashMap<&str, Option<Vec<u32>>> = HashMap::new();
    let mut size = win.get_size();
    let mut shot = 0;

    while win.is_open() && !(win.is_key_down(K::LeftCtrl) && win.is_key_down(K::Q)) {
        let now = ms();
        // Keys: down and up go to the app, which makes short, long and repeat presses of them (KEY-3, KEY-4).
        for k in win.get_keys_pressed(KeyRepeat::No) {
            match k {
                K::F2 => show_flash = !show_flash,
                K::F3 => overlay = !overlay,
                K::F12 => {
                    let p: PathBuf = repo.join(format!("target/sim/shot-{shot}.png"));
                    let (w, h) = sim.size();
                    ferrite_sim::save_rgb(sim.compose(show_flash, None), w, h, &p);
                    shot += 1;
                }
                _ => {
                    if let Some(e) = map_event(k) {
                        sim.event(e);
                    } else if let Some(key) = map_key(k) {
                        sim.key(key, true);
                    }
                }
            }
        }
        for k in win.get_keys_released() {
            if let Some(key) = map_key(k) {
                sim.key(key, false);
            }
        }
        // The mouse on the front panel: down and up are the key's down and up.
        let down = win.get_mouse_down(MouseButton::Left);
        let pos = win.get_unscaled_mouse_pos(MouseMode::Pass).map(|m| to_panel(m, win.get_size(), (ww, wh)));
        let (mx, my) = pos.unwrap_or((-1, -1));
        sim.mouse(mx, my, down);
        // The knob: [ and ] (with the PC's auto-repeat), or the mouse wheel.
        if !sim.booting() {
            for k in win.get_keys_pressed(KeyRepeat::Yes) {
                match k {
                    K::LeftBracket => sim.knob(-1),
                    K::RightBracket => sim.knob(1),
                    _ => {}
                }
            }
            if let Some((_, dy)) = win.get_scroll_wheel() {
                if dy.abs() >= 1.0 {
                    sim.knob(if dy > 0.0 { 1 } else { -1 });
                }
            }
        }
        sim.step(now);

        if last_present.elapsed() < Duration::from_millis(16) {
            win.update();
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        last_present = Instant::now();

        // A resized window: snap to the largest whole scale that fits.
        if win.get_size() != size {
            size = win.get_size();
            sim.set_scale(fit_scale(size, &sizes));
            (ww, wh) = sim.size();
        }

        let mockup = if overlay {
            let name = sim.mockup_name();
            mockups.entry(name).or_insert_with(|| load_mockup(&shots.join(name))).as_deref()
        } else {
            None
        };
        let bus = bus_mbps.map(|m| format!(" ≈ {:.1} ms", sim.last_px() as f64 * 2.0 / (m * 1e6) * 1e3)).unwrap_or_default();
        let title = format!(
            "Ferrite sim · {}{bus}{}{}",
            sim.status(),
            if show_flash { " · F2" } else { "" },
            if overlay { " · F3 mock-up" } else { "" }
        );
        win.update_with_buffer(sim.compose(show_flash, mockup), ww, wh).expect("draw");
        win.set_title(&title);
    }
}
