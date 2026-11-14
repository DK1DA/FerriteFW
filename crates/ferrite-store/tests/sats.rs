//! Satellite mode's records in the store (SAT-2, decision 340): written, read back and kept across a restart; a
//! whole list rewritten keeps each satellite's choices; the flash wear of a sync and of the choices (SAT-21: at most
//! one rewrite a day); and power cuts at every point of a sync and of a choices write, after which every place holds
//! the old satellite or the new one, never anything else.

use ferrite_app::data::{Data, NoData, Op};
use ferrite_core::layout::{SATELLITES, SCRATCH_RING};
use ferrite_core::sat::data::Prefs;
use ferrite_core::sat::wire::Info;
use ferrite_core::sat::{Kind, Sat, SatSource, Status, Transponder, TPS};
use ferrite_store::sats::StoreSats;
use ferrite_store::sim::SimFlash;
use ferrite_store::{FlashStore, SECTOR};

type Store = FlashStore<SimFlash, NoData>;

fn mount(f: SimFlash) -> Store {
    FlashStore::mount(f, NoData)
}

fn sat(id: u32, epoch: u32) -> Sat {
    let mut name = [0u8; 16];
    let t = format!("SAT {id}");
    name[..t.len()].copy_from_slice(t.as_bytes());
    let mut tps = [Transponder::default(); TPS];
    tps[0] = Transponder { up: 14_585_000, down: 43_679_500, tone: 670, arm: 744, kind: Kind::Voice };
    let el = ferrite_core::sat::Elements { epoch, incl: 645_527_000, ecc: 68_979, n: 1_483_296_838, ..Default::default() };
    Sat { id, name, status: Status::Active, el, tps, ntp: 1, flags: 0 }
}

/// Console's whole-list write (LINK-63, LINK-64): clear, each record, the info.
fn sync(s: &Store, list: &[Sat], info: Info) {
    assert!(s.request(Op::ClearSats));
    for (k, x) in list.iter().enumerate() {
        assert!(s.sat_stage(x));
        assert!(s.request(Op::PutSat(k as u8)));
    }
    assert!(s.request(Op::SatInfo(Some(info))));
}

/// The same, nothing checked (the power may go in it).
fn sync_cut(s: &Store, list: &[Sat], info: Info) {
    s.request(Op::ClearSats);
    for (k, x) in list.iter().enumerate() {
        s.sat_stage(x);
        s.request(Op::PutSat(k as u8));
    }
    s.request(Op::SatInfo(Some(info)));
}

/// Erases in the region and in the scratch ring so far.
fn erases(s: &Store) -> (u32, u32) {
    let f = s.nor();
    (region_erases(&f), ring_erases(&f))
}

fn region_erases(f: &SimFlash) -> u32 {
    (SATELLITES.start..SATELLITES.end).step_by(SECTOR as usize).map(|a| f.erases[(a / SECTOR) as usize]).sum()
}

fn ring_erases(f: &SimFlash) -> u32 {
    SCRATCH_RING.step_by(SECTOR as usize).map(|a| f.erases[(a / SECTOR) as usize]).sum()
}

/// SAT-2: 32 satellites written, read back in place order, kept across a restart; one deleted; a 33rd refused.
#[test]
fn written_read_and_kept() {
    let s = mount(SimFlash::new());
    let list: Vec<Sat> = (0..32).map(|i| sat(40_000 + i, 846_000_000)).collect();
    let info = Info { decl: Some(-130), stamp: 1_793_500_000, written: 1_793_541_600 };
    sync(&s, &list, info);
    assert_eq!(s.sat_places(), u32::MAX);
    let src = StoreSats(&s);
    assert_eq!(src.count(), 32);
    assert_eq!(src.get(31), Some(list[31]));
    assert_eq!(src.declination(), Some(-130));
    let v = src.version();
    assert!(s.request(Op::DeleteSat(3)));
    assert_ne!(StoreSats(&s).version(), v, "SAT-6: predicted again");
    assert!(s.sat_stage(&sat(1, 1)));
    assert!(!s.request(Op::PutSat(32)), "32 places");
    let s = mount(s.into_parts().0);
    assert_eq!(s.sat_info(), Some(info));
    let src = StoreSats(&s);
    assert_eq!(src.count(), 31);
    assert_eq!(src.get(3), Some(list[4]), "place order, the gap skipped");
    assert_eq!(s.sat_rec(3), None);
}

