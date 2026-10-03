//! Renders Ferrite's screens with the firmware's own drawing code into `target/sim/*.png` (240 x 320, as on the
//! radio), from sample data. `cargo sim`; `cargo sim --scale 2` for bigger images.
//!
//! `cargo simw` opens the interactive simulator instead ([`window`]): a window, PC keys as radio keys, a fake
//! radio, and tools to check the drawing against the look review's mock-ups.

use ferrite_core::freq::mhz;
use ferrite_core::msg::Live;
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State};
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::home;
use ferrite_ui::panel::{H, W};
use std::path::Path;

mod window;

fn save(fb: &FrameBuffer, name: &str, scale: u32) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/sim");
    save_fb(fb, &dir.join(format!("{name}.png")), scale);
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
    let scale = args.iter().position(|a| a == "--scale").and_then(|i| args.get(i + 1)?.parse().ok()).unwrap_or(1);
    if args.iter().any(|a| a == "--window") {
        let bus = args.iter().position(|a| a == "--bus-mbps").and_then(|i| args.get(i + 1)?.parse().ok());
        window::run(if scale > 1 { scale as usize } else { 2 }, bus);
        return;
    }

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

    let live = Live { freq: mhz(146) + 52_000, rssi: -87, squelch_open: true, chips: 3, ..Default::default() };
    home::draw(&mut fb, &live);
    save(&fb, "home", scale);
}
