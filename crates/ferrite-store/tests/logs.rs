//! The event log, APRS messages, waypoints and the APRS record in the simulated flash: round trips across a
//! power cycle, the ring wrapping, waypoints moving down on a delete, the APRS settings saved like the settings,
//! a chip with old data in these areas, and power cuts at random points of every kind of write.

use ferrite_app::aprs::AprsCfg;
use ferrite_app::data::{Data, Draft, Event, Fix, Message, MsgState, Op, Pos, Src, Station, Sym, Waypoint, WpSrc};
use ferrite_app::screens::alert::AlertInfo;
use ferrite_app::settings::Settings;
use ferrite_app::App;
use ferrite_core::layout::{EVENT_LOG, WAYPOINTS};
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;
use ferrite_ui::text::Buf;

/// The records other tasks keep: a callsign, a fix and one heard station.
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
        (i == 0).then(|| Station {
            call: Buf::from(b"VE3OCB-7"),
            sym: Sym::Ht,
            pos: Some(Pos { lat: 454_252_000, lon: -756_901_000 }),
            ago_s: 5,
            via: Buf::new(),
            course: None,
            alt_m: None,
            times: 1,
            comment: Buf::new(),
            msg: Buf::new(),
            msg_min: None,
        })
    }
}

type Store = FlashStore<SimFlash, Tasks>;

fn mount(f: SimFlash) -> Store {
    FlashStore::mount(f, Tasks)
}

fn cycle(s: Store) -> Store {
    mount(s.into_parts().0)
}

fn ev(src: Src, n: u32) -> Event {
    let mut text = Buf::<64>::new();
    text.set(b"event ");
    for d in n.to_string().bytes() {
        text.push(&[d]);
    }
    Event { src, ago_s: 0, freq: 14_439_000 + n, slot: (n % 3) as u8, unread: true, id: n, x: 1, y: 2, pos: n.is_multiple_of(2).then_some(Pos { lat: n as i32, lon: -(n as i32) }), name: Buf::from(b"VE3KPW-9"), text }
}

fn wp(n: u32) -> Waypoint {
    let mut name = Buf::<16>::from(b"WP ");
    for d in n.to_string().bytes() {
        name.push(&[d]);
    }
    Waypoint { name, pos: Pos { lat: n as i32 * 1000, lon: -(n as i32) }, src: WpSrc::Link }
}

fn app() -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    let alert: &'static mut AlertInfo = Box::leak(Box::new(AlertInfo::NONE));
    App::new(set, alert)
}

#[test]
fn events_round_trip_newest_first_and_are_marked_read() {
    let s = mount(SimFlash::new());
    for n in 0..5 {
        assert!(s.log(&ev(Src::Pocsag, n)));
    }
    let s = cycle(s);
    assert_eq!(s.events(), 5);
    for i in 0..5u16 {
        let e = s.event(i).unwrap();
        let want = ev(Src::Pocsag, 4 - i as u32);
        assert!(e.text == want.text && e.pos == want.pos && e.freq == want.freq && e.id == want.id && e.slot == want.slot, "event {i}");
        assert!(e.unread);
    }
    assert!(s.request(Op::Read(1)));
    assert!(!s.event(1).unwrap().unread && s.event(0).unwrap().unread);
    assert!(s.request(Op::ReadAll));
    let s = cycle(s);
    assert!((0..5).all(|i| !s.event(i).unwrap().unread), "read marks are kept");
    assert!(s.request(Op::ClearLog));
    assert_eq!(cycle(s).events(), 0);
}

#[test]
fn the_log_wraps_a_sector_at_a_time() {
    let s = mount(SimFlash::new());
    let cap = (EVENT_LOG.len() as u32 / 4096) * 31;
    for n in 0..cap + 40 {
        s.log(&ev(Src::Aprs, n));
    }
    // Entering a used sector drops its 31 oldest at once.
    let s = cycle(s);
    let n = s.events() as u32;
    assert!(n > cap - 31 && n <= cap, "{n} of {cap}");
    assert_eq!(s.event(0).unwrap().id, cap + 39, "newest first");
    assert_eq!(s.event(n as u16 - 1).unwrap().id, cap + 40 - n, "oldest last, no gaps");
    assert!(s.event(n as u16).is_none());
}

