//! A flash that stops answering: every read, program, erase and busy poll fails from a chosen call on (a chip
//! that drops off the bus, unlike [`SimFlash`]'s power cut, which only counts writes). The store must never
//! panic: a mount that fails reports [`Mounted::Failed`] and keeps nothing, a change that fails reports false,
//! and once the chip answers again a fresh mount finds every record either as it was before the change or as it
//! would be after it. A failure at every call of a mount and of each kind of change is tried.

use ferrite_app::aprs::AprsCfg;
use ferrite_app::chan::{channel, ChanWrite, Channel, ChannelStore, Name, MAX_CHANNELS, NO_ZONE};
use ferrite_app::data::{Contact, Data, Draft, Dtmf, Event, Fix, MsgState, Op, Pos, RangePreset, Src, Station, Sym, Waypoint, WpSrc, ZoneCfg};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_app::App;
use ferrite_store::sim::SimFlash;
use ferrite_store::store::{Job, Mounted};
use ferrite_store::{block, FlashStore, Nor, NorError};
use ferrite_ui::text::Buf;

/// [`SimFlash`] behind a bus that fails every call once `fail_at` calls have gone through.
struct Flaky {
    f: SimFlash,
    calls: u64,
    fail_at: Option<u64>,
}

impl Flaky {
    fn new(f: SimFlash) -> Self {
        Flaky { f, calls: 0, fail_at: None }
    }

    fn call(&mut self) -> bool {
        self.calls += 1;
        self.fail_at.is_none_or(|n| self.calls <= n)
    }
}

impl Nor for Flaky {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> ferrite_store::Result {
        if !self.call() {
            return Err(NorError);
        }
        self.f.read(addr, buf)
    }
    fn start_program(&mut self, addr: u32, data: &[u8]) -> ferrite_store::Result {
        if !self.call() {
            return Err(NorError);
        }
        self.f.start_program(addr, data)
    }
    fn start_erase(&mut self, addr: u32) -> ferrite_store::Result {
        if !self.call() {
            return Err(NorError);
        }
        self.f.start_erase(addr)
    }
    fn busy(&mut self) -> ferrite_store::Result<bool> {
        if !self.call() {
            return Err(NorError);
        }
        self.f.busy()
    }
    async fn pause() {}
}

/// What other tasks keep: a callsign, a fix, one heard station with a position.
struct Tasks;

impl Data for Tasks {
    fn callsign(&self) -> Option<Buf<10>> {
        Some(Buf::from(b"VA7FER-9"))
    }
    fn fix(&self) -> Option<Fix> {
        Some(Fix { pos: Pos { lat: 454_000_000, lon: -757_000_000 }, three_d: true, alt_m: 70, speed_kmh: 0, heading: 0, hdop_x10: 9, used: 8, age_s: 1, utc_s: 0 })
    }
    fn stations(&self) -> u16 {
        1
    }
    fn station(&self, i: u16) -> Option<Station> {
        (i == 0).then(|| station(b"VE3OCB-7"))
    }
    fn send_room(&self) -> bool {
        true
    }
}

type Store = FlashStore<Flaky, Tasks>;

fn station(call: &[u8]) -> Station {
    Station {
        call: Buf::from(call),
        sym: Sym::Ht,
        pos: Some(Pos { lat: 454_252_000, lon: -756_901_000 }),
        ago_s: 5,
        via: Buf::from(b"WIDE1-1"),
        course: None,
        alt_m: None,
        times: 1,
        comment: Buf::from(b"QRV"),
        msg: Buf::new(),
        msg_min: None,
        fx25: None,
    }
}

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

fn mount(f: SimFlash) -> Store {
    FlashStore::mount(Flaky::new(f), Tasks)
}

fn unmount(s: Store) -> SimFlash {
    s.into_parts().0.f
}

fn ev(n: u32) -> Event {
    Event { src: Src::Aprs, ago_s: 0, freq: 14_439_000, slot: 0, unread: true, id: n, x: 0, y: 0, pos: None, name: Buf::from(b"VE3KPW-9"), text: Buf::from(b"QSL?") }
}

fn ch(num: u16, zone: u8, name: &str) -> Channel {
    channel(num, zone, 14_652_000 + num as u32 * 100, name, DEFAULT_CFG)
}

