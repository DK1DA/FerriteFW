//! The store on the simulated flash: round trips, first boot and its range presets, a damaged or foreign header,
//! thousands of power cuts at random points of settings writes, channel, zone and preset changes, and of the
//! recovery after them, and reads while a write is under way. After every cut the store must mount, and every
//! setting, channel, zone and preset must read either as it was before the interrupted change or as it would be
//! after it, never anything else; the zone lists must agree with the channels.

use ferrite_app::chan::{channel, ChanWrite, Channel, ChannelStore, Name, MAX_CHANNELS, NO_ZONE};
use ferrite_app::data::{Data, NoData, Op, RangePreset, ZoneCfg};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_app::App;
use ferrite_store::sim::SimFlash;
use ferrite_store::store::Mounted;
use ferrite_store::FlashStore;
use ferrite_ui::text::Buf;

type Store = FlashStore<SimFlash, NoData>;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

fn mount(f: SimFlash) -> Store {
    FlashStore::mount(f, NoData)
}

fn unmount(s: Store) -> SimFlash {
    s.into_parts().0
}

fn settings_of(s: &Store) -> Option<Settings> {
    let mut set = Settings::DEFAULT;
    s.load_settings(&mut set).then_some(set)
}

/// Everything the store holds that a change can touch.
#[derive(Clone, PartialEq)]
struct Snap {
    chans: Vec<Option<Channel>>,
    zones: Vec<Option<(Name, ZoneCfg)>>,
    presets: Vec<RangePreset>,
}

fn snap(s: &Store) -> Snap {
    let chans = (0..MAX_CHANNELS).map(|n| s.by_num(n).map(|x| x.0)).collect();
    let zones = (0..s.zones()).map(|z| s.zone_name(z).map(|n| (n, s.zone_cfg(z)))).collect();
    let presets = (0..s.presets()).filter_map(|i| s.preset(i)).collect();
    Snap { chans, zones, presets }
}

/// The zone lists agree with the records.
fn index_agrees(s: &impl ChannelStore) {
    let mut seen = 0u16;
    for z in (0..s.zones()).chain([NO_ZONE]) {
        let mut last: Option<u16> = None;
        for i in 0..s.zone_len(z) {
            let c = s.zone_at(z, i).expect("indexed channel exists");
            assert_eq!(c.zone, z, "channel {} indexed in zone {z}", c.num);
            assert!(last.is_none_or(|l| c.num > l), "zone {z} in number order");
            last = Some(c.num);
            assert_eq!(s.by_num(c.num).map(|x| x.1), Some(i), "position of {}", c.num);
            seen += 1;
        }
    }
    let real = (0..MAX_CHANNELS).filter(|&n| s.by_num(n).is_some()).count() as u16;
    assert_eq!(seen, real, "index covers every channel");
    assert_eq!(s.count(), real);
    for i in 0..real {
        assert!(s.by_number(i).is_some());
    }
    assert!(s.by_number(real).is_none());
}

fn cfg_with(tone: u16, off: i32) -> ferrite_app::slot::SlotCfg {
    let mut c = DEFAULT_CFG;
    c.tx_tone = tone;
    c.offset = off;
    c
}

/// A store with a few zones, channels and a preset, settings saved once.
fn seeded() -> SimFlash {
    let s = mount(SimFlash::new());
    assert_eq!(s.mounted(), Mounted::FirstBoot);
    let zc = ZoneCfg { scan_all: true, shown: true, ..ZoneCfg::default() };
    for n in [&b"Local"[..], b"Repeaters", b"Air"] {
        assert!(s.add_zone(n, &zc));
    }
    for (i, num) in [0u16, 1, 2, 9, 11, 14, 63, 64, 65, 130, 999].iter().enumerate() {
        let c = channel(*num, (i % 4) as u8 % 3, 14_400_000 + i as u32 * 2500, "Chan", cfg_with(1000, if i % 2 == 0 { 60_000 } else { 0 }));
        assert!(s.put_with_position(c, None));
    }
    assert!(s.add_preset(&RangePreset { name: Buf::from(b"Air"), start: 11_800_000, end: 13_700_000, step: 10, mode: 2, narrow: false, skips: 0, resume: 0 }));
    let a = app();
    assert!(s.save(&a));
    unmount(s)
}

