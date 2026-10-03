//! Renders Ferrite's screens with the firmware's own drawing code into `target/sim/*.png` (240 x 320, as on the
//! radio), from sample data. `cargo sim`; `cargo sim --scale 2` for bigger images.
//!
//! The desktop simulator (`docs/firmware-design.md`, "Testing and tooling": a window, PC keys as radio keys, fake
//! radio events) grows from this.

use ferrite_core::freq::mhz;
use ferrite_core::msg::Live;
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State, Style};
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::home;
use ferrite_ui::panel::{H, W};
use std::path::Path;

fn save(fb: &FrameBuffer, name: &str, scale: u32) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/sim");
    std::fs::create_dir_all(&dir).unwrap();
    let rgb = fb.rgb();
    let (w, h) = (W as u32 * scale, H as u32 * scale);
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let i = (((y / scale) * W as u32 + x / scale) * 3) as usize;
            out.extend_from_slice(&rgb[i..i + 3]);
        }
    }
    let path = dir.join(format!("{name}.png"));
    let mut enc = png::Encoder::new(std::fs::File::create(&path).unwrap(), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().unwrap().write_image_data(&out).unwrap();
    println!("{}", path.display());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scale = args.iter().position(|a| a == "--scale").and_then(|i| args.get(i + 1)?.parse().ok()).unwrap_or(1);

    let checks = [
        Check { label: "MCU", value: "AT32F403A", state: State::Ok },
        Check { label: "SPI flash", value: "4 MB", state: State::Ok },
        Check { label: "BK4829 VHF", value: "0x4829", state: State::Ok },
        Check { label: "BK4829 UHF", value: "0x4829", state: State::Ok },
        Check { label: "SI4732", value: "", state: State::Pending },
    ];
    let info = BootInfo { version: VERSION, checks: &checks, tx_armed: false };
    let mut fb = FrameBuffer::new();
    boot::draw(&mut fb, Style::Logo, &info);
    save(&fb, "boot-logo", scale);
    boot::draw(&mut fb, Style::List, &info);
    save(&fb, "boot-list", scale);

    let live = Live { freq: mhz(146) + 52_000, rssi: -87, squelch_open: true, chips: 3, ..Default::default() };
    home::draw(&mut fb, &live);
    save(&fb, "home", scale);
}