/// A chip holding some of everything the store keeps.
fn populated() -> SimFlash {
    let s = mount(SimFlash::new());
    let a = app();
    assert!(s.save(&a));
    assert!(s.add_zone(b"Local", &ZoneCfg::default()));
    assert!(s.add_zone(b"Events", &ZoneCfg { shown: true, dwell: 9, ..ZoneCfg::default() }));
    for (n, z) in [(0, 0), (1, 0), (2, 1), (70, NO_ZONE), (500, 1)] {
        assert!(s.apply(ChanWrite::Put { ch: ch(n, z, "Chan"), from: None }));
    }
    assert!(s.add_preset(&RangePreset { name: Buf::from(b"2 m"), start: 14_400_000, end: 14_800_000, step: 10, mode: 0, narrow: true, skips: 0, resume: 0 }));
    assert!(s.request(Op::Dtmf(3, Some(Dtmf { len: 4, d: [0x12, 0x34, 0, 0, 0, 0, 0, 0] }))));
    assert!(s.request(Op::DtmfName(3, Buf::from(b"Gate"))));
    let mut cfg = AprsCfg { f5k: 28_878, beacon: 3, power: 1, ssid: 9, ..AprsCfg::DEFAULT };
    cfg.comment[..3].copy_from_slice(b"Hi!");
    assert!(s.request(Op::AprsCfg(cfg)));
    assert!(s.request(Op::Quick(1, Some(Buf::from(b"QRT")))));
    for n in 0..3 {
        assert!(s.log(&ev(n)));
    }
    assert!(s.message_in(true, b"VE3KPW-9", b"QSL?", 0));
    assert!(s.message_in(false, b"VA3RTX", b"Net at 8?", 0)); // ours, still pending: failed at the next mount
    assert!(s.heard_row(&station(b"VE3OCB-7")));
    assert!(s.add_waypoint(&Waypoint { name: Buf::from(b"Camp"), pos: Pos { lat: 1, lon: 2 }, src: WpSrc::Link }));
    assert!(s.request(Op::SaveContact(Contact { call: Buf::from(b"VE3KPW-9"), name: Buf::from(b"Pat") })));
    unmount(s)
}

/// Everything a change can touch, read back.
#[derive(Clone, PartialEq)]
struct Snap {
    chans: Vec<Option<Channel>>,
    zones: Vec<Option<(Name, ZoneCfg)>>,
    presets: Vec<RangePreset>,
    dtmf: Vec<Option<Dtmf>>,
    dtmf_names: Vec<Buf<10>>,
    aprs: AprsCfg,
    quick: Vec<Option<Buf<67>>>,
    events: Vec<(u32, bool)>,
    msgs: Vec<(Buf<10>, Buf<67>, MsgState)>,
    heard: Vec<Buf<10>>,
    wps: Vec<Waypoint>,
    contacts: Vec<Contact>,
}

fn snap(s: &Store) -> Snap {
    let mut heard = vec![];
    s.heard_rows(64, &mut |st, _| heard.push(st.call));
    Snap {
        chans: (0..MAX_CHANNELS).map(|n| s.by_num(n).map(|x| x.0)).collect(),
        zones: (0..s.zones()).map(|z| s.zone_name(z).map(|n| (n, s.zone_cfg(z)))).collect(),
        presets: (0..s.presets()).filter_map(|i| s.preset(i)).collect(),
        dtmf: (0..10).map(|i| s.dtmf(i)).collect(),
        dtmf_names: (0..10).map(|i| s.dtmf_name(i)).collect(),
        aprs: s.aprs_cfg(),
        quick: (0..ferrite_app::words::QUICK).map(|i| s.quick(i)).collect(),
        events: (0..s.events()).filter_map(|i| s.event(i)).map(|e| (e.id, e.unread)).collect(),
        msgs: (0..s.messages()).filter_map(|i| s.message(i)).map(|m| (m.peer, m.text, m.state)).collect(),
        heard,
        wps: (0..s.waypoints()).filter_map(|i| s.waypoint(i)).collect(),
        contacts: (0..s.contacts()).filter_map(|i| s.contact(i)).collect(),
    }
}

/// A mount that loses the chip at any call reports Failed (or, past the last call it needs, mounts), never
/// panics, and leaves the records for the next mount as a clean mount would.
#[test]
fn mount_with_the_chip_gone_at_every_call() {
    let f = populated();
    let clean = mount(f.clone());
    assert_eq!(clean.mounted(), Mounted::Ok);
    let calls = clean.nor().calls;
    let want = snap(&clean);
    assert_eq!(want.msgs[0].2, MsgState::Failed, "APRS-13.5: ours left pending ends as failed at power-on");
    assert!(want.aprs.f5k == 28_878 && want.aprs.ssid == 9 && &want.aprs.comment[..4] == b"Hi!\0");
    let mut failed = 0;
    for k in 0..calls {
        let s = FlashStore::new(Flaky { f: f.clone(), calls: 0, fail_at: Some(k) }, Tasks);
        block(s.mount_async());
        if s.mounted() == Mounted::Failed {
            failed += 1;
            // nothing is read or written while failed
            assert!(s.get(0).is_none() && s.dtmf(3).is_none() && s.dtmf_name(3).is_empty() && s.quick(1).is_none() && s.preset(0).is_none());
            assert!(!s.apply(ChanWrite::Delete(0)) && !s.log(&ev(9)) && !s.save(&app()) && !s.request(Op::Read(0)));
            assert!(!s.heard_row(&station(b"N0CALL")) && !s.message_in(true, b"A", b"B", 0) && !s.add_waypoint(&want.wps[0]));
            assert!(!block(s.run(Job::AprsSave)));
        }
        let again = mount(unmount(s));
        assert_eq!(again.mounted(), Mounted::Ok, "call {k}");
        assert!(snap(&again) == want, "the records after a mount that failed at call {k}");
    }
    assert!(failed as u64 > calls / 2, "most of them fail: {failed} of {calls}");
}