/// SAT-8, SAT-11, SAT-21: the choices are kept by catalogue number through a whole-list rewrite with fresh
/// elements, written only after the save delay, and a satellite no longer listed has none applied to another.
#[test]
fn choices_survive_a_resync() {
    let s = mount(SimFlash::new());
    let list: Vec<Sat> = (0..5).map(|i| sat(27_600 + i, 846_000_000)).collect();
    sync(&s, &list, Info::default());
    let src = StoreSats(&s);
    assert_eq!(src.prefs(2), None, "none kept: the flags' starting ones apply");
    let p = Prefs { trim: 30, hd: true, uhf_only: true, tp: 0, power: 1 };
    src.set_prefs(2, p);
    assert_eq!(src.prefs(2), Some(p), "at once in RAM");
    let f = s.into_parts().0;
    let s = mount(f.clone());
    assert_eq!(StoreSats(&s).prefs(2), None, "not written before the save delay");
    // again, and the save delay passes
    let s = mount(f);
    StoreSats(&s).set_prefs(2, p);
    s.autosave(5_000, &app());
    let fresh: Vec<Sat> = list.iter().rev().map(|x| Sat { el: ferrite_core::sat::Elements { epoch: 846_100_000, ..x.el }, ..*x }).collect();
    sync(&s, &fresh, Info::default());
    let s = mount(s.into_parts().0);
    let src = StoreSats(&s);
    assert_eq!(src.get(2).unwrap().id, 27_602);
    assert_eq!(src.prefs(2), Some(p), "same satellite, new elements, same place by chance");
    assert_eq!(src.prefs(1), None);
    assert_eq!(src.get(0).unwrap().el.epoch, 846_100_000);
}

fn app() -> ferrite_app::App {
    let set: &'static mut ferrite_app::settings::Settings = Box::leak(Box::new(ferrite_app::settings::Settings::DEFAULT));
    let alert: &'static mut ferrite_app::screens::alert::AlertInfo = Box::leak(Box::new(ferrite_app::screens::alert::AlertInfo::NONE));
    ferrite_app::App::new(set, alert)
}

/// SAT-21's wear: a sync of 32 satellites over a full list rewrites each of the region's sectors once (its scratch
/// copies one each); a sync over an empty region only programs; 200 choices written rewrite the choices' sector
/// only when its log fills (every 62); 100 info writes likewise (every 31).
#[test]
fn wear_of_a_sync_and_of_the_choices() {
    let s = mount(SimFlash::new());
    let list: Vec<Sat> = (0..32).map(|i| sat(40_000 + i, 846_000_000)).collect();
    let e0 = erases(&s);
    sync(&s, &list, Info::default());
    assert_eq!(erases(&s), e0, "an empty region: programs only");
    for day in 1..=10u32 {
        let fresh: Vec<Sat> = list.iter().map(|x| Sat { el: ferrite_core::sat::Elements { epoch: 846_000_000 + day * 86_400, ..x.el }, ..*x }).collect();
        let (r, k) = erases(&s);
        sync(&s, &fresh, Info { decl: Some(160), stamp: day, written: day });
        let (r1, k1) = erases(&s);
        assert_eq!(r1 - r, 2, "day {day}: each sector once");
        assert!(k1 - k <= 4, "day {day}");
    }
    let r = erases(&s).0;
    let src = StoreSats(&s);
    for i in 0..200u32 {
        // each a choice left alone for the save delay
        s.request(Op::SatPrefs(src.get((i % 3) as u8).unwrap().id, Prefs { trim: (i % 50) as i16 * 10, ..Default::default() }));
    }
    assert!(erases(&s).0 - r <= 4, "200 writes: a compaction every 62 or so");
    // the newest choices of each survive the compactions
    let s = mount(s.into_parts().0);
    assert_eq!(StoreSats(&s).prefs(0), Some(Prefs { trim: (198 % 50) as i16 * 10, ..Default::default() }));
    assert_eq!(StoreSats(&s).prefs(2), Some(Prefs { trim: (197 % 50) as i16 * 10, ..Default::default() }));
}