#[test]
fn round_trip_and_first_boot() {
    let f = seeded();
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::Ok);
    assert_eq!(s.zones(), 3);
    assert_eq!(s.zone_name(1).unwrap().as_bytes(), b"Repeaters");
    assert_eq!(s.count(), 11);
    assert_eq!(s.presets(), 1);
    index_agrees(&s);
    // Settings and slots.
    let mut a = app();
    a.settings_mut().set_callsign(b"ve3abc");
    a.settings_mut().home_ch = Some(11);
    a.set_watch(2);
    a.set_channel(1, 1, 0, &s);
    assert!(s.save(&a));
    let s = mount(unmount(s));
    let mut set = Settings::DEFAULT;
    assert!(s.load_settings(&mut set));
    assert_eq!(set.callsign.as_bytes(), b"VE3ABC");
    assert_eq!(set.home_ch, Some(11));
    let mut b = app();
    assert!(s.restore(&mut b));
    assert_eq!(b.slot(1).ch.map(|c| c.num), a.slot(1).ch.map(|c| c.num));
    // The save timer: nothing to save right after restoring, a save 2 s after a change.
    assert!(!s.autosave(0, &b));
    b.set_watch(0);
    assert!(!s.autosave(100, &b));
    assert!(!s.autosave(2000, &b));
    assert!(s.autosave(2100, &b));
    assert!(!s.flush(&b));
}

#[test]
fn a_foreign_header_or_noise_formats() {
    // Another layout (the earlier journal layout was version 1): formatted, first boot, empty.
    let mut f = seeded();
    let h = ferrite_core::layout::LAYOUT_HEADER.start as usize;
    f.mem[h + 4] = 1;
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::FirstBoot);
    assert_eq!(s.count(), 0);
    assert_eq!(s.presets(), 0);
    // A chip full of noise: the same.
    let mut f = SimFlash::new();
    let mut r = Rng(7);
    for b in f.mem.iter_mut() {
        *b = r.next() as u8;
    }
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::FirstBoot);
    assert_eq!(s.count(), 0);
    assert_eq!(s.zones(), 0);
    index_agrees(&s);
}

/// Runs `op` on a copy to count its flash operations, then on `f` with the power cut at a random one of them,
/// then mounts again (sometimes cutting that too) and returns the store and what it was before and after.
fn cut_during(f: SimFlash, r: &mut Rng, op: &dyn Fn(&Store)) -> (Store, Snap, Snap) {
    let s = mount(f.clone());
    let before = snap(&s);
    let n0 = s.nor().ops;
    op(&s);
    let ops = s.nor().ops - n0;
    let after = snap(&s);
    let s = mount(f);
    if ops > 0 {
        let mut nor = s.nor();
        nor.cut_at = Some(nor.ops + 1 + r.below(ops));
        nor.seed(r.next());
    }
    op(&s);
    let mut f = unmount(s);
    f.power_on();
    if r.below(4) == 0 {
        // The power goes again while the store recovers.
        f.cut_at = Some(f.ops + 1 + r.below(20_000));
        f.seed(r.next());
        f = unmount(mount(f));
        f.power_on();
    }
    (mount(f), before, after)
}