/// A dead chip at power-on: an empty store that keeps nothing (and the calibration check can't tell).
#[test]
fn a_dead_chip_mounts_as_failed() {
    let mut f = SimFlash::new();
    f.dead = true;
    let s = FlashStore::mount(f, Tasks);
    assert_eq!(s.mounted(), Mounted::Failed);
    assert!(!s.first_boot());
    assert_eq!(block(s.check_cal()), ferrite_core::txcal::CalCrc::Unknown);
    let mut set = Settings::DEFAULT;
    assert!(!s.load_settings(&mut set));
    let mut a = app();
    assert!(!s.restore(&mut a));
    assert!(!s.flush(&a) || s.idle());
    assert_eq!((s.count(), s.zones(), s.presets(), s.write_room()), (0, 0, 0, 0));
    assert!(s.zone_name(0).is_none() && s.zone_at(NO_ZONE, 0).is_none() && s.by_number(0).is_none() && s.located(0).is_none());
    assert!(!s.request(Op::NewZone) && !s.request(Op::Draft(Draft::default())));
    assert!(!s.put_with_position(ch(1, NO_ZONE, "x"), None));
}

/// Each record of `got` reads as it did `before` or as it would `after` (a change that spans several records,
/// such as deleting a zone, may stop between them); None, or what doesn't.
fn mixed(got: &Snap, before: &Snap, after: &Snap) -> Option<&'static str> {
    fn each<T: PartialEq>(g: &[T], b: &[T], a: &[T]) -> bool {
        g.iter().enumerate().all(|(i, x)| b.get(i) == Some(x) || a.get(i) == Some(x))
    }
    fn whole<T: PartialEq>(g: &T, b: &T, a: &T) -> bool {
        g == b || g == a
    }
    [
        (each(&got.chans, &before.chans, &after.chans), "a channel"),
        (each(&got.zones, &before.zones, &after.zones), "a zone"),
        (whole(&got.presets, &before.presets, &after.presets), "the presets"),
        (each(&got.dtmf, &before.dtmf, &after.dtmf), "a DTMF memory"),
        (each(&got.dtmf_names, &before.dtmf_names, &after.dtmf_names), "a DTMF name"),
        (whole(&got.aprs, &before.aprs, &after.aprs) && whole(&got.quick, &before.quick, &after.quick), "the APRS record"),
        (each(&got.events, &before.events, &after.events), "an event"),
        (each(&got.msgs, &before.msgs, &after.msgs), "a message"),
        (whole(&got.heard, &before.heard, &after.heard), "the heard rows"),
        (whole(&got.wps, &before.wps, &after.wps), "the waypoints"),
        (whole(&got.contacts, &before.contacts, &after.contacts), "the contacts"),
    ]
    .into_iter()
    .find(|x| !x.0)
    .map(|x| x.1)
}

/// One change with the chip gone at each of its calls: after the chip answers again, every record reads before
/// or after, and some of the failures are reported. `strict`: one reported done reads after.
fn sweep(name: &str, strict: bool, op: &dyn Fn(&Store) -> bool) {
    let base = populated();
    let s = mount(base.clone());
    let before = snap(&s);
    let n0 = s.nor().calls;
    assert!(op(&s), "{name}: works on a good chip");
    let calls = s.nor().calls - n0;
    let after = snap(&mount(unmount(s)));
    assert!(before != after, "{name} changes something");
    let mut refused = 0;
    for k in 0..calls {
        let s = mount(base.clone());
        let at = s.nor().calls + k;
        s.nor().fail_at = Some(at);
        let ok = op(&s);
        refused += !ok as u32;
        let again = mount(unmount(s));
        assert_eq!(again.mounted(), Mounted::Ok, "{name}: call {k}");
        let got = snap(&again);
        if let Some(what) = mixed(&got, &before, &after) {
            panic!("{name}: chip gone at call {k} of {calls}: {what} reads neither before nor after");
        }
        if ok && strict {
            assert!(got == after, "{name}: reported done with the chip gone at call {k} of {calls}, but not stored");
        }
    }
    assert!(refused > 0, "{name}: no failure reported");
}

