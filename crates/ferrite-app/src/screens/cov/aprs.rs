//! APRS heard and one station (chapter 25): every sample station's page in each unit system, a station without
//! a position, no fix here, saving a waypoint, and a station heard as FX.25.

use super::*;
use crate::data::Data;
use crate::screens::aprs::Aprs;

fn station(t: &mut T, i: u16) -> String {
    t.app.screen = Screen::Aprs(Aprs { cur: crate::screens::lists::Cursor { sel: i, top: i.saturating_sub(3) }, station: true });
    t.draw();
    t.texts()
}

/// APRS-3, APRS-4: each station's page in metric, imperial and nautical: weather in each unit, the path's hops,
/// the altitude or a dash, no position.
#[test]
fn every_station_page() {
    let mut t = T::new();
    t.open(Dest::AprsHeard);
    assert_eq!(t.screen().mockup(), Some("AprsHeard.png"));
    t.reads("title=\"Heard\"");
    let n = t.s.stations();
    for units in 0..3 {
        t.app.set.units = units;
        for i in 0..n {
            let s = station(&mut t, i);
            assert!(!s.is_empty());
        }
    }
    assert_eq!(t.screen().mockup(), Some("AprsStation.png"));
    t.reads("title=\"APRS\"");
    let wx = (0..n).find(|&i| t.s.station(i).unwrap().call.as_bytes() == b"VE3OBW").unwrap();
    t.app.set.units = 1;
    let s = station(&mut t, wx);
    assert!(s.contains("mph") && s.contains("\u{b0}F"), "{s}");
    t.app.set.units = 2;
    let s = station(&mut t, wx);
    assert!(s.contains("kn"), "{s}");
    let multi = (0..n).find(|&i| t.s.station(i).unwrap().call.as_bytes() == b"VA3MAR").unwrap();
    assert!(station(&mut t, multi).contains("VE3RSB-1*, WIDE2-1"));
}

/// APRS-3: a station without a position says so, and # can't save it as a waypoint; with no fix here the
/// distance can't be worked out.
#[test]
fn no_position_and_no_fix() {
    let mut t = T::new();
    t.open(Dest::AprsHeard);
    let i = (0..t.s.stations()).find(|&i| t.s.station(i).unwrap().pos.is_none()).unwrap();
    let s = station(&mut t, i);
    assert!(s.contains("No position"), "{s}");
    t.tap(Key::Hash);
    assert!(t.toast().ends_with("/ has no position"), "{}", t.toast());
    t.s.fix.set(None);
    let s = station(&mut t, 0);
    assert!(s.contains("No GPS fix here"), "{s}");
    t.app.screen = Screen::Aprs(Aprs { cur: Default::default(), station: false });
    t.draw();
    t.shows("VE3KPW-9");
}

/// APRS-44: a station last heard as FX.25 says so with the bytes fixed.
#[test]
fn fx25_station() {
    let mut t = T::new();
    t.app.set_aprs_mode(true);
    for s in 1..400 {
        t.s.aprs_tick(s * 30, true, true);
        if (0..t.s.stations()).any(|i| t.s.station(i).unwrap().fx25.is_some()) {
            break;
        }
    }
    let i = (0..t.s.stations()).find(|&i| t.s.station(i).unwrap().fx25.is_some()).expect("an FX.25 station heard");
    t.open(Dest::AprsHeard);
    let s = station(&mut t, i);
    assert!(s.contains("FX.25, 3") && s.contains("fixed"), "{s}"); // wrapped onto two lines
}