#[test]
fn power_cuts_during_settings_writes() {
    let mut r = Rng(0x5EED_0001);
    let mut f = seeded();
    let mut a = app();
    let mut cur = settings_of(&mount(f.clone())).unwrap();
    let (mut old, mut new) = (0, 0);
    for i in 0..3000u32 {
        a.settings_mut().pin = i;
        a.settings_mut().fm_freq = (i % 2000) as u16;
        a.settings_mut().set_callsign(if i % 3 == 0 { b"VE3ABC" } else { b"VA3XYZ9" });
        let next = *a.settings();
        let (s, _, _) = cut_during(f, &mut r, &|s| {
            s.save(&a);
        });
        let got = settings_of(&s).expect("settings survive");
        assert!(got == cur || got == next, "iteration {i}: neither old nor new (pin {})", got.pin);
        assert_eq!(s.mounted(), Mounted::Ok);
        if got == next && got != cur {
            new += 1;
        } else {
            old += 1;
        }
        cur = got;
        // Half the time the next save gets through, so both copies take turns.
        if r.below(2) == 0 {
            assert!(s.save(&a));
            cur = next;
            assert!(settings_of(&s) == Some(cur));
        }
        f = unmount(s);
    }
    // A cut anywhere in the copy being written leaves it torn (the CRC is its last bytes), so nearly every cut
    // reads as before; a cut on the very last byte can still complete it.
    eprintln!("settings: {old} cuts left the old record, {new} the new one");
    assert_eq!(old + new, 3000);
}

#[test]
fn power_cuts_during_channel_zone_and_preset_changes() {
    let mut r = Rng(0xC0FF_EE01);
    let mut f = seeded();
    let (mut changed, mut old, mut new, mut ops_total) = (0, 0, 0, 0u64);
    for i in 0..2500u32 {
        let pick = r.below(9);
        let num = [0u16, 1, 2, 9, 11, 14, 63, 64, 65, 130, 999, 5, 300][r.below(13) as usize];
        let to = r.below(MAX_CHANNELS as u64) as u16;
        let z = r.below(4) as u8;
        let freq = 14_400_000 + r.below(400) as u32 * 1250;
        let op = move |s: &Store| {
            match pick {
                0 | 1 => {
                    let mut c = s.by_num(num).map(|x| x.0).unwrap_or_else(|| channel(num, NO_ZONE, freq, "New", DEFAULT_CFG));
                    c.rx = freq;
                    c.set_name(b"Edited");
                    s.apply(ChanWrite::Put { ch: c, from: None });
                }
                2 => {
                    s.apply(ChanWrite::Delete(num));
                }
                3 => {
                    s.request(Op::Renumber(num, to));
                }
                4 => {
                    s.request(Op::Move(num, if z == 3 { NO_ZONE } else { z }));
                }
                5 => {
                    s.request(Op::RenameZone(z.min(2), Name::from(b"Renamed")));
                }
                6 => {
                    if r_zone_delete(i) {
                        s.request(Op::DeleteZone(z.min(2)));
                    } else {
                        s.request(Op::NewZone);
                    }
                }
                7 => {
                    let p = RangePreset { name: Buf::from(b"P"), start: freq, end: freq + 100_000, step: 3, mode: 0, narrow: true, skips: 1, resume: 0 };
                    if s.presets() > 0 && i % 2 == 0 {
                        s.request(Op::Preset(0, p));
                    } else if s.presets() < 3 {
                        s.request(Op::NewPreset);
                    } else {
                        s.request(Op::DeletePreset(1));
                    }
                }
                _ => {
                    s.request(Op::Skip(num, i % 2 == 0));
                }
            }
        };
        let (s, before, after) = cut_during(f, &mut r, &op);
        assert_eq!(s.mounted(), Mounted::Ok, "iteration {i}");
        ops_total += s.nor().ops;
        let now = snap(&s);
        if before != after {
            changed += 1;
            if now == after {
                new += 1;
            } else if now == before {
                old += 1;
            }
        }
        for n in 0..MAX_CHANNELS as usize {
            assert!(
                now.chans[n] == before.chans[n] || now.chans[n] == after.chans[n],
                "iteration {i} (op {pick}): channel {n} is {:?}, was {:?}, would be {:?}",
                now.chans[n],
                before.chans[n],
                after.chans[n]
            );
        }
        for (zi, z) in now.zones.iter().enumerate() {
            let ok = before.zones.get(zi) == Some(z) || after.zones.get(zi) == Some(z);
            assert!(ok, "iteration {i}: zone {zi}");
        }
        assert!(now.presets == before.presets || now.presets == after.presets, "iteration {i}: presets");
        index_agrees(&s);
        f = unmount(s);
    }
    eprintln!("changes: {changed} of 2500 ops changed something; after the cut {old} read as before, {new} as after; {ops_total} flash ops");
    assert!(changed > 1000 && old > 300 && new > 300, "cuts landed on both sides");
}