#[test]
fn channel_changes_with_the_chip_gone() {
    sweep("put", true, &|s| s.apply(ChanWrite::Put { ch: ch(3, 0, "New"), from: None }));
    sweep("overwrite", true, &|s| s.apply(ChanWrite::Put { ch: ch(1, 1, "Moved"), from: None }));
    sweep("renumber", true, &|s| s.request(Op::Renumber(2, 900)));
    sweep("move from", true, &|s| s.apply(ChanWrite::Put { ch: ch(4, 0, "To 4"), from: Some(70) }));
    sweep("position", true, &|s| s.apply(ChanWrite::PutPos { ch: ch(0, 0, "Here"), pos: Some(Pos { lat: 5, lon: 6 }) }));
    sweep("delete", true, &|s| s.apply(ChanWrite::Delete(500)));
    sweep("skip", true, &|s| s.request(Op::Skip(0, true)));
    sweep("hide", true, &|s| s.request(Op::Hide(1, true)));
    sweep("move", true, &|s| s.request(Op::Move(70, 1)));
    sweep("erase all", true, &|s| s.apply(ChanWrite::EraseAll));
}

#[test]
fn zone_and_preset_changes_with_the_chip_gone() {
    sweep("new zone", true, &|s| s.request(Op::NewZone));
    sweep("delete zone", true, &|s| s.request(Op::DeleteZone(1)));
    sweep("zone cfg", true, &|s| s.request(Op::Zone(0, ZoneCfg { dwell: 3, ..ZoneCfg::default() })));
    sweep("rename zone", true, &|s| s.request(Op::RenameZone(0, Buf::from(b"Home"))));
    sweep("put zone", true, &|s| s.request(Op::PutZone(2, Buf::from(b"Third"), ZoneCfg::default())));
    sweep("new preset", true, &|s| s.request(Op::NewPreset));
    sweep("preset", true, &|s| s.request(Op::Preset(0, RangePreset { name: Buf::from(b"6 m"), start: 5_000_000, end: 5_400_000, step: 10, mode: 0, narrow: false, skips: 0, resume: 0 })));
    sweep("put preset", true, &|s| s.request(Op::PutPreset(0, RangePreset { name: Buf::from(b"Air"), start: 11_800_000, end: 13_700_000, step: 8, mode: 2, narrow: false, skips: 0, resume: 0 })));
    sweep("delete preset", true, &|s| s.request(Op::DeletePreset(0)));
    sweep("dtmf", true, &|s| s.request(Op::Dtmf(3, Some(Dtmf { len: 2, d: [0x99, 0, 0, 0, 0, 0, 0, 0] }))));
    sweep("dtmf clear", true, &|s| s.request(Op::Dtmf(3, None)));
    sweep("dtmf name clear", true, &|s| s.request(Op::DtmfName(3, Buf::new())));
}

#[test]
fn log_and_aprs_changes_with_the_chip_gone() {
    sweep("log", true, &|s| s.log(&ev(7)));
    sweep("read", false, &|s| s.request(Op::Read(1)));
    sweep("read all", false, &|s| s.request(Op::ReadAll));
    sweep("clear log", true, &|s| s.request(Op::ClearLog));
    sweep("message", true, &|s| s.message_to_us(b"VE3KPW-9", b"73", b"AB1"));
    sweep("message read", false, &|s| s.request(Op::MessageRead(1)));
    sweep("messages read all", false, &|s| s.request(Op::MessagesReadAll));
    sweep("clear messages", true, &|s| s.request(Op::ClearMessages));
    sweep("message state", true, &|s| s.message_in(false, b"VA3RTX", b"Again", 0) && s.message_state(b"VA3RTX", b"Again", MsgState::Failed));
    sweep("heard", true, &|s| s.heard_row(&station(b"VE3NEW")));
    sweep("clear heard", true, &|s| s.request(Op::ClearHeard));
    sweep("waypoint", true, &|s| s.request(Op::MarkHere));
    sweep("delete waypoint", true, &|s| s.request(Op::DeleteWaypoint(0)));
    sweep("contact", true, &|s| s.request(Op::SaveContact(Contact { call: Buf::from(b"VA3AAA"), name: Buf::from(b"Al") })));
    sweep("delete contact", true, &|s| s.request(Op::DeleteContact(Buf::from(b"VE3KPW-9"))));
    sweep("quick", true, &|s| s.request(Op::Quick(0, Some(Buf::from(b"CQ")))));
    sweep("aprs settings", true, &|s| s.request(Op::AprsCfg(AprsCfg { beacon: 1, ..s.aprs_cfg() })) && block(s.run(Job::AprsSave)));
    sweep("wipe", true, &|s| s.request(Op::Wipe));
}
