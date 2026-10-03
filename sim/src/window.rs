//! The interactive simulator: a window running the firmware's own screens, driven by the PC keyboard and a fake
//! radio. Every pixel comes from `ferrite-ui` through [`Panel`], exactly as on the radio.
//!
//! Tools for checking the drawing code against the look review:
//! - F2 flashes each region as it is redrawn (only what changed should flash, HOME-7);
//! - F3 blends the matching mock-up (`docs/img/screens/*.png`) over the screen;
//! - the title shows the pixels pushed by the last redraw and, with `--bus-mbps`, the time it would take on the
//!   radio's display bus;
//! - F12 saves a screenshot to `target/sim/`.

use ferrite_core::freq::{self, mhz};
use ferrite_core::keys::Key;
use ferrite_core::msg::Live;
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State};
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::home;
use ferrite_ui::panel::{Panel, H, W};
use minifb::{Key as K, KeyRepeat, Window, WindowOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The frame buffer, plus a record of what each redraw touched.
struct Counting {
    fb: FrameBuffer,
    rects: Vec<(i32, i32, i32, i32)>,
    pixels: u64,
}

impl Panel for Counting {
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.rects.push((x0, y0, x1, y1));
        self.fb.window(x0, y0, x1, y1);
    }
    fn push(&mut self, color: u16, count: u32) {
        self.pixels += count as u64;
        self.fb.push(color, count);
    }
    fn push_buf(&mut self, px: &[u16]) {
        self.pixels += px.len() as u64;
        self.fb.push_buf(px);
    }
}

#[derive(PartialEq)]
enum Screen {
    Boot(Instant),
    Home,
}

/// A recent redraw region, shown for a moment with F2 on.
struct Flash {
    rect: (i32, i32, i32, i32),
    at: Instant,
}

const KEYMAP: &str = "\
keys: arrows = UP/DOWN/LEFT/RIGHT · Enter = MENU/OK · Esc = BACK · 0-9 digits · , = * · . = #
      S = SLOT · V = V/M · Q = SIDE1 · W = SIDE2 · Space = PTT
      F2 redraw flash · F3 mock-up overlay · F12 screenshot · Ctrl-Q quit";

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
        K::Space => Key::Ptt,
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

fn rgb565_to_rgb(c: u16) -> u32 {
    let r = ((c >> 11) & 31) as u32 * 255 / 31;
    let g = ((c >> 5) & 63) as u32 * 255 / 63;
    let b = (c & 31) as u32 * 255 / 31;
    r << 16 | g << 8 | b
}

