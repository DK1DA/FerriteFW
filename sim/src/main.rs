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

use ferrite_sim::fake;

mod shots_d;
mod termport;
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
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    if let Some(secs) = arg("--term-headless").and_then(|s| s.parse::<u64>().ok()) {
        // No window: the radio and its cable only, for scripted terminal checks (`--term-tcp PORT` for TCP).
        let tcp = arg("--term-tcp").and_then(|s| s.parse::<u16>().ok());
        let mut sim = ferrite_sim::simcore::SimCore::new(1, false);
        let mut cable = match tcp {
            Some(p) => termport::Port::tcp(p),
            None => termport::Port::pty(),
        }
        .expect("a cable");
        println!("{}", cable.describe());
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < secs {
            sim.step(start.elapsed().as_millis() as u32);
            cable.pump(&mut sim);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        return;
    }
    if args.iter().any(|a| a == "--window") {
        // The window: a whole scale (default 1); resizing snaps to whole scales.
        let scale = arg("--scale").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
        let bus = arg("--bus-mbps").and_then(|s| s.parse().ok());
        let tcp = arg("--term-tcp").and_then(|s| s.parse::<u16>().ok());
        window::run(scale, bus, args.iter().any(|a| a == "--owner-card"), tcp, args.iter().any(|a| a == "--fresh"));
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
    let info = BootInfo { version: VERSION, callsign: Some(b"VA7FER"), owner: None, checks: &checks };
    let mut fb = FrameBuffer::new();
    boot::draw(&mut fb, &info);
    save(&fb, "boot", scale);
    let mut failed = checks;
    failed[1] = Check { label: "SPI flash", value: "no answer", state: State::Fail };
    boot::draw(&mut fb, &BootInfo { checks: &failed, ..info });
    save(&fb, "boot-failed", scale);
    // The owner card (MENU 0 7 3 3, BootC mock-up): the sample settings with the sample card on them.
    let mut set = ferrite_app::sample::settings();
    ferrite_app::sample::owner_card(&mut set);
    let mut running = checks;
    running[0..3].iter_mut().for_each(|c| c.state = State::Ok);
    (running[3].state, running[4].state) = (State::Pending, State::Pending);
    boot::draw(&mut fb, &BootInfo { callsign: set.boot_callsign(), owner: set.boot_owner(), checks: &running, ..info });
    save(&fb, "boot-owner", scale);

    // The app, as in the window: keys pressed at simulated times, the fake radio stepped to match.
    let store = fake::store();
    let mut radio = fake::FakeRadio::new();
    let mut dev = fake::device();
    dev.clock_min = Some(14 * 60 + 32);
    dev.utc = false;
    let mut app = fake::app();
    fake::sample(&mut app, &store, &mut radio);
    let mut t = 0u32;
    let mut run = |app: &mut App, fb: &mut FrameBuffer, until: u32, t: &mut u32| {
        while *t <= until {
            while let Some(c) = app.pop_cmd() {
                radio.cmd(c);
            }
            while let Some(w) = app.pop_write() {
                store.apply(w);
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
    let face = ferrite_sim::face::Face::new(2);
    let mut px = vec![0u32; face.w * face.h];
    let side1 = face.find(Key::Side(1)).into_iter().collect::<Vec<_>>();
    face.draw(&mut px, &side1, 0, true, false, 2);
    face.put_screen(&mut px, 2, |x, y| ferrite_sim::simcore::rgb565_to_rgb(fb.at(x, y)));
    ferrite_sim::save_rgb(&px, face.w, face.h, &Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/sim/radio.png"));
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
    run(&mut app, &mut fb, t + 3500, &mut t); // the toast times out

    // Group A, entry and editing: each screen as it first shows, with a little typed.
    let tap = |app: &mut App, t: &mut u32, keys: &[Key]| {
        for &k in keys {
            press(app, k, 100, t);
            *t += 150;
        }
    };
    use Key::{Digit as D, Down, Exit, Menu, Right, Slot, Star, VfoMem};
    press(&mut app, Exit, 800, &mut t);
    tap(&mut app, &mut t, &[D(1), D(4), D(6), D(9), D(4)]); // NAV-4: frequency entry
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "entry-freq", scale);
    press(&mut app, Exit, 800, &mut t);
    app.key_down(D(5), t, &store); // HOME-8: hold 5 opens tones at the long press, a popup names it
    run(&mut app, &mut fb, t + 800, &mut t);
    app.key_up(D(5), t, &store);
    run(&mut app, &mut fb, t + 100, &mut t);
    save(&fb, "hold-toast", scale);
    tap(&mut app, &mut t, &[Exit]);
    run(&mut app, &mut fb, t + 3000, &mut t); // the toast times out
    app.key_down(D(2), t, &store); // hold 2 on home: TX power steps, the popup shows the new level
    run(&mut app, &mut fb, t + 800, &mut t);
    app.key_up(D(2), t, &store);
    run(&mut app, &mut fb, t + 100, &mut t);
    save(&fb, "hold-popup-power", scale);
    app.key_down(D(2), t, &store); // and back to Low (2 more steps), then let the popup go
    run(&mut app, &mut fb, t + 800, &mut t);
    app.key_up(D(2), t, &store);
    app.key_down(D(2), t, &store);
    run(&mut app, &mut fb, t + 800, &mut t);
    app.key_up(D(2), t, &store);
    run(&mut app, &mut fb, t + 1500, &mut t);
    save(&fb, "hold-popup-gone", scale);
    app.open(ferrite_app::screens::nav::Dest::ShortcutMap); // the reference page, MENU 0 9 1 4
    run(&mut app, &mut fb, t + 100, &mut t);
    save(&fb, "shortcut-map", scale);
    tap(&mut app, &mut t, &[Exit]);
    tap(&mut app, &mut t, &[Menu, Menu]); // MENU 1, OK: the squelch editor (MenuEdit)
    run(&mut app, &mut fb, t + 3000, &mut t);
    save(&fb, "value-squelch", scale);
    tap(&mut app, &mut t, &[Exit, D(5), Menu]); // MENU 5, OK: offset as a list
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "value-offset", scale);
    tap(&mut app, &mut t, &[Exit, D(4), Menu]); // MENU 4, OK: the tone picker
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "tones", scale);
    press(&mut app, Exit, 800, &mut t);
    press(&mut app, VfoMem, 800, &mut t); // hold V/M: save as channel
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "save-channel", scale);
    tap(&mut app, &mut t, &[Down, Down, Menu, D(4), D(4)]); // its name: text entry, "H"
    t += 1100;
    tap(&mut app, &mut t, &[Star, D(4), D(4), D(4), D(5), D(5), D(5)]);
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "text", scale);
    press(&mut app, Exit, 800, &mut t);
    press(&mut app, Menu, 800, &mut t); // MENU 0 2 2: add repeater
    tap(&mut app, &mut t, &[D(2), D(2)]);
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "add-repeater", scale);
    press(&mut app, Exit, 800, &mut t);
    tap(&mut app, &mut t, &[Slot, D(1), D(5)]); // B is on a channel: channel entry
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "entry-channel", scale);
    press(&mut app, Exit, 800, &mut t);
    press(&mut app, Slot, 800, &mut t); // hold SLOT on a channel: the channel editor (HomeEditCh)
    tap(&mut app, &mut t, &[Down, Down, Down, Right, Right]);
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "home-edit-ch", scale);
    tap(&mut app, &mut t, &[Exit, Slot, Slot]);
    press(&mut app, Menu, 800, &mut t); // MENU 0 9 8 2: reset asks first
    tap(&mut app, &mut t, &[D(9), D(8), D(2)]);
    run(&mut app, &mut fb, t, &mut t);
    save(&fb, "confirm", scale);
    press(&mut app, Exit, 800, &mut t);
    // Setup, the PIN and the Extend TX warning are group D's ([`shots_d`]).

    // Radio activity (group B). Each from home, the fake radio answering.
    macro_rules! shot {
        ($ms:expr, $name:expr) => {
            run(&mut app, &mut fb, t + $ms, &mut t);
            save(&fb, $name, scale);
        };
    }
    let home = |app: &mut App, t: &mut u32| press(app, Key::Exit, 800, t);
    // Transmit: PTT held on A (145.920 after the -600k offset), 2 s in.
    app.key_down(Key::PTT, t, &store);
    shot!(2800, "transmit");
    app.key_up(Key::PTT, t, &store);
    // Refused: 420.500 is outside 70 cm in Canada (the TxRefused mock-up).
    app.set_vfo(0, 42_050_000);
    app.key_down(Key::PTT, t, &store);
    app.key_up(Key::PTT, t + 200, &store);
    shot!(300, "tx-refused");
    home(&mut app, &mut t);
    // Emergency: 406.025 (EPIRB) with Extend TX on; the first PTT only prompts. (243.000 is on the 1.25 m PA
    // path, which TX-10 keeps locked until its ceiling is measured.)
    app.set_region(Some(ferrite_core::txplan::Region::Canada), true);
    app.set_vfo_cfg(0, ferrite_app::slot::DEFAULT_CFG); // simplex: the offset would move it off 406.025
    app.set_vfo(0, 40_602_500);
    app.key_down(Key::PTT, t, &store);
    app.key_up(Key::PTT, t + 200, &store);
    shot!(1500, "tx-emergency");
    home(&mut app, &mut t);
    app.set_region(Some(ferrite_core::txplan::Region::Canada), false);
    fake::sample_app(&mut app, &store);
    app.set_vfo(0, 14_652_000);
    run(&mut app, &mut fb, t, &mut t);
    // Scan: SIDE2 on A, 9 s in (a hit every 7 s).
    press(&mut app, Key::Side(2), 100, &mut t);
    shot!(9000, "scan");
    press(&mut app, Key::Side(2), 100, &mut t);
    run(&mut app, &mut fb, t, &mut t); // the radio takes the commands (16 queue at most)
    // Band scope: hold 1, the marker moved onto the strong carrier.
    press(&mut app, Key::Digit(1), 800, &mut t);
    run(&mut app, &mut fb, t, &mut t);
    for _ in 0..13 {
        press(&mut app, Key::Right, 60, &mut t);
        run(&mut app, &mut fb, t, &mut t);
    }
    shot!(2500, "scope");
    home(&mut app, &mut t);
    // Tone search: MENU 0 3 7, found after 2.5 s.
    app.open(ferrite_app::screens::nav::Dest::ToneSearch);
    shot!(1000, "tone-search");
    shot!(2500, "tone-found");
    home(&mut app, &mut t);
    // Frequency counter: MENU 0 3 4.
    app.open(ferrite_app::screens::nav::Dest::Counter);
    shot!(2500, "counter");
    home(&mut app, &mut t);
    // FM broadcast and HF mode: group D's shots.
    // Weather radio: MENU 0 3 3 5 (Canada).
    app.open(ferrite_app::screens::nav::Dest::Weather);
    shot!(300, "weather");
    home(&mut app, &mut t);
    // The alert: group D's shot, as in the mock-up.
    // Squelch: SIDE1 held, the knob turned.
    app.key_down(Key::Side(1), t, &store);
    app.knob(1, t, &store);
    shot!(300, "squelch");
    app.knob(-1, t, &store);
    app.key_up(Key::Side(1), t, &store);
    run(&mut app, &mut fb, t + 100, &mut t);
    // The shortcut map: group A's shot.
    run(&mut app, &mut fb, t + 3500, &mut t);
    // Crossband: start page, then ready, then passing A -> B.
    app.open(ferrite_app::screens::nav::Dest::Crossband);
    shot!(300, "xb-start");
    press(&mut app, Key::Hash, 100, &mut t);
    shot!(14_000, "xb-ready");
    shot!(21_000, "xb-passing");
    home(&mut app, &mut t);
    // Home's other layouts (MENU 0 7 2).
    app.set_layout(1);
    shot!(1500, "home-scope");
    app.set_layout(2);
    shot!(20_000, "home-activity");
    app.set_layout(0);
    // Group C, lists and data: each opened as its way in does, keys pressed, the page saved.
    use ferrite_app::screens::nav::Dest;
    type Shot<'a> = (&'a str, Dest, &'a [(Key, u32)]);
    let shots: &[Shot] = &[
        ("event-log", Dest::EventLog, &[(Key::Down, 100)]),
        ("event-detail", Dest::EventLog, &[(Key::Down, 100), (Key::Down, 100), (Key::Menu, 100)]),
        ("event-detail-pager", Dest::EventLog, &[(Key::Down, 100), (Key::Menu, 100)]),
        ("event-log-options", Dest::EventLog, &[(Key::Menu, 800)]),
        ("aprs-heard", Dest::AprsHeard, &[]),
        ("aprs-station", Dest::AprsHeard, &[(Key::Menu, 100)]),
        ("aprs-messages", Dest::AprsMessages, &[]),
        ("aprs-message", Dest::AprsMessages, &[(Key::Menu, 100)]),
        ("aprs-compose", Dest::AprsMessages, &[(Key::Menu, 100), (Key::Menu, 100), (Key::Star, 100), (Key::Digit(2), 100), (Key::Digit(2), 100), (Key::Star, 100), (Key::Star, 100), (Key::Digit(3), 100), (Key::Digit(3), 100), (Key::Hash, 100), (Key::Digit(8), 100), (Key::Digit(4), 100), (Key::Digit(4), 100), (Key::Digit(3), 100), (Key::Digit(3), 100), (Key::Digit(7), 100), (Key::Digit(7), 100), (Key::Digit(7), 100), (Key::Digit(3), 100), (Key::Digit(3), 100), (Key::Hash, 100), (Key::Digit(2), 100), (Key::Digit(8), 100), (Key::Hash, 100), (Key::Star, 100), (Key::Star, 100), (Key::Digit(5), 100)]),
        ("sensors", Dest::Sensors, &[]),
        ("browse", Dest::Browse, &[(Key::Down, 100)]),
        ("browse-zone", Dest::Browse, &[(Key::Down, 100), (Key::Menu, 100), (Key::Down, 100)]),
        ("browse-all", Dest::Browse, &[(Key::Up, 100), (Key::Up, 100), (Key::Up, 100), (Key::Up, 100), (Key::Up, 100), (Key::Up, 100), (Key::Up, 100), (Key::Menu, 100)]),
        ("browse-nearby", Dest::Browse, &[(Key::Up, 100), (Key::Menu, 100)]),
        ("browse-recent", Dest::Browse, &[(Key::Up, 100), (Key::Up, 100), (Key::Menu, 100)]),
        ("browse-actions", Dest::Browse, &[(Key::Down, 100), (Key::Menu, 100), (Key::Down, 100), (Key::Menu, 100)]),
        ("browse-delete", Dest::Browse, &[(Key::Down, 100), (Key::Menu, 100), (Key::Down, 100), (Key::Menu, 100), (Key::Up, 100), (Key::Menu, 100)]),
        ("zones", Dest::Zones, &[(Key::Down, 100)]),
        ("zone-edit", Dest::Zones, &[(Key::Down, 100), (Key::Menu, 100), (Key::Down, 100), (Key::Down, 100), (Key::Down, 100), (Key::Down, 100)]),
        ("zone-delete", Dest::Zones, &[(Key::Down, 100), (Key::Down, 100), (Key::Star, 100)]),
        ("range-presets", Dest::RangePresets, &[(Key::Down, 100)]),
        ("range-preset", Dest::RangePresets, &[(Key::Down, 100), (Key::Down, 100), (Key::Menu, 100), (Key::Down, 100)]),
        ("skip-list", Dest::SkipList, &[]),
        ("hf-recent", Dest::HfRecent, &[]),
        ("fm-presets", Dest::FmPresets, &[(Key::Down, 100)]),
        ("gps-position", Dest::GpsPosition, &[]),
        ("gps-satellites", Dest::GpsPosition, &[(Key::Right, 100)]),
        ("waypoints", Dest::Waypoints, &[(Key::Down, 100)]),
        ("waypoint-go", Dest::Waypoints, &[(Key::Down, 100), (Key::Menu, 100)]),
    ];
    for &(name, dest, keys) in shots {
        press(&mut app, Key::Exit, 800, &mut t); // home
        run(&mut app, &mut fb, t + 3500, &mut t); // toasts gone
        app.open(dest);
        for &(k, hold) in keys {
            press(&mut app, k, hold, &mut t);
            t += 150;
        }
        run(&mut app, &mut fb, t, &mut t);
        save(&fb, name, scale);
    }
    shots_d::run(scale);
}
