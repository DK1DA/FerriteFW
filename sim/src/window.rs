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
//! The front panel ([`crate::face`]) is clickable: mouse down and up are key down and up, so holds and long
//! presses work; the channel knob steps on a click (upper half up, lower half down) or the wheel. Pressed keys
//! light up whichever way they were pressed. The indicator is green while the active slot receives and red while
//! a PTT is held (a fake transmit: the app has no transmit yet).
//!
//! Stand-ins for what the radio would report: M a message arrives, N an alert, F5 GPS off/searching/fix,
//! F6 Bluetooth, F7 battery normal/low/cut-off, F8 the power switch, F9 an empty battery, F10 an unknown SSB
//! patch at boot, F11 a write from a new Bluetooth device (allowed, it runs a fake write session). Each calls
//! the App hook the firmware calls.

use crate::face::{Face, Part};
use crate::fake::{self, FakeRadio};
use ferrite_app::Screen as AppScreen;
use ferrite_core::freq;
use ferrite_core::keys::Key;
use ferrite_core::msg::GpsState;
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State};
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::panel::{Panel, H, W};
use minifb::{Key as K, KeyRepeat, MouseButton, MouseMode, ScaleMode, Window, WindowOptions};
use std::collections::HashMap;
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


/// A recent redraw region, shown for a moment with F2 on.
struct Flash {
    rect: (i32, i32, i32, i32),
    at: Instant,
}

const KEYMAP: &str = "\
keys: arrows = UP/DOWN/LEFT/RIGHT · Enter = MENU/OK · Esc = BACK · 0-9 digits · , = * · . = #
      S = SLOT · V = V/M · Q = SIDE1 · W = SIDE2 · Space = PTT A · [ ] or mouse wheel = knob
      or click the radio's keys (hold the mouse for long presses; hold SIDE1 + wheel = squelch)
      hold a key 0.7 s for its long press (hold S: edit, hold . : lock, hold Esc: home, hold Enter: MENU 0)
      M message · N alert · F5 GPS · F6 Bluetooth · F7 battery · F8 power off · F9 empty battery
      F10 unknown SSB patch · F11 a write from a new device · X crossband (also MENU 0 3 6)
      L home layout · B a beacon (automatic transmit) · hold Enter while booting: the settings reset
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

pub(crate) fn rgb565_to_rgb(c: u16) -> u32 {
    let r = ((c >> 11) & 31) as u32 * 255 / 31;
    let g = ((c >> 5) & 63) as u32 * 255 / 63;
    let b = (c & 31) as u32 * 255 / 31;
    r << 16 | g << 8 | b
}

fn blend(a: u32, b: u32) -> u32 {
    let ch = |s: u32| ((a >> s & 255) + (b >> s & 255)) / 2;
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// The key a front-panel part sends.
fn key_of(p: Part) -> Key {
    match p {
        Part::Key(k) => k,
        _ => Key::Ptt,
    }
}

/// The mock-up that matches the screen on show (a placeholder has none: the boot one stands in).
fn mockup_name(s: Option<AppScreen>) -> &'static str {
    s.map_or(Some("BootA.png"), |s| s.mockup()).unwrap_or("BootA.png")
}

fn screen_name(s: Option<AppScreen>) -> &'static str {
    s.map_or("Boot", |s| s.name())
}

/// A window position to panel (buffer) pixels: minifb fits the buffer into the window, aspect kept, centred.
pub(crate) fn to_panel(m: (f32, f32), win: (usize, usize), buf: (usize, usize)) -> (i32, i32) {
    let (ww, wh, bw, bh) = (win.0 as f32, win.1 as f32, buf.0 as f32, buf.1 as f32);
    let k = (ww / bw).min(wh / bh);
    let (ox, oy) = ((ww - bw * k) / 2.0, (wh - bh * k) / 2.0);
    (((m.0 - ox) / k).floor() as i32, ((m.1 - oy) / k).floor() as i32)
}