/// Power cuts at every flash operation of a resync (the clear, each record, the info) and of choices written into a
/// full log: after the restart every place holds the old satellite, the new one or nothing (cleared), never a
/// broken one; the choices are the old or the new; the store mounts.
#[test]
fn power_cuts_during_a_sync() {
    let base = mount(SimFlash::new());
    let old: Vec<Sat> = (0..20).map(|i| sat(40_000 + i, 846_000_000)).collect();
    sync(&base, &old, Info { decl: Some(10), stamp: 1, written: 1 });
    for i in 0..61u32 {
        base.request(Op::SatPrefs(40_000 + i % 20, Prefs { trim: 10, ..Default::default() }));
    }
    let f0 = base.into_parts().0;
    let new: Vec<Sat> = (0..20).map(|i| sat(40_000 + i, 846_500_000)).collect();
    let op = |s: &Store| {
        sync_cut(s, &new, Info { decl: Some(20), stamp: 2, written: 2 });
        s.request(Op::SatPrefs(40_005, Prefs { trim: 20, ..Default::default() }));
        s.request(Op::SatPrefs(40_006, Prefs { trim: 30, ..Default::default() }));
    };
    let probe = mount(f0.clone());
    let n0 = probe.nor().ops;
    op(&probe);
    let ops = probe.nor().ops - n0;
    assert!(ops > 1_000);
    let mut checked = 0;
    let mut cut = 1;
    while cut <= ops {
        let s = mount(f0.clone());
        {
            let mut nor = s.nor();
            nor.cut_at = Some(nor.ops + cut);
            nor.seed(cut);
        }
        op(&s);
        let mut f = s.into_parts().0;
        f.power_on();
        let s = mount(f);
        for k in 0..20u8 {
            if let Some(x) = s.sat_rec(k) {
                assert!(x == old[k as usize] || x == new[k as usize], "cut {cut}: place {k}");
            }
        }
        let i = s.sat_info().unwrap();
        assert!(i.decl == Some(10) || i.decl == Some(20), "cut {cut}");
        let p = s.sat_prefs(40_005).unwrap();
        assert!(p.trim == 10 || p.trim == 20, "cut {cut}");
        assert_eq!(s.sat_prefs(40_001).map(|p| p.trim), Some(10), "cut {cut}: kept through a compaction");
        checked += 1;
        cut += if cut < 50 { 1 } else { 1 + cut / 40 };
    }
    assert!(checked > 200);
}

// ---- the link (LINK-63 – LINK-65) ----

use ferrite_app::link::Session;
use ferrite_core::link::{self as wire, msg, what, MAX_WIRE};
use ferrite_core::msg::{Device, LiveAll};

struct Wire(Vec<u8>);