fn r_zone_delete(i: u32) -> bool {
    i.is_multiple_of(3)
}

#[test]
fn settings_reset_erases_every_channel() {
    let s = mount(seeded());
    assert!(s.apply(ChanWrite::EraseAll));
    assert_eq!(s.count(), 0);
    let s = mount(unmount(s));
    assert_eq!(s.count(), 0);
    assert!(s.by_num(11).is_none());
    index_agrees(&s);
}

/// BOOT-14: the reset wipes every record area but the settings and calibration, and is a first boot again: the
/// next save after setup seeds as on a new radio. Outside the US and Canada no WX zone.
#[test]
fn reset_wipes_and_seeds_again() {
    let s = mount(seeded());
    assert!(s.count() > 0 && s.zones() > 0 && s.presets() > 0);
    let cal = s.nor().mem[0xF000..0x10000].to_vec();
    assert!(s.request(Op::Wipe));
    assert_eq!((s.count(), s.zones(), s.presets()), (0, 0, 0));
    assert!(s.first_boot());
    assert_eq!(s.nor().mem[0xF000..0x10000], cal[..], "calibration untouched");
    let mut a = app();
    a.settings_mut().set_region(Some(ferrite_core::txplan::Region::Uk));
    a.settings_mut().setup_done = true;
    assert!(s.save(&a));
    assert!(s.presets() > 0);
    assert_eq!(s.zones(), 0, "no WX zone in the UK");
    let s = mount(unmount(s));
    assert_eq!(s.count(), 0);
    index_agrees(&s);
}

#[test]
fn renumber_carries_the_position() {
    let s = mount(seeded());
    let c = s.by_num(11).unwrap().0;
    assert!(s.put_with_position(c, Some(ferrite_app::data::Pos { lat: 45, lon: -75 })));
    assert!(s.request(Op::Renumber(11, 500)));
    assert!(s.by_num(11).is_none());
    assert_eq!(s.position(500), Some(ferrite_app::data::Pos { lat: 45, lon: -75 }));
    let _ = s.version();
}

#[test]
fn first_boot_after_setup_seeds_the_regions_presets() {
    let s = mount(SimFlash::new());
    assert!(s.first_boot());
    let mut a = app();
    // A save during setup seeds nothing.
    assert!(s.save(&a));
    assert_eq!(s.presets(), 0);
    a.settings_mut().set_region(Some(ferrite_core::txplan::Region::Us));
    a.settings_mut().setup_done = true;
    assert!(s.save(&a));
    assert!(!s.first_boot());
    let names: Vec<_> = (0..s.presets()).map(|i| s.preset(i).unwrap().name).collect();
    let names: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
    assert_eq!(names, [&b"2 m"[..], b"70 cm", b"1.25 m", b"Airband", b"Marine", b"Weather", b"GMRS/FRS"]);
    // Deleting them all is kept: no new seeding on later saves or boots.
    for _ in 0..s.presets() {
        assert!(s.request(Op::DeletePreset(0)));
    }
    assert!(s.save(&a));
    let s = mount(unmount(s));
    assert_eq!(s.presets(), 0);
    // WX-1: the US and Canada get the weather channels as a "Weather" zone, receive only.
    assert_eq!(s.zones(), 1);
    assert_eq!(s.zone_name(0).unwrap().as_bytes(), b"Weather");
    assert_eq!(s.zone_len(0), 7);
    let wx: Vec<_> = (0..7).map(|i| s.zone_at(0, i).unwrap()).collect();
    assert_eq!(wx.iter().map(|c| c.rx).collect::<Vec<_>>(), (0..7).map(|k| 16_240_000 + k * 2_500).collect::<Vec<_>>());
    assert!(wx.iter().all(|c| c.flags & ferrite_app::chan::FLAG_RX_ONLY != 0 && c.cfg.mode == 0));
    assert_eq!(wx[0].name(), b"WX1");
    assert_eq!(wx.iter().map(|c| c.num).collect::<Vec<_>>(), (990..997).collect::<Vec<_>>(), "channels 991-997 as shown");
    // Europe: no weather or GMRS, PMR446 instead, 2 m to 146 MHz.
    let e: Vec<_> = ferrite_store::store::default_presets(ferrite_core::txplan::Region::Europe).collect();
    assert!(e.iter().any(|p| p.name.as_bytes() == b"PMR446") && !e.iter().any(|p| p.name.as_bytes() == b"Weather"));
    assert_eq!(e[0].end, 14_600_000);
}

