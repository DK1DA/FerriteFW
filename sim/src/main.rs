//! Renders Ferrite's screens with the firmware's own drawing code into `target/sim/*.png` (240 x 320, as on the
//! radio), from sample data. `cargo sim`; `cargo sim --scale 2` for bigger images.
//!
//! `cargo simw` opens the interactive simulator instead ([`window`]): a window, PC keys as radio keys, a fake
//! radio, and tools to check the drawing against the look review's mock-ups.

use ferrite_app::App;
use ferrite_core::keys::Key;
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State};
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::panel::{H, W};
use std::path::Path;

mod face;
mod fake;
mod window;

fn save(fb: &FrameBuffer, name: &str, scale: u32) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/sim");
    save_fb(fb, &dir.join(format!("{name}.png")), scale);
}

/// An RGB image (0xRRGGBB per pixel) as a PNG.
pub(crate) fn save_rgb(px: &[u32], w: usize, h: usize, path: &Path) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    let out: Vec<u8> = px.iter().flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).collect();
    let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().unwrap().write_image_data(&out).unwrap();
    println!("{}", path.display());
}

/// The frame buffer as a PNG, scaled up by a whole number.
pub(crate) fn save_fb(fb: &FrameBuffer, path: &Path, scale: u32) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    let rgb = fb.rgb();
    let (w, h) = (W as u32 * scale, H as u32 * scale);
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let i = (((y / scale) * W as u32 + x / scale) * 3) as usize;
            out.extend_from_slice(&rgb[i..i + 3]);
        }
    }
    let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().unwrap().write_image_data(&out).unwrap();
    println!("{}", path.display());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    if args.iter().any(|a| a == "--window") {
        // The window: any scale, fractional too (default 1.5, about 680 px tall); resizable afterwards.
        let scale = arg("--scale").and_then(|s| s.parse::<f32>().ok()).filter(|&s| s > 0.0).unwrap_or(1.5);
        let bus = arg("--bus-mbps").and_then(|s| s.parse().ok());
        window::run(scale, bus);
        return;
    }
    let scale = arg("--scale").and_then(|s| s.parse().ok()).unwrap_or(1);

    let checks = [
        Check { label: "MCU", value: "AT32F403A", state: State::Ok },
        Check { label: "SPI flash", value: "4 MB", state: State::Ok },
        Check { label: "BK4829 VHF", value: "0x4829", state: State::Ok },
        Check { label: "BK4829 UHF", value: "0x4829", state: State::Ok },
        Check { label: "SI4732", value: "", state: State::Pending },
    ];
    let info = BootInfo { version: VERSION, callsign: Some("VA7FER"), checks: &checks };
    let mut fb = FrameBuffer::new();
    boot::draw(&mut fb, &info);
    save(&fb, "boot", scale);
    let mut failed = checks;
    failed[1] = Check { label: "SPI flash", value: "no answer", state: State::Fail };
    boot::draw(&mut fb, &BootInfo { checks: &failed, ..info });
    save(&fb, "boot-failed", scale);

    // The app, as in the window: keys pressed at simulated times, the fake radio stepped to match.
    let store = fake::store();
    let mut radio = fake::FakeRadio::new();
    let mut dev = fake::device();
    dev.clock_min = Some(14 * 60 + 32);
    dev.utc = false;
    let mut app = App::new();
    fake::sample(&mut app, &store);
    let mut t = 0u32;
    let mut run = |app: &mut App, fb: &mut FrameBuffer, until: u32, t: &mut u32| {
        while *t <= until {
            while let Some(c) = app.pop_cmd() {
                radio.cmd(c);
            }
            radio.step(*t as f32 / 1000.0, app.active());
            app.tick(*t, &radio.live, &store);
            app.render(fb, &radio.live, &dev, &store);
            *t += 20;
        }
    };
    let press = |app: &mut App, key: Key, hold: u32, t: &mut u32| {
        app.key_down(key, *t, &store);
        *t += hold;
        app.tick(*t, &Default::default(), &store); // long presses fire while held
        app.key_up(key, *t, &store);
    };
    run(&mut app, &mut fb, 7000, &mut t); // slot A receiving (6-9 s of every 10)
    save(&fb, "home", scale);
    // The whole radio, as the window shows it: the front panel around the screen, SIDE1 held, A receiving.
    let face = face::Face::new(2);
    let mut px = vec![0u32; face.w * face.h];
    let side1 = face.find(Key::Side(1)).into_iter().collect::<Vec<_>>();
    face.draw(&mut px, &side1, 0, true, false, 2);
    face.put_screen(&mut px, 2, |x, y| window::rgb565_to_rgb(fb.at(x, y)));
    save_rgb(&px, face.w, face.h, &Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/sim/radio.png"));
    press(&mut app, Key::Slot, 800, &mut t); // hold SLOT: edit in place
    press(&mut app, Key::Right, 100, &mut t);
    press(&mut app, Key::Right, 100, &mut t);
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "home-edit", scale);
    press(&mut app, Key::Exit, 100, &mut t);
    press(&mut app, Key::Menu, 100, &mut t); // quick settings
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "menu-quick", scale);
    press(&mut app, Key::Digit(0), 100, &mut t); // advanced
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "menu-top", scale);
    press(&mut app, Key::Exit, 100, &mut t);
    press(&mut app, Key::Exit, 100, &mut t);
    press(&mut app, Key::Hash, 100, &mut t); // Status page
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "status", scale);
    press(&mut app, Key::Exit, 100, &mut t);
    app.notify(b"VE3KPW-9", b"On my way", false);
    run(&mut app, &mut fb, t + 100, &mut t);
    save(&fb, "toast", scale);
    // A screen not built yet: hold 3 (band scope) opens its placeholder.
    run(&mut app, &mut fb, t + 3500, &mut t);
    press(&mut app, Key::Digit(3), 800, &mut t);
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "coming", scale);
}