/// `window_scale` is the window's size relative to the radio's 240 x 320 (1.5 by default). The panel is rendered at a
/// whole scale (2, or more above 2) and minifb fits it to the window, which can be resized freely.
pub fn run(window_scale: f32, bus_mbps: Option<f64>) {
    let scale = (window_scale.ceil() as usize).max(2);
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
    let store = fake::store();
    let mut radio = FakeRadio::new();
    let mut dev = fake::device();
    let mut app = fake::app();
    fake::sample(&mut app, &store, &mut radio); // B on "Club repeater", as in the mock-up
    let mut booting = true;
    boot::draw(&mut lcd, &info);

    let face = Face::new(scale);
    let (ww, wh) = (face.w, face.h);
    let fit = window_scale / scale as f32;
    let (vw, vh) = ((ww as f32 * fit).round() as usize, (wh as f32 * fit).round() as usize);
    let opts = WindowOptions { resize: true, scale_mode: ScaleMode::AspectRatioStretch, ..WindowOptions::default() };
    let mut win = Window::new("Ferrite sim", vw, vh, opts).expect("open window");
    // Input is polled every ~2 ms so a quick trackpad tap (down and up inside one 16 ms frame) is never missed;
    // the window is redrawn at 60 Hz.
    win.set_target_fps(0);
    let mut last_present = Instant::now() - Duration::from_secs(1);
    println!("{KEYMAP}");

    let start = Instant::now();
    let ms = || start.elapsed().as_millis() as u32;
    let mut flashes: Vec<Flash> = Vec::new();
    let (mut show_flash, mut overlay) = (false, false);
    let mut mockups: HashMap<&str, Option<Vec<u32>>> = HashMap::new();
    let mut last_key = String::from("-");
    let mut last_px: u64 = lcd.pixels;
    let mut out = vec![0u32; ww * wh];
    let mut shot = 0;
    let mut battery = 0;
    let mut layout = 0;
    // Parts held by the keyboard and by the mouse; the knob half last clicked and until when it shows; PTT.
    let mut kb_held: Vec<usize> = Vec::new();
    let mut mouse: Option<usize> = None;
    let mut knob_flash: (i8, u32) = (0, 0);
    let mut tx: Option<u8> = None;

    while win.is_open() && !(win.is_key_down(K::LeftCtrl) && win.is_key_down(K::Q)) {
        lcd.rects.clear();
        lcd.pixels = 0;
        let now = ms();
        if booting && now > 2000 {
            booting = false;
            // MENU (Enter) held through the boot screen: the settings reset (PIN-6, BOOT-14), as the firmware does.
            if win.is_key_down(K::Enter) {
                app.power_on_reset();
            }
        }

        // Keys: down and up go to the app, which makes short, long and repeat presses of them (KEY-3, KEY-4).
        for k in win.get_keys_pressed(KeyRepeat::No) {
            match k {
                K::F2 => show_flash = !show_flash,
                K::F3 => overlay = !overlay,
                K::F12 => {
                    let p: PathBuf = repo.join(format!("target/sim/shot-{shot}.png"));
                    crate::save_rgb(&out, ww, wh, &p);
                    shot += 1;
                }
                K::M if !booting => app.notify(b"VE3KPW-9", b"On my way", false),
                K::N if !booting => {
                    // The fake decoder's SAME warning now, as if heard on slot C (it also comes by itself 8 s
                    // after a slot settles on a weather channel).
                    if let Some(h) = ferrite_core::same::parse(fake::SAME_HEADER) {
                        app.same_alert(&h, 2, 16_255_000);
                    }
                }
                K::X if !booting => app.open(ferrite_app::screens::nav::Dest::Crossband),
                K::L if !booting => {
                    layout = (layout + 1) % 3;
                    app.set_layout(layout);
                }
                K::B if !booting => {
                    let a = app.active();
                    ferrite_app::screens::transmit::auto_tx(&mut app, a, b"Beacon");
                }
                K::F5 => {
                    dev.gps = match dev.gps {
                        GpsState::Off => GpsState::Searching,
                        GpsState::Searching => GpsState::Fix,
                        GpsState::Fix => GpsState::Off,
                    }
                }
                K::F6 => dev.bt = !dev.bt,
                K::F8 if !booting => app.power_off(),
                K::F9 if !booting => app.battery_empty(),
                K::F10 if !booting => app.ssb_patch_found(),
                K::F11 if !booting => app.link_write_request(),
                K::F7 => {
                    battery = (battery + 1) % 3;
                    (dev.battery_low, dev.battery_cutoff, dev.battery_pct, dev.battery_mv) =
                        [(false, false, 82, 7900), (true, false, 15, 7000), (true, true, 3, 6600)][battery];
                }
                _ => {
                    if let Some(key) = map_key(k) {
                        last_key = format!("{key:?}");
                        kb_held.extend(face.find(key));
                        if key == Key::Ptt {
                            tx = Some(0);
                        }
                        if !booting {
                            app.key_down(key, now, &store);
                        }
                    }
                }
            }
        }
        for k in win.get_keys_released() {
            if let Some(key) = map_key(k) {
                if let Some(i) = face.find(key) {
                    kb_held.retain(|&h| h != i);
                }
                if key == Key::Ptt {
                    tx = None;
                }
                if !booting {
                    app.key_up(key, now, &store);
                }
            }
        }
        // The mouse on the front panel: down and up are the key's down and up.
        let down = win.get_mouse_down(MouseButton::Left);
        let pos = win.get_unscaled_mouse_pos(MouseMode::Pass).map(|m| to_panel(m, win.get_size(), (ww, wh)));
        if down && mouse.is_none() {
            if let Some(i) = pos.and_then(|(x, y)| face.hit(x, y)) {
                let h = face.hits[i];
                match h.part {
                    Part::Knob => {
                        let d = if pos.unwrap().1 < h.y + h.h / 2 { 1 } else { -1 };
                        knob_flash = (d, now + 150);
                        if !booting {
                            app.knob(d, now, &store);
                        }
                    }
                    Part::Key(_) | Part::Ptt(_) => {
                        if let Part::Ptt(n) = h.part {
                            tx = Some(n);
                        }
                        let key = key_of(h.part);
                        last_key = format!("{key:?} (mouse)");
                        if !booting {
                            app.key_down(key, now, &store);
                        }
                        mouse = Some(i);
                    }
                }
            }
        } else if !down {
            if let Some(i) = mouse.take() {
                if let Part::Ptt(_) = face.hits[i].part {
                    tx = None;
                }
                if !booting {
                    app.key_up(key_of(face.hits[i].part), now, &store);
                }
            }
        }
        // The knob: [ and ] (with the PC's auto-repeat), or the mouse wheel.
        if !booting {
            for k in win.get_keys_pressed(KeyRepeat::Yes) {
                match k {
                    K::LeftBracket => app.knob(-1, now, &store),
                    K::RightBracket => app.knob(1, now, &store),
                    _ => {}
                }
            }
            if let Some((_, dy)) = win.get_scroll_wheel() {
                if dy.abs() >= 1.0 {
                    app.knob(if dy > 0.0 { 1 } else { -1 }, now, &store);
                }
            }
        }

        // The fake radio follows the app's commands, then the app redraws what changed.
        if !booting {
            while let Some(c) = app.pop_cmd() {
                radio.cmd(c);
            }
            while let Some(w) = app.pop_write() {
                store.apply(w);
            }
            radio.step(now as f32 / 1000.0, app.active());
            if let Some(a) = radio.take_same() {
                app.same_alert(&a.header, a.slot, a.freq); // WX-4: the decoder's alert, over any screen
            }
            // The fake link: an allowed write runs a session, 2 % a frame, then ends.
            match app.take_link_answer() {
                Some(ferrite_app::screens::link::LinkAnswer::Allowed) => {
                    dev.link.write_pct = Some(0);
                    app.link_write_progress();
                }
                Some(_) => dev.link.write_pct = None,
                None => {}
            }
            if let Some(p) = dev.link.write_pct {
                if p >= 100 {
                    dev.link.write_pct = None;
                    app.link_write_done();
                } else {
                    dev.link.write_pct = Some(p + 2);
                }
            }
            let clock = fake::device().clock_min;
            dev.clock_min = clock;
            app.tick(now, &radio.live, &store);
            while let Some(c) = app.pop_cmd() {
                radio.cmd(c);
            }
            while let Some(w) = app.pop_write() {
                store.apply(w);
            }
            app.render(&mut lcd, &radio.live, &dev, &store);
        }
        if lcd.pixels > 0 {
            last_px = lcd.pixels;
        }
        let t = Instant::now();
        flashes.extend(lcd.rects.iter().map(|&rect| Flash { rect, at: t }));
        flashes.retain(|f| f.at.elapsed() < Duration::from_millis(250));

        if last_present.elapsed() < Duration::from_millis(16) {
            win.update();
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        last_present = Instant::now();

        let screen = (!booting).then(|| app.screen());
        let mockup = if overlay {
            let name = mockup_name(screen);
            mockups.entry(name).or_insert_with(|| load_mockup(&shots.join(name))).as_ref()
        } else {
            None
        };

        // Compose: the front panel, then the screen: frame buffer, optional mock-up overlay, optional redraw
        // outlines, scaled up.
        let mut pressed = kb_held.clone();
        pressed.extend(mouse);
        let knob = if now < knob_flash.1 { knob_flash.0 } else { 0 };
        let rx = !booting && radio.live.slots[app.active() as usize].squelch_open;
        face.draw(&mut out, &pressed, knob, rx, tx.is_some(), scale);
        face.put_screen(&mut out, scale, |x, y| {
            let mut c = rgb565_to_rgb(lcd.fb.at(x, y));
            if let Some(m) = mockup {
                c = blend(c, m[(y * W + x) as usize]);
            }
            if show_flash
                && flashes.iter().any(|f| {
                    let (x0, y0, x1, y1) = f.rect;
                    (x == x0 || x == x1) && (y0..=y1).contains(&y) || (y == y0 || y == y1) && (x0..=x1).contains(&x)
                })
            {
                c = 0xFF00FF;
            }
            c
        });
        win.update_with_buffer(&out, ww, wh).expect("draw");

        let bus = bus_mbps.map(|m| format!(" ≈ {:.1} ms", last_px as f64 * 2.0 / (m * 1e6) * 1e3)).unwrap_or_default();
        let a = app.active();
        win.set_title(&format!(
            "Ferrite sim · {} · key {last_key} · {}{} {} · last redraw {last_px} px{bus}{}{}{}",
            screen_name(screen),
            tx.map(|n| format!("TX {} · ", ["A", "B", "C"][n as usize])).unwrap_or_default(),
            ["A", "B", "C"][a as usize],
            freq_text(app.slot(a).freq()),
            if app.locked() { " · locked" } else { "" },
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