#[test]
fn messages_keep_their_state() {
    let s = mount(SimFlash::new());
    assert!(s.message_in(true, b"VE3KPW-9", b"On my way", 0));
    s.request(Op::Draft(Draft { to: Buf::from(b"VA3RTX"), text: Buf::from(b"Net at 8?") }));
    assert!(s.draft().text.as_bytes() == b"Net at 8?", "the draft is held");
    assert!(s.request(Op::Send));
    assert!(s.draft() == Draft::default(), "sending empties the draft");
    let state = |s: &Store, i: u16| s.message(i).unwrap().state;
    assert_eq!(state(&s, 0), MsgState::Pending);
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Pending);
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Retrying(2));
    assert_eq!(state(&s, 0), MsgState::Retrying(2));
    let s = cycle(s);
    assert_eq!(s.messages(), 2);
    let m: Message = s.message(1).unwrap();
    assert!(m.inbound && m.peer.as_bytes() == b"VE3KPW-9" && m.state == MsgState::Unread);
    assert!(s.request(Op::MessageRead(1)));
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Acked);
    let s = cycle(s);
    assert_eq!((state(&s, 0), state(&s, 1)), (MsgState::Acked, MsgState::Read));
    // An ended message isn't moved again.
    s.message_state(b"VA3RTX", b"Net at 8?", MsgState::Failed);
    assert_eq!(state(&s, 0), MsgState::Acked);
    // APRS-8: nothing to send, nothing saved.
    assert!(!s.request(Op::Send));
    assert_eq!(s.messages(), 2);
}

#[test]
fn waypoints_list_newest_first_and_move_down_on_delete() {
    let s = mount(SimFlash::new());
    for n in 0..200 {
        assert!(s.add_waypoint(&wp(n)));
    }
    // Delete the oldest: everything after it moves down a slot, across two sector boundaries.
    assert!(s.request(Op::DeleteWaypoint(199)));
    assert!(s.request(Op::DeleteWaypoint(0)));
    let s = cycle(s);
    assert_eq!(s.waypoints(), 198);
    assert!(s.waypoint(0).unwrap() == wp(198));
    assert!(s.waypoint(197).unwrap() == wp(1));
    for i in 0..198u16 {
        assert!(s.waypoint(i).unwrap() == wp(198 - i as u32), "slot {i}");
    }
    // Mark here, a heard station and a grid square (GPS-10, GPS-20, GPS-21).
    assert!(s.request(Op::MarkHere));
    assert!(s.request(Op::SaveStation(0)));
    assert!(s.request(Op::AddWaypoint(Pos { lat: 1, lon: 2 }, Buf::from(b"FN25dk"))));
    let w = |i| s.waypoint(i).unwrap();
    assert!(w(0).src == WpSrc::Grid && w(1).src == WpSrc::Aprs && w(1).name.as_bytes() == b"VE3OCB-7" && w(2).src == WpSrc::Marked);
    assert_eq!(w(2).name.as_bytes(), b"Marked 199");
    // 256 at most.
    while s.waypoints() < 256 {
        assert!(s.add_waypoint(&wp(1000)));
    }
    assert!(!s.add_waypoint(&wp(1001)) && !s.request(Op::MarkHere));
}