impl ferrite_app::term::Sink for Wire {
    fn room(&self) -> usize {
        4096
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// The cable's other end on a real store: one request, polled until its answer (status, the rest).
fn ask(app: &mut ferrite_app::App, link: &mut Session, s: &Store, now: &mut u32, ty: u8, p: &[u8]) -> (u8, Vec<u8>) {
    let mut w = [0u8; MAX_WIRE];
    let n = wire::build(ty, 7, p, &mut w).unwrap();
    for &c in &w[..n] {
        link.feed(c, *now);
    }
    let mut out = Wire(Vec::new());
    for _ in 0..8 {
        *now += 10;
        link.poll(app, s, &Device::default(), &LiveAll::default(), *now, &mut out);
        if !out.0.is_empty() {
            break;
        }
    }
    let k = out.0.len() - 1;
    let f = wire::parse(&mut out.0[..k]).unwrap();
    (f.payload[0], f.payload[1..].to_vec())
}

fn many(w: u8, recs: &[(u16, &[u8])]) -> Vec<u8> {
    let mut p = vec![w];
    for (i, r) in recs {
        p.extend_from_slice(&i.to_le_bytes());
        p.push(r.len() as u8);
        p.extend_from_slice(r);
    }
    p
}

/// LINK-63, LINK-64, LINK-65: satellites written by `WRITE_MANY` (clear, records, info) and
/// read back by `READ_MANY`; a 10 m transponder refused (SAT-13); the same record again writes nothing; `CLOCK`
/// sets and reads the clock.
#[test]
fn over_the_link() {
    let s = mount(SimFlash::new());
    let mut a = app();
    let mut link = Session::new(true);
    let mut now = 1_000;
    let (st, _) = ask(&mut a, &mut link, &s, &mut now, msg::HELLO, &[ferrite_core::link::PROTOCOL, b'T']);
    assert_eq!(st, 0);
    let list: Vec<Sat> = (0..3).map(|i| sat(27_600 + i, 846_000_000)).collect();
    assert_eq!(ask(&mut a, &mut link, &s, &mut now, msg::WRITE_MANY, &many(what::SAT, &[(wire::ADD, &[])])).0, 0);
    for (k, x) in list.iter().enumerate() {
        let r = ferrite_core::sat::wire::encode(x);
        let (st, ans) = ask(&mut a, &mut link, &s, &mut now, msg::WRITE_MANY, &many(what::SAT, &[(k as u16, &r)]));
        assert_eq!((st, ans.first().copied()), (0, Some(1)), "{ans:?}");
    }
    let info = Info { decl: Some(162), stamp: 1_793_500_000, written: 1_793_541_600 };
    assert_eq!(ask(&mut a, &mut link, &s, &mut now, msg::WRITE_MANY, &many(what::SAT_INFO, &[(0, &info.encode())])).0, 0);
    assert_eq!(s.sat_info(), Some(info));
    assert_eq!(StoreSats(&s).count(), 3);
    // READ_MANY: index, length, record for each held
    let mut q = vec![what::SAT];
    q.extend_from_slice(&0u16.to_le_bytes());
    q.extend_from_slice(&32u16.to_le_bytes());
    let (st, ans) = ask(&mut a, &mut link, &s, &mut now, msg::READ_MANY, &q);
    assert_eq!(st, 0);
    assert_eq!((ans[2], ans[3], ans[4] as usize), (0, 0, wire::SAT_LEN));
    assert_eq!(ferrite_core::sat::wire::decode(&ans[5..5 + wire::SAT_LEN]), Ok(list[0]));
    // SAT-13: a 10 m downlink is refused with its reason
    let mut bad = list[1];
    bad.tps[0].down = 2_940_000;
    let (st, ans) = ask(&mut a, &mut link, &s, &mut now, msg::WRITE_MANY, &many(what::SAT, &[(1, &ferrite_core::sat::wire::encode(&bad))]));
    assert_eq!(st, wire::Status::Invalid as u8);
    assert!(String::from_utf8_lossy(&ans).contains("SAT-13"));
    // the same again: nothing written
    let ops = s.nor().ops;
    ask(&mut a, &mut link, &s, &mut now, msg::WRITE_MANY, &many(what::SAT, &[(1, &ferrite_core::sat::wire::encode(&list[1]))]));
    assert_eq!(s.nor().ops, ops);
    // LINK-65: the clock
    let (st, ans) = ask(&mut a, &mut link, &s, &mut now, msg::CLOCK, &1_793_541_600u32.to_le_bytes());
    assert_eq!((st, &ans[..]), (0, &[&1_793_541_600u32.to_le_bytes()[..], &[2]].concat()[..]));
    assert_eq!(a.take_clock_set(), Some(1_793_541_600), "for the clock's owner, as 0 6 6");
    assert_eq!(ask(&mut a, &mut link, &s, &mut now, msg::CLOCK, &5u32.to_le_bytes()).0, wire::Status::BadArg as u8);
    let (st, ans) = ask(&mut a, &mut link, &s, &mut now, msg::CLOCK, &[]);
    assert_eq!((st, ans.len(), ans[4]), (0, 5, 0), "no clock yet in this App");
}
