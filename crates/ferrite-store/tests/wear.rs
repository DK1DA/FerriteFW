//! Flash wear (decision 227, Alex: "keep an eye on how we abuse the flash with writes"): a heavy day of use and a
//! Console bulk write in the simulated flash, counting the erases each sector takes against a budget. A change that
//! makes the store erase more fails here, and the table shows where the erases go.
//!
//! NOR flash like this radio's is usually rated for 100,000 erases a sector; at the budgets below the most-worn
//! sector lasts the years the test prints.

use ferrite_app::chan::{channel, ChanWrite, ChannelStore, NO_ZONE};
use ferrite_app::data::{Data, Event, NoData, Op, Pos, Src, Waypoint, WpSrc};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_app::App;
use ferrite_core::layout::{CHANNELS, EVENT_LOG, SCRATCH_RING, SETTINGS};
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;
use ferrite_ui::text::Buf;

type Store = FlashStore<SimFlash, NoData>;

const RATED: u32 = 100_000;

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

fn event(n: u32) -> Event {
    let mut text = Buf::<64>::new();
    text.set(b"wear");
    Event { src: Src::Pocsag, ago_s: 0, freq: 15_000_000, slot: 0, unread: true, id: n, x: 0, y: 0, pos: None, name: Buf::new(), text }
}

/// Erases per sector (sector index, count), most first, between two snapshots.
fn worst(before: &[u32], after: &[u32]) -> Vec<(usize, u32)> {
    let mut v: Vec<(usize, u32)> = after.iter().zip(before).enumerate().map(|(i, (a, b))| (i, a - b)).filter(|x| x.1 > 0).collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v
}

fn region(sector: usize) -> &'static str {
    let a = sector as u32 * 4096;
    match a {
        a if SETTINGS.contains(&a) => "settings",
        a if CHANNELS.contains(&a) => "channels",
        a if EVENT_LOG.contains(&a) => "event log",
        a if SCRATCH_RING.contains(&a) => "scratch ring",
        _ => "other",
    }
}

/// A heavy day on the keypad: 300 settings saves (tuning and settings changes, each 2 s after the last change),
/// 50 channel edits, 200 logged events, 30 messages each way, 5 waypoints. Budget: the most-worn sector takes at
/// most 160 erases a day (settings, A/B, take half the saves each), which is about 1.7 years of such days at the
/// rated 100,000; a normal day is a fraction of this.
#[test]
fn a_heavy_day_on_the_keypad() {
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    for n in 0..60u16 {
        assert!(s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_600_000 + n as u32 * 1250, "Ch", DEFAULT_CFG), from: None }));
    }
    let before = s.nor().erases.clone();
    let mut a = app();
    let mut now = 0u32;
    for i in 0..300u32 {
        a.set_watch((i % 2) as u8);
        s.autosave(now, &a);
        now += 2_500;
        s.autosave(now, &a);
    }
    for i in 0..50u16 {
        assert!(s.apply(ChanWrite::Put { ch: channel(i, NO_ZONE, 14_700_000 + i as u32 * 1250, "Edited", DEFAULT_CFG), from: None }));
    }
    for n in 0..200 {
        s.log(&event(n));
    }
    for n in 0..30 {
        s.message_in(true, b"VE3KPW-9", format!("hello {n}").as_bytes(), 0);
        s.message_in(false, b"VE3KPW-9", format!("reply {n}").as_bytes(), 0);
    }
    for n in 0..5 {
        s.add_waypoint(&Waypoint { name: Buf::from(b"Here"), pos: Pos { lat: n, lon: n }, src: WpSrc::Marked });
    }
    s.request(Op::ReadAll);
    let w = worst(&before, &s.nor().erases);
    println!("a heavy day: the most-worn sectors");
    for (sec, n) in w.iter().take(6) {
        println!("  0x{:06X} {:13} {n:4} erases ({:.1} years of such days)", sec * 4096, region(*sec), RATED as f64 / *n as f64 / 365.0);
    }
    let (sec, n) = w[0];
    assert!(n <= 160, "0x{:06X} ({}) took {n} erases in a day", sec * 4096, region(sec));
    let scratch: u32 = w.iter().filter(|(s, _)| region(*s) == "scratch ring").map(|x| x.1).max().unwrap_or(0);
    assert!(scratch <= 3, "a scratch sector took {scratch} erases");
}

/// A Console bulk write of 1,000 changed channels through the Storage task's queue (8 deep, merged per sector):
/// each channel sector is erased about 63 / 8 times, each scratch sector a few times.
#[test]
fn a_console_bulk_write() {
    let s: Store = FlashStore::mount(SimFlash::new(), NoData);
    for n in 0..1000u16 {
        assert!(s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_600_000 + n as u32 * 125, "Old", DEFAULT_CFG), from: None }));
    }
    let before = s.nor().erases.clone();
    s.set_deferred(true);
    // The link's WRITE_MANY fills the queue; the Storage task takes a job, which merges what is queued behind it.
    let mut n = 0u16;
    while n < 1000 || s.has_jobs() {
        while n < 1000 && s.apply(ChanWrite::Put { ch: channel(n, NO_ZONE, 14_400_000 + n as u32 * 125, "New", DEFAULT_CFG), from: None }) {
            n += 1;
        }
        if let Some(j) = s.next_job() {
            ferrite_store::block(s.run(j));
        }
    }
    s.set_deferred(false);
    assert_eq!(s.by_num(999).map(|c| c.0.rx), Some(14_400_000 + 999 * 125));
    let w = worst(&before, &s.nor().erases);
    let ch: u32 = w.iter().filter(|(s, _)| region(*s) == "channels").map(|x| x.1).max().unwrap_or(0);
    let sc: u32 = w.iter().filter(|(s, _)| region(*s) == "scratch ring").map(|x| x.1).max().unwrap_or(0);
    println!("a bulk write of 1,000 channels: a channel sector {ch} erases, a scratch sector {sc}");
    assert!(ch <= 10, "a channel sector took {ch} erases");
    assert!(sc <= 6, "a scratch sector took {sc} erases");
}