#[test]
fn aprs_settings_wait_two_seconds_and_quick_messages_are_kept() {
    let s = mount(SimFlash::new());
    let a = app();
    // APRS-26: the custom path is kept too, and chosen (path 3)
    let mut custom = [0u8; ferrite_app::aprs::CUSTOM_LEN];
    custom[..7].copy_from_slice(b"WIDE2-2");
    let cfg = AprsCfg { f5k: 28_878, beacon: 3, power: 1, path: ferrite_app::aprs::PATH_CUSTOM, digi: 0x23, after_voice: true, custom };
    s.autosave(1000, &a);
    assert!(s.request(Op::AprsCfg(cfg)));
    assert!(s.aprs_cfg() == cfg);
    let ops = s.nor().ops;
    s.autosave(2500, &a);
    assert_eq!(s.nor().ops, ops, "not written within 2 s of the change");
    s.autosave(3100, &a);
    assert!(s.request(Op::Quick(2, Some(Buf::from(b"QRT, 73")))));
    assert!(s.quick(2).unwrap().as_bytes() == b"QRT, 73" && s.quick(1).is_none());
    let s = cycle(s);
    assert!(s.aprs_cfg() == cfg);
    assert!(s.quick(2).unwrap().as_bytes() == b"QRT, 73");
    assert!(s.request(Op::Quick(2, None)), "back to the factory text");
    assert!(cycle(s).quick(2).is_none());
}

#[test]
fn the_reset_empties_them_all() {
    let s = mount(SimFlash::new());
    s.log(&ev(Src::Same, 1));
    s.message_in(true, b"A", b"B", 0);
    s.add_waypoint(&wp(1));
    s.request(Op::AprsCfg(AprsCfg { f5k: 1, beacon: 1, power: 1, ..AprsCfg::DEFAULT }));
    assert!(s.request(Op::Wipe));
    assert_eq!((s.events(), s.messages(), s.waypoints()), (0, 0, 0));
    assert!(s.aprs_cfg() == AprsCfg::DEFAULT);
    let s = cycle(s);
    assert_eq!((s.events(), s.messages(), s.waypoints()), (0, 0, 0));
}

/// A chip that held something else in these areas (a radio before Ferrite): mount clears the way.
#[test]
fn old_data_in_the_areas_is_cleared_at_mount() {
    let s = mount(SimFlash::new());
    let mut f = s.into_parts().0;
    let mut noise = 0x1234_5678u32;
    for a in EVENT_LOG.start..EVENT_LOG.start + 3 * 4096 {
        noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        ferrite_store::Nor::start_program(&mut f, a, &[(noise >> 16) as u8]).unwrap();
    }
    for a in WAYPOINTS.start + 40..WAYPOINTS.start + 9000 {
        ferrite_store::Nor::start_program(&mut f, a, &[0x5A]).unwrap();
    }
    let s = mount(f);
    assert_eq!((s.events(), s.waypoints()), (0, 0));
    for n in 0..300 {
        assert!(s.add_waypoint(&wp(n % 256)) || n >= 256);
        s.log(&ev(Src::Dtmf, n));
    }
    let s = cycle(s);
    assert_eq!((s.events(), s.waypoints()), (300, 256));
    assert!(s.waypoint(0).unwrap() == wp(255) && s.event(0).unwrap().id == 299);
}

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

/// What a cut can change.
#[derive(Clone, PartialEq, Debug)]
struct Snap {
    events: Vec<(u32, bool)>,
    msgs: Vec<(Vec<u8>, MsgState)>,
    wps: Vec<Vec<u8>>,
    cfg: (u16, u8, u8),
    quick: Vec<Option<Vec<u8>>>,
}

fn snap(s: &Store) -> Snap {
    Snap {
        events: (0..s.events()).map(|i| s.event(i).map_or((u32::MAX, false), |e| (e.id, e.unread))).collect(),
        msgs: (0..s.messages()).map(|i| s.message(i).map_or((vec![], MsgState::Failed), |m| (m.text.as_bytes().to_vec(), m.state))).collect(),
        wps: (0..s.waypoints()).map(|i| s.waypoint(i).map_or(vec![], |w| w.name.as_bytes().to_vec())).collect(),
        cfg: {
            let c = s.aprs_cfg();
            (c.f5k, c.beacon, c.power)
        },
        quick: (0..9).map(|i| s.quick(i).map(|q| q.as_bytes().to_vec())).collect(),
    }
}