/// A flash that stays busy a few polls after each program and erase, and whose pause yields once: the store's
/// writes take several polls, as on the radio, and the test reads in between, as the UI does.
struct Slow {
    f: SimFlash,
    left: u32,
}

impl ferrite_store::Nor for Slow {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> ferrite_store::Result {
        self.f.read(addr, buf)
    }
    fn start_program(&mut self, addr: u32, data: &[u8]) -> ferrite_store::Result {
        self.left = 2;
        self.f.start_program(addr, data)
    }
    fn start_erase(&mut self, addr: u32) -> ferrite_store::Result {
        self.left = 3;
        self.f.start_erase(addr)
    }
    fn busy(&mut self) -> ferrite_store::Result<bool> {
        let b = self.left > 0;
        self.left = self.left.saturating_sub(1);
        Ok(b)
    }
    async fn pause() {
        let mut once = false;
        core::future::poll_fn(|cx| {
            if once {
                core::task::Poll::Ready(())
            } else {
                once = true;
                cx.waker().wake_by_ref();
                core::task::Poll::Pending
            }
        })
        .await
    }
}

#[test]
fn reads_during_a_queued_write_see_old_or_new() {
    use ferrite_store::Nor;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let mut r = Rng(0xD1E5_0001);
    let mut f = seeded();
    let mut polls = 0u32;
    for i in 0..300u32 {
        let s: FlashStore<Slow, NoData> = FlashStore::new(Slow { f, left: 0 }, NoData);
        ferrite_store::block(s.mount_async());
        s.set_deferred(true);
        let num = [0u16, 1, 2, 9, 11, 63, 64, 999, 7][r.below(9) as usize];
        let before = s.by_num(num).map(|x| x.0);
        let others: Vec<_> = (0..MAX_CHANNELS).filter(|&n| n != num).map(|n| s.by_num(n).map(|x| x.0)).collect();
        let mut c = before.unwrap_or_else(|| channel(num, NO_ZONE, 14_600_000, "New", DEFAULT_CFG));
        c.rx = 14_500_000 + i * 1250;
        c.set_name(b"Queued");
        let w = if i % 4 == 3 { ChanWrite::Delete(num) } else { ChanWrite::Put { ch: c, from: None } };
        let after = if i % 4 == 3 { None } else { Some(c) };
        assert!(s.apply(w));
        assert!(s.has_jobs() && !s.idle());
        let job = s.next_job().unwrap();
        {
        let mut fut = std::pin::pin!(s.run(job));
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            let done = matches!(fut.as_mut().poll(&mut cx), Poll::Ready(true));
            // The UI reads in between: the channel and its sector neighbours are whole, old or new.
            let now = s.by_num(num).map(|x| x.0);
            assert!(now == before || now == after, "iteration {i}: channel {num} read {now:?} mid-write");
            let now_others: Vec<_> = (0..MAX_CHANNELS).filter(|&n| n != num).map(|n| s.by_num(n).map(|x| x.0)).collect();
            assert!(now_others == others, "iteration {i}: another channel changed mid-write");
            if done {
                break;
            }
            polls += 1;
        }
        }
        assert!(s.idle());
        assert_eq!(s.by_num(num).map(|x| x.0), after);
        index_agrees(&s);
        let (mut slow, _) = s.into_parts();
        let mut probe = [0u8; 1];
        assert!(slow.read(0x02C000, &mut probe).is_ok());
        f = slow.f;
    }
    eprintln!("queued writes: {polls} polls with reads in between");
    assert!(polls > 1000);
}

