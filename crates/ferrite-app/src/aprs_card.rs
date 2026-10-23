//! Home's APRS card (APRS-17, decision 181): what slot C shows while it is the APRS slot, for the drawing
//! (`screens::home`) and the readback (`uistate`, TERM-37) alike. One shape for the three holders, each filling
//! it with its own facts:
//!
//! | holder      | header         | right of the frequency          | bottom line, left                    |
//! |-------------|----------------|---------------------------------|--------------------------------------|
//! | APRS mode   | APRS Mode · VHF| "14 heard"                      | "Last VE7ABC-9 · 2 min"              |
//! | (+ beacons) | APRS + Beacon ·| (APRS-17.5)                     |                                      |
//! | KISS        | APRS KISS · VHF| "Bluetooth" or "Cable"          | "Sent 3 · Heard 14"                  |
//! | Beacons     | APRS Beacon ·  | "next in" over "4:32", or why   | "Every 10 min · GPS fix"             |
//!
//! and the beacon power at the bottom right. A beacon waiting for the relay (APRS-20) takes the bottom line in
//! amber. With beacons on beside APRS mode or KISS, their countdown is on the large card's beacon row
//! ("Next beacon in 4 min", as before) and in the readback's `next=`; the small card keeps the holder's own fact.

use crate::aprs::Holder;
use crate::data::Data;
use crate::screens::lists::{age_text, Txt};
use crate::App;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// The card's texts.
pub struct AprsCard {
    pub holder: Holder,
    /// Right of the frequency: "14 heard", "Bluetooth", the countdown "4:32" ([`AprsCard::countdown`]) or a
    /// short reason ("no fix").
    pub right: Buf<16>,
    /// `right` is the beacon's countdown, drawn under a small "next in".
    pub countdown: bool,
    /// `right` or `line` is a warning (amber): no fix, or the beacon waiting for the relay.
    pub right_warn: bool,
    pub line_warn: bool,
    /// The bottom line's left part.
    pub line: Buf<48>,
    /// The beacon power, as the chips write it.
    pub power: &'static str,
    /// The beacon's countdown "m:ss" or why there is none ("no fix", "waiting", "by GPS", "soon"); empty while
    /// automatic beacons are off. The readback's `next=`.
    pub next: Buf<16>,
}

/// "4:32" from seconds.
fn mmss(s: u32, b: &mut Buf<16>) {
    b.clear();
    let _ = uwrite!(b, "{}:{}{}", s / 60, (s % 60) / 10, s % 10);
}

impl App {
    /// The APRS card's texts while slot C is the APRS slot (APRS-17); None otherwise.
    pub fn aprs_card(&self, data: &dyn Data) -> Option<AprsCard> {
        let holder = self.aprs_holder()?;
        let st = data.beacon();
        let fix = data.fix();
        // The beacon's countdown or why there is none, while automatic beacons are on.
        let mut next = Buf::<16>::new();
        let (mut next_warn, mut timed) = (false, false);
        if self.beacons_on() {
            if st.waiting || fix.is_none() {
                next.set(if st.waiting { b"waiting" } else { b"no fix" });
                next_warn = true;
            } else if let Some(s) = st.next_s {
                mmss(s, &mut next);
                timed = true;
            } else if self.aprs_cfg().every_min().is_some() {
                next.set(b"soon");
            } else {
                next.set(b"by GPS");
            }
        }
        let mut right = Buf::<16>::new();
        let (mut countdown, mut right_warn) = (false, false);
        let mut line = Buf::<48>::new();
        match holder {
            Holder::Mode => {
                let _ = uwrite!(right, "{} heard", data.stations());
                match data.station(0) {
                    Some(s) => {
                        let mut a = Buf::<12>::new();
                        age_text(s.ago_s, &mut a);
                        let _ = uwrite!(line, "Last {} \u{b7} {}", Txt(s.call.as_bytes()), Txt(a.as_bytes()));
                    }
                    None => line.set(b"Nothing heard yet"),
                }
            }
            Holder::Kiss => {
                right.set(if self.settings().kiss_on_cable() { b"Cable" } else { b"Bluetooth" });
                let (sent, heard) = data.kiss_counts();
                let _ = uwrite!(line, "Sent {} \u{b7} Heard {}", sent, heard);
            }
            Holder::Beacon => {
                right = next;
                (countdown, right_warn) = (timed, next_warn);
                let mut m = Buf::<24>::new();
                self.beacon_mode_text(&mut m);
                let pos: &str = match fix {
                    Some(f) if f.age_s == crate::gpsdata::FIXED_AGE => "fixed pos",
                    Some(_) => "GPS fix",
                    None => "no fix",
                };
                let _ = uwrite!(line, "{} \u{b7} {}", Txt(m.as_bytes()), pos);
            }
        }
        // APRS-20: a beacon waiting for the relay says so on the bottom line.
        let line_warn = st.waiting;
        if st.waiting {
            let mut b = Buf::<28>::new();
            self.beacon_text(data, &mut b);
            line = Buf::from(b.as_bytes());
        }
        let power = ["LOW", "MID", "HIGH"][self.aprs_cfg().power as usize % 3];
        Some(AprsCard { holder, right, countdown, right_warn, line_warn, line, power, next })
    }
}
