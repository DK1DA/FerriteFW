//! Limits: the radio filled to every maximum (4000 channels, all with a position, 512 zones, 16 of them full), then
//! every screen opened and driven, with each 20 ms frame's flash reads turned into time on the radio. A frame that
//! would hold the UI task too long fails (users' reports, Nov 17, 2026: Nearby with a few hundred channels with
//! positions ran until the watchdog restarted the radio; the sample's 20 channels never showed it).
//!
//! The cost model is the SPI bus's: [`READ_US`] for each read (command, address, chip select, the driver) and
//! [`BYTE_US`] for each byte read. Work that reads nothing (sorting in RAM) isn't counted; on the radio it follows
//! the reads closely, as every pass over the channels is a record read each.

mod common;

use common::Radio;
use ferrite_app::chan::{Channel, ChannelStore, MAX_CHANNELS, MAX_ZONES, ZONE_CAP};
use ferrite_app::data::{Pos, ZoneCfg};
use ferrite_core::keys::Key;
use ferrite_sim::simcore::Stored;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

/// A flash read's fixed cost on the radio, µs: 4 command bytes at 15 MHz, chip select, the driver's call.
const READ_US: f64 = 8.0;
/// Each byte read, µs: 15 MHz SPI, polled a byte at a time.
const BYTE_US: f64 = 1.0;
/// The longest a frame's reads may take on the radio. The watchdog restarts it after 4.4 s at the least (SAFE-8,
/// LICK's slowest); a second is already a frozen radio to its owner.
const FRAME_MAX_MS: f64 = 1000.0;
/// Frames over this are listed (not failed): slow enough to notice.
const NOTE_MS: f64 = 200.0;

/// The full radio's flash image, built once: the sample's records, then zones to [`MAX_ZONES`] and channels to
/// [`MAX_CHANNELS`], each with a position (spread over about 100 km around the sample's fix), 16 zones full.
fn full_image() -> Vec<u8> {
    static IMG: OnceLock<Vec<u8>> = OnceLock::new();
    IMG.get_or_init(|| {
        let r = Radio::with(Stored::Sample);
        let s = r.sim.store();
        let here = s.data().fix().map(|f| f.pos).unwrap_or(Pos { lat: 454_178_090, lon: -757_116_483 });
        let first = s.zones();
        let mut i = 0u32;
        while s.zones() < MAX_ZONES {
            assert!(s.add_zone(format!("Zone {i:03}").as_bytes(), &ZoneCfg { shown: true, ..ZoneCfg::default() }), "zone {i}");
            i += 1;
        }
        let mut z = first;
        let mut k = 0u32;
        for num in 0..MAX_CHANNELS {
            if s.get(num).is_some() {
                continue;
            }
            while s.zone_len(z) >= ZONE_CAP {
                z += 1;
            }
            let mut name = [0u8; 16];
            let n = format!("Ch {num:04} far");
            name[..n.len()].copy_from_slice(n.as_bytes());
            let c = Channel { num, zone: z, rx: 14_400_000 + (num as u32 % 160) * 1_250, name, cfg: ferrite_app::slot::DEFAULT_CFG, flags: 0, tones: 0 };
            // a scattered order of distance: k * 7919 mod 4000 steps of about 25 m
            let d = ((k * 7919) % 4000) as i32;
            let pos = Pos { lat: here.lat + d * 2_250 * if k.is_multiple_of(2) { 1 } else { -1 }, lon: here.lon + (d % 97) * 3_000 };
            assert!(s.put_with_position(c, Some(pos)), "channel {num}");
            k += 1;
        }
        assert_eq!(s.count(), MAX_CHANNELS);
        r.sim.flash_image()
    })
    .clone()
}

/// The frames' costs: (screen, step, ms on the radio).
type Costs = Rc<RefCell<Vec<(String, String, f64)>>>;

/// A full radio whose every frame's reads are costed into `costs`, labelled with the current step.
fn full_radio(costs: &Costs, step: &Rc<RefCell<String>>) -> Radio {
    let mut r = Radio::with(Stored::Image(full_image()));
    let (costs, step) = (costs.clone(), step.clone());
    let last = Rc::new(RefCell::new((u64::MAX, 0u64)));
    r.rec = Some(Box::new(move |sim| {
        let f = sim.store().nor();
        let mut l = last.borrow_mut();
        if l.0 != u64::MAX {
            let ms = ((f.reads - l.0) as f64 * READ_US + (f.read_bytes - l.1) as f64 * BYTE_US) / 1000.0;
            if ms > 1.0 {
                costs.borrow_mut().push((sim.screen_name().to_string(), step.borrow().clone(), ms));
            }
        }
        *l = (f.reads, f.read_bytes);
    }));
    r
}