/// BOOT-12, SAFE-1: a first boot on a chip full of stock's (or any) data outside Ferrite's areas, with the power
/// cut at points through the format: the next boot formats again and comes up as a first boot, and nothing
/// outside the formatted areas changes — not calibration, not the SSB patch and trims (kept by a format), not
/// stock's data below 0x02C000 or above the waypoints.
#[test]
fn power_cuts_during_the_first_format() {
    use ferrite_core::layout::{CALIBRATION, SSB_PATCH, TRIMS, WRITABLE};
    let mut f = SimFlash::new();
    let mut r = Rng(11);
    for b in f.mem.iter_mut() {
        *b = r.next() as u8;
    }
    let before = f.mem.clone();
    // How many flash operations a whole first mount takes.
    let total = {
        let s = mount(f.clone());
        assert_eq!(s.mounted(), Mounted::FirstBoot);
        let n = s.nor().ops;
        n
    };
    assert!(total > 10);
    let kept = |a: usize| !WRITABLE[0].contains(&(a as u32)) || SSB_PATCH.contains(&(a as u32)) || TRIMS.contains(&(a as u32));
    for cut in [1, 2, total / 3, total / 2, total - 2, total - 1] {
        let mut g = f.clone();
        g.cut_at = Some(cut);
        let s = mount(g);
        let mut g = unmount(s);
        g.power_on();
        let s = mount(g);
        assert_eq!(s.mounted(), Mounted::FirstBoot, "cut at op {cut} of {total}: formats again");
        assert_eq!(s.count(), 0);
        index_agrees(&s);
        let g = unmount(s);
        for (a, (&x, &y)) in before.iter().zip(&g.mem).enumerate() {
            if kept(a) {
                assert_eq!(x, y, "cut at {cut}: byte {a:#x} outside the formatted areas changed");
            }
        }
        assert_eq!(&g.mem[CALIBRATION.start as usize..CALIBRATION.end as usize], &before[CALIBRATION.start as usize..CALIBRATION.end as usize]);
    }
}

fn fake_patch() -> (ferrite_core::ssb::Patch, Vec<u8>) {
    let p: Vec<u8> = (0..1979 * 8).map(|i| if i % 8 == 0 { if i % 80 == 0 { 0x15 } else { 0x16 } } else { (i * 13 + 5) as u8 }).collect();
    (ferrite_core::ssb::Patch { len: p.len() as u32, sha: ferrite_core::sha256::sha256(&p) }, p)
}