fn blend(a: u32, b: u32) -> u32 {
    let ch = |s: u32| ((a >> s & 255) + (b >> s & 255)) / 2;
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// The fake radio: a signal opens squelch for 3 s every 10 s, with a wandering level.
fn fake_radio(live: &mut Live, t: f32) {
    let open = (t % 10.0) > 6.0 && (t % 10.0) < 9.0;
    live.squelch_open = open;
    live.rssi = if open { -85 + ((t * 1.7).sin() * 6.0) as i16 } else { -121 + ((t * 0.9).sin() * 2.0) as i16 };
}

pub fn run(scale: usize, bus_mbps: Option<f64>) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let shots = repo.join("../docs/img/screens");
    let checks = [
        Check { label: "MCU", value: "AT32F403A", state: State::Ok },
        Check { label: "SPI flash", value: "4 MB", state: State::Ok },
        Check { label: "BK4829 VHF", value: "0x4829", state: State::Ok },
        Check { label: "BK4829 UHF", value: "0x4829", state: State::Ok },
        Check { label: "SI4732", value: "", state: State::Ok },
    ];
    let info = BootInfo { version: VERSION, callsign: Some("VA3ABC"), checks: &checks };

    let mut lcd = Counting { fb: FrameBuffer::new(), rects: Vec::new(), pixels: 0 };
    let mut live = Live { freq: mhz(146) + 52_000, rssi: -121, chips: 3, ..Default::default() };
    let mut screen = Screen::Boot(Instant::now());
    boot::draw(&mut lcd, &info);

    let (ww, wh) = (W as usize * scale, H as usize * scale);
    let mut win = Window::new("Ferrite sim", ww, wh, WindowOptions::default()).expect("open window");
    win.set_target_fps(60);
    println!("{KEYMAP}");

    let start = Instant::now();
    let mut flashes: Vec<Flash> = Vec::new();
    let (mut show_flash, mut overlay) = (false, false);
    let mut mockup: Option<Vec<u32>> = None;
    let mut last_key = String::from("-");
    let mut last_px: u64 = lcd.pixels;
    let mut out = vec![0u32; ww * wh];
    let mut shot = 0;

    while win.is_open() && !(win.is_key_down(K::LeftCtrl) && win.is_key_down(K::Q)) {
        lcd.rects.clear();
        lcd.pixels = 0;

        // Screens: boot for 2 s, then home.
        if let Screen::Boot(t0) = screen {
            if t0.elapsed() > Duration::from_millis(2000) {
                screen = Screen::Home;
                home::draw(&mut lcd, &live);
                mockup = load_mockup(&shots.join("HomeA.png"));
            }
        }

        // Keys
        for k in win.get_keys_pressed(KeyRepeat::Yes) {
            match k {
                K::F2 => show_flash = !show_flash,
                K::F3 => overlay = !overlay,
                K::F12 => {
                    let p: PathBuf = repo.join(format!("target/sim/shot-{shot}.png"));
                    crate::save_fb(&lcd.fb, &p, 2);
                    shot += 1;
                }
                _ => {
                    if let Some(key) = map_key(k) {
                        last_key = format!("{key:?}");
                        if screen == Screen::Home {
                            match key {
                                Key::Up => live.freq += 1_250,
                                Key::Down => live.freq -= 1_250,
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // Fake radio, then redraw only what changed.
        if screen == Screen::Home {
            let before = live;
            fake_radio(&mut live, start.elapsed().as_secs_f32());
            if live != before {
                home::update(&mut lcd, &live);
            }
        }
        if lcd.pixels > 0 {
            last_px = lcd.pixels;
        }
        let now = Instant::now();
        flashes.extend(lcd.rects.iter().map(|&rect| Flash { rect, at: now }));
        flashes.retain(|f| f.at.elapsed() < Duration::from_millis(250));

        // Compose: frame buffer, optional mock-up overlay, optional redraw outlines, scaled up.
        for y in 0..H {
            for x in 0..W {
                let mut c = rgb565_to_rgb(lcd.fb.at(x, y));
                if overlay {
                    if let Some(m) = &mockup {
                        c = blend(c, m[(y * W + x) as usize]);
                    }
                }
                if show_flash
                    && flashes.iter().any(|f| {
                        let (x0, y0, x1, y1) = f.rect;
                        (x == x0 || x == x1) && (y0..=y1).contains(&y) || (y == y0 || y == y1) && (x0..=x1).contains(&x)
                    })
                {
                    c = 0xFF00FF;
                }
                for dy in 0..scale {
                    let row = (y as usize * scale + dy) * ww;
                    for dx in 0..scale {
                        out[row + x as usize * scale + dx] = c;
                    }
                }
            }
        }
        win.update_with_buffer(&out, ww, wh).expect("draw");

        let bus = bus_mbps.map(|m| format!(" ≈ {:.1} ms", last_px as f64 * 2.0 / (m * 1e6) * 1e3)).unwrap_or_default();
        let name = if screen == Screen::Home { "Home" } else { "Boot" };
        win.set_title(&format!(
            "Ferrite sim · {name} · key {last_key} · {} · last redraw {last_px} px{bus}{}{}",
            freq_text(live.freq),
            if show_flash { " · F2" } else { "" },
            if overlay { " · F3 mock-up" } else { "" }
        ));
    }
}

fn freq_text(f: freq::Freq10) -> String {
    let mut b = [0u8; 16];
    let n = freq::format(f, &mut b);
    String::from_utf8_lossy(&b[..n]).into_owned()
}
