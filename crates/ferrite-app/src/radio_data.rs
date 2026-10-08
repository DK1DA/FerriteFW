//! The records the radio's flash store doesn't keep itself (its `extra`, [`crate::data::Data`]): the APRS task's
//! heard list, beacon and messages being tried ([`AprsLive`]), the callsign the APRS task sends as (APRS-8: the
//! UI task copies it from the settings), and the GPS (the GPS task's fix and satellites through the UI task,
//! kept in [`GpsData`]: the fix's age, the fixed position without one, GPS-2, GPS-3). The simulators use their
//! sample data instead.

use crate::aprs::AprsCfg;
use crate::aprs_live::AprsLive;
use crate::data::{Beacon, Data, Draft, Fix, Op, Pos, Sat, Station};
use crate::gpsdata::GpsData;
use core::cell::Cell;
use ferrite_core::msg::GpsFix;
use ferrite_ui::text::Buf;

pub struct RadioData {
    pub aprs: AprsLive,
    cfg: Cell<AprsCfg>,
    callsign: Cell<Buf<10>>,
    /// The GPS as the screens read it: the UI task feeds it the GPS task's watches.
    pub gps: GpsData,
    draft: Cell<Draft>,
    ver: Cell<u32>,
}

impl Default for RadioData {
    fn default() -> Self {
        Self::new()
    }
}

impl RadioData {
    pub const fn new() -> Self {
        RadioData {
            aprs: AprsLive::new(),
            cfg: Cell::new(AprsCfg::DEFAULT),
            callsign: Cell::new(Buf::new()),
            gps: GpsData::new(),
            draft: Cell::new(Draft { to: Buf::new(), text: Buf::new() }),
            ver: Cell::new(0),
        }
    }

    /// The settings' callsign (SETUP-12), copied by the UI task when it changes.
    pub fn set_callsign(&self, c: &[u8]) {
        if self.callsign.get().as_bytes() != c {
            self.callsign.set(Buf::from(c));
            self.changed();
        }
    }

    /// The GPS task's latest report (`chan::POSITION`), read at `now_ms` on the same clock: the fix the beacons
    /// and "mark here" use, or None with the GPS off or no fix.
    pub fn set_gps(&self, g: &GpsFix, now_ms: u32) {
        self.gps.set_fix(*g);
        self.gps.tick(now_ms);
    }

    pub fn cfg(&self) -> AprsCfg {
        self.cfg.get()
    }

    /// Something the lists show moved on.
    pub fn changed(&self) {
        self.ver.set(self.ver.get().wrapping_add(1));
    }
}

impl Data for RadioData {
    fn version(&self) -> u32 {
        self.ver.get().wrapping_add(self.gps.version())
    }

    fn stations(&self) -> u16 {
        self.aprs.stations()
    }

    fn station(&self, i: u16) -> Option<Station> {
        self.aprs.station(i)
    }

    fn callsign(&self) -> Option<Buf<10>> {
        Some(self.callsign.get()).filter(|c| !c.is_empty())
    }

    fn aprs_cfg(&self) -> AprsCfg {
        self.cfg.get()
    }

    fn beacon(&self) -> Beacon {
        self.aprs.beacon(&self.cfg.get())
    }

    fn draft(&self) -> Draft {
        self.draft.get()
    }

    fn fix(&self) -> Option<Fix> {
        self.gps.fix()
    }

    fn sats(&self) -> u8 {
        self.gps.sats()
    }

    fn sat(&self, i: u8) -> Option<Sat> {
        self.gps.sat(i)
    }

    fn request(&self, op: Op) -> bool {
        match op {
            Op::Draft(d) => self.draft.set(d),
            // The store has saved it (pending); the APRS task tries it (APRS-13).
            Op::Send => {
                let d = self.draft.get();
                self.aprs.send(d.to.as_bytes(), d.text.as_bytes());
            }
            Op::BeaconNow => self.aprs.beacon_now(),
            Op::AfterVoice => self.aprs.after_voice(),
            Op::ClearHeard => self.aprs.clear_heard(),
            Op::AprsCfg(c) => self.cfg.set(c),
            Op::Wipe => {
                self.aprs.clear();
                self.cfg.set(AprsCfg::DEFAULT);
            }
            _ => return false,
        }
        self.changed();
        true
    }
}

/// The GPS task's report as the screens' and the APRS task's [`Fix`].
pub fn fix_of(g: &GpsFix, now_ms: u32) -> Option<Fix> {
    g.has_fix().then(|| Fix {
        pos: Pos { lat: g.lat, lon: g.lon },
        three_d: g.fix >= 3,
        alt_m: g.alt_dm / 10,
        speed_kmh: (g.speed_kn10 as u32 * 1852 / 10_000) as u16,
        heading: g.course10 / 10,
        hdop_x10: g.hdop10,
        used: g.used,
        age_s: (now_ms.wrapping_sub(g.at_ms) / 1000).min(u16::MAX as u32) as u16,
        utc_s: g.unix % 86_400,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_to_fix() {
        let g = GpsFix { on: true, fix: 3, used: 7, view: 11, lat: 454_252_000, lon: -756_901_000, alt_dm: 812, speed_kn10: 270, course10: 905, hdop10: 9, unix: 1_791_400_000, at_ms: 1_000 };
        let f = fix_of(&g, 3_500).unwrap();
        assert_eq!((f.pos.lat, f.alt_m, f.speed_kmh, f.heading, f.age_s), (454_252_000, 81, 50, 90, 2));
        assert!(fix_of(&GpsFix { fix: 0, ..g }, 0).is_none());
        let d = RadioData::new();
        d.set_gps(&g, 2_000);
        assert!(d.fix().is_some());
        d.set_callsign(b"VE3XYZ");
        assert_eq!(d.callsign().unwrap().as_bytes(), b"VE3XYZ");
        assert!(d.request(Op::Draft(Draft { to: Buf::from(b"VE3KPW"), text: Buf::from(b"Hi") })));
        assert!(d.request(Op::Send));
        let mut out = [0u8; crate::aprs_live::FRAME_MAX];
        let t = d.aprs.tick(0, true, true, b"VE3XYZ", d.fix(), &d.cfg(), &mut out);
        assert_eq!(t.what, "Message");
    }
}