/// HF-18, HF-20, HF-21 (SAFE-9): the patch installs into SSB_PATCH and checks; the same patch again writes
/// nothing; a reformat (a foreign header) keeps it; a patch that doesn't match its header is refused before
/// anything is erased; nothing outside SSB_PATCH changes.
#[test]
fn ssb_patch_install_and_check() {
    use ferrite_core::layout::SSB_PATCH;
    let (p, patch) = fake_patch();
    let s = mount(SimFlash::new());
    assert_eq!(s.ssb_check(), Ok(None));
    let before = s.nor().mem.clone();
    assert_eq!(ferrite_store::block(s.ssb_install(p, &patch, ferrite_core::ssb::SOURCE_FLASHER)), Ok(true));
    assert_eq!(s.ssb_check(), Ok(Some(p)));
    for (a, (&x, &y)) in before.iter().zip(&s.nor().mem).enumerate() {
        if !SSB_PATCH.contains(&(a as u32)) {
            assert_eq!(x, y, "byte {a:#x} outside SSB_PATCH changed");
        }
    }
    let ops = s.nor().ops;
    assert_eq!(ferrite_store::block(s.ssb_install(p, &patch, ferrite_core::ssb::SOURCE_FLASHER)), Ok(false));
    assert!(s.nor().ops - ops <= 1, "the same patch again: read only");
    let mut wrong = patch.clone();
    wrong[100] ^= 1;
    let ops = s.nor().ops;
    assert!(ferrite_store::block(s.ssb_install(p, &wrong, ferrite_core::ssb::SOURCE_FLASHER)).is_err());
    assert_eq!(s.nor().ops, ops, "refused before any write");
    // A foreign layout header: the store formats, the patch stays.
    let mut f = unmount(s);
    f.mem[ferrite_core::layout::LAYOUT_HEADER.start as usize + 4] = 1;
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::FirstBoot);
    assert_eq!(s.ssb_check(), Ok(Some(p)), "a format keeps the patch");
}

/// HF-18: the power cut at any point of an install never leaves a patch that checks unless it's the whole one,
/// and the next install completes.
#[test]
fn ssb_patch_install_power_cuts() {
    let (p, patch) = fake_patch();
    let total = {
        let s = mount(SimFlash::new());
        let n0 = s.nor().ops;
        ferrite_store::block(s.ssb_install(p, &patch, 1)).unwrap();
        let n = s.nor().ops - n0;
        n
    };
    for cut in [1, 2, total / 4, total / 2, total - 3, total - 1] {
        let s = mount(SimFlash::new());
        let n0 = s.nor().ops;
        s.nor().cut_at = Some(n0 + cut);
        let _ = ferrite_store::block(s.ssb_install(p, &patch, 1));
        let mut f = unmount(s);
        f.power_on();
        let s = mount(f);
        let c = s.ssb_check().unwrap();
        assert!(c.is_none() || c == Some(p), "cut at {cut}: a half patch never checks");
        assert!(ferrite_store::block(s.ssb_install(p, &patch, 1)).is_ok());
        assert_eq!(s.ssb_check(), Ok(Some(p)), "cut at {cut}: the next install completes");
    }
}

/// CAL-7, TX-14 (TX audit F8): the calibration sector's CRC is recorded in the layout header on the first boot
/// that checks it, then compared at every boot; recording it leaves the header valid (nothing is formatted), and
/// a changed sector reads as changed. Calibration itself is only read.
#[test]
fn calibration_crc_recorded_then_checked() {
    use ferrite_core::txcal::CalCrc;
    let cal = ferrite_core::layout::CALIBRATION.start as usize;
    let mut f = seeded();
    for (i, b) in f.mem[cal..cal + 0x1000].iter_mut().enumerate() {
        *b = (i * 7) as u8;
    }
    let before = f.mem[cal..cal + 0x1000].to_vec();
    let s = mount(f);
    assert_eq!(s.mounted(), Mounted::Ok);
    let n = s.count();
    assert_eq!(ferrite_store::block(s.check_cal()), CalCrc::Recorded);
    assert_eq!(ferrite_store::block(s.check_cal()), CalCrc::Same);
    let mut f = unmount(s);
    assert_eq!(f.mem[cal..cal + 0x1000], before[..], "calibration never written");
    let s = mount(f.clone());
    assert_eq!((s.mounted(), s.count()), (Mounted::Ok, n), "the header still valid: nothing formatted");
    assert_eq!(ferrite_store::block(s.check_cal()), CalCrc::Same);
    f.mem[cal + 0x210] ^= 1;
    let s = mount(f);
    assert_eq!(ferrite_store::block(s.check_cal()), CalCrc::Changed);
}