/// Runs `op` on a copy to count its flash operations, then on `f` with the power cut at a random one, then
/// mounts again (sometimes cutting the recovery too): the store as it came back, and before and after.
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
    let mut f = s.into_parts().0;
    f.power_on();
    if r.below(4) == 0 {
        f.cut_at = Some(f.ops + 1 + r.below(20_000));
        f.seed(r.next());
        f = mount(f).into_parts().0;
        f.power_on();
    }
    (mount(f), before, after)
}

#[test]
fn power_cuts_leave_before_or_after() {
    let mut r = Rng(0x1065_0001);
    let s = mount(SimFlash::new());
    // A log near a sector boundary, a few messages and waypoints, stored APRS settings.
    for n in 0..58 {
        s.log(&ev(Src::Pocsag, n));
    }
    for n in 0..6u32 {
        s.message_in(n.is_multiple_of(2), b"VA3RTX", format!("msg {n}").as_bytes(), 0);
        s.add_waypoint(&wp(n));
    }
    s.request(Op::AprsCfg(AprsCfg { f5k: 7, beacon: 2, power: 0, ..AprsCfg::DEFAULT }));
    s.flush(&app());
    let mut f = s.into_parts().0;
    let (mut old, mut new, mut dup) = (0, 0, 0);
    for i in 0..1500u32 {
        let k = r.below(8);
        let n = 1000 + i;
        let (s, before, after) = cut_during(f, &mut r, &|s| match k {
            0 | 1 => {
                s.log(&ev(Src::Ais, n));
            }
            2 => {
                s.request(Op::Read(r_idx(n, s.events())));
            }
            3 => {
                s.message_in(true, b"VE3KPW-9", format!("hi {n}").as_bytes(), 0);
            }
            4 => {
                s.message_state(b"VA3RTX", b"msg 1", if n % 3 == 0 { MsgState::Acked } else { MsgState::Retrying(2) });
            }
            5 => {
                s.add_waypoint(&wp(n));
            }
            6 => {
                if s.waypoints() > 3 {
                    s.request(Op::DeleteWaypoint(r_idx(n, s.waypoints())));
                }
            }
            _ => {
                s.request(Op::AprsCfg(AprsCfg { f5k: n as u16, beacon: 1, power: 2, ..AprsCfg::DEFAULT }));
                s.flush(&app());
                s.request(Op::Quick((n % 9) as u8, Some(Buf::from(format!("q {n}").as_bytes()))));
            }
        });
        let got = snap(&s);
        if got == before {
            old += 1;
        } else if got == after {
            new += 1;
        } else {
            // The only other outcomes allowed: a waypoint delete cut midway leaves one waypoint twice and none
            // lost; the APRS record's settings and its quick message are two writes, so one may land alone.
            let mut ok = false;
            if k <= 1 {
                // A cut while entering the oldest sector: its records go (as the append would drop them), the
                // new event isn't there yet.
                ok = got.events.as_slice() == &after.events[1..] && got.msgs == before.msgs && got.wps == before.wps;
            }
            if k == 6 {
                let mut g = got.wps.clone();
                g.dedup();
                ok = got.wps.len() == before.wps.len() && (g == after.wps || g.len() == got.wps.len() - 1);
            }
            if k == 7 {
                ok = got.events == before.events && got.msgs == before.msgs && got.wps == before.wps;
            }
            assert!(
                ok,
                "iteration {i} (kind {k}): neither before nor after: events {}/{}/{}, msgs {:?} / {:?} / {:?}, wps {:?} / {:?} / {:?}",
                before.events.len(),
                after.events.len(),
                got.events.len(),
                before.msgs,
                after.msgs,
                got.msgs,
                before.wps,
                after.wps,
                got.wps
            );
            dup += 1;
        }
        // Whatever happened, the store keeps working.
        assert!(s.log(&ev(Src::Dtmf, 2 * n)) && s.event(0).unwrap().id == 2 * n);
        f = s.into_parts().0;
    }
    eprintln!("cuts: {old} before, {new} after, {dup} in between (allowed)");
}

fn r_idx(n: u32, len: u16) -> u16 {
    (n % len.max(1) as u32) as u16
}