/// Each screen of the jump list in `range` (0 = home), from a full radio each, driven by the keys a person tries:
/// the arrows, the knob, OK into whatever is there and BACK out.
fn drive(range: std::ops::Range<usize>) {
    let costs: Costs = Rc::default();
    let step = Rc::new(RefCell::new(String::new()));
    for i in range {
        let mut r = full_radio(&costs, &step);
        let at = |_: &mut Radio, s: &str| *step.borrow_mut() = format!("jump {i}: {s}");
        at(&mut r, "open");
        r.sim.jump(i);
        r.wait(400);
        for (name, k) in [("Down", Key::Down), ("Up", Key::Up), ("Right", Key::Right), ("Left", Key::Left), ("OK", Key::Menu)] {
            at(&mut r, name);
            r.press(k).wait(300);
        }
        at(&mut r, "knob +5");
        r.knob(5).wait(300);
        at(&mut r, "OK");
        r.press(Key::Menu).wait(300);
        at(&mut r, "knob -3");
        r.knob(-3).wait(300);
        at(&mut r, "BACK");
        r.press(Key::Exit).wait(300);
        quiet(r);
    }
    judge(&costs.borrow());
}

/// Fails on a frame over [`FRAME_MAX_MS`]; lists each screen's worst frame over [`NOTE_MS`].
fn judge(costs: &[(String, String, f64)]) {
    let mut worst: Vec<(String, String, f64)> = Vec::new();
    for (screen, step, ms) in costs {
        match worst.iter_mut().find(|w| &w.0 == screen) {
            Some(w) if w.2 >= *ms => {}
            Some(w) => *w = (screen.clone(), step.clone(), *ms),
            None => worst.push((screen.clone(), step.clone(), *ms)),
        }
    }
    worst.sort_by(|a, b| b.2.total_cmp(&a.2));
    for (screen, step, ms) in worst.iter().filter(|w| w.2 > NOTE_MS) {
        let n = costs.iter().filter(|c| &c.0 == screen && c.2 > NOTE_MS).count();
        eprintln!("slow frame: {ms:8.0} ms  {screen} ({step}), {n} frames over {NOTE_MS} ms");
    }
    eprintln!("worst frame: {:.0} ms over {} costed frames", worst.first().map_or(0.0, |w| w.2), costs.len());
    assert!(worst.iter().all(|w| w.2 <= FRAME_MAX_MS), "frames over {FRAME_MAX_MS} ms on a full radio (above)");
}

/// Text cut off is the journeys' check (clipcheck); here it is listed, not failed, so the costs are judged.
fn quiet(r: Radio) {
    for c in ferrite_ui::clipcheck::take() {
        eprintln!("text cut off: {c:?}");
    }
    drop(r);
}

// The jump list in parts, so the test runner spreads them.
#[test]
fn full_screens_0_to_9() {
    drive(0..10);
}
#[test]
fn full_screens_10_to_19() {
    drive(10..20);
}
#[test]
fn full_screens_20_to_29() {
    drive(20..30);
}
#[test]
fn full_screens_30_to_39() {
    drive(30..40);
}
#[test]
fn full_screens_40_to_49() {
    drive(40..50);
}
#[test]
fn full_screens_50_to_59() {
    drive(50..60);
}
#[test]
fn full_screens_60_on() {
    drive(60..ferrite_app::screens::nav::ALL.len() + 1);
}

/// The channel browser's every view on a full radio: its picker's rows in turn (each zone view is alike, so the
/// first and the views after the zones), the list opened and scrolled.
#[test]
fn full_browser_views() {
    let costs: Costs = Rc::default();
    let step = Rc::new(RefCell::new(String::new()));
    let views = 12; // All, the bands, Broadcast, Recent, Nearby: Up from the first row reaches them from the end
    for v in 0..views {
        let mut r = full_radio(&costs, &step);
        *step.borrow_mut() = format!("browse view -{v}: open");
        r.menu("0 2 1").wait(400);
        for _ in 0..=v {
            r.press(Key::Up).wait(100);
        }
        *step.borrow_mut() = format!("browse view -{v}: list");
        r.press(Key::Menu).wait(500);
        *step.borrow_mut() = format!("browse view -{v}: scroll");
        r.knob(20).wait(300);
        r.knob(-40).wait(300);
        quiet(r);
    }
    judge(&costs.borrow());
}

/// The zones page with the Nearby row (hold V/M, KEY-23), OK on it: the users' crash.
#[test]
fn full_nearby_from_hold_vm() {
    let costs: Costs = Rc::default();
    let step = Rc::new(RefCell::new(String::new()));
    let mut r = full_radio(&costs, &step);
    *step.borrow_mut() = "hold V/M".into();
    r.hold(Key::VfoMem).wait(500);
    *step.borrow_mut() = "OK on Nearby".into();
    r.press(Key::Up).wait(100); // the Nearby row heads the list
    r.press(Key::Menu).wait(500);
    *step.borrow_mut() = "Nearby scrolled".into();
    r.knob(30).wait(300);
    quiet(r);
    judge(&costs.borrow());
}
