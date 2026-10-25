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
use crate::data::{Data, Station};
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
    /// APRS mode's two counts (APRS-17.2): stations heard since power-on and in the last hour. `right` holds one
    /// of them in turn ([`COUNT_FLIP_MS`]) for the small card; the large card shows both.
    pub counts: Option<(u16, u16)>,
}

/// APRS-17.2 (decision 282, option C): the card's two counts. "14 heard": heard since this power-on, so the
/// stations restored from flash (APRS-4) don't count; "Last ..." names the newest of them.
fn since_boot(s: &Station, up_s: u32) -> bool {
    s.ago_s <= up_s
}

/// "3 /hr": heard in the last hour by real age (a row from before a restart counts once GPS time dates it;
/// `AGO_UNKNOWN` never does).
fn in_hour(s: &Station) -> bool {
    s.ago_s <= 3600
}

/// APRS-17.2: the small card's one bold spot shows each count this long in turn.
pub const COUNT_FLIP_MS: u32 = 5000;

/// "4:32" from seconds.
fn mmss(s: u32, b: &mut Buf<16>) {
    b.clear();
    let _ = uwrite!(b, "{}:{}{}", s / 60, (s % 60) / 10, s % 10);
}

impl App {
    /// APRS-17.2: the card's counts, since power-on and in the last hour ([`since_boot`], [`in_hour`]), and the
    /// newest station heard since power-on; the readback's `heard=` and `hour=` too.
    pub fn heard_counts(&self, data: &dyn Data) -> (u16, u16, Option<Station>) {
        let up_s = self.now / 1000;
        let (mut boot, mut hour) = (0u16, 0u16);
        for s in (0..data.stations()).filter_map(|i| data.station(i)) {
            boot += since_boot(&s, up_s) as u16;
            hour += in_hour(&s) as u16;
        }
        (boot, hour, data.station(0).filter(|s| since_boot(s, up_s)))
    }

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
        let mut counts = None;
        match holder {
            Holder::Mode => {
                let (n, h, last) = self.heard_counts(data);
                counts = Some((n, h));
                if (self.now / COUNT_FLIP_MS).is_multiple_of(2) {
                    let _ = uwrite!(right, "{} heard", n);
                } else {
                    let _ = uwrite!(right, "{} /hr", h);
                }
                match last {
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
        Some(AprsCard { holder, right, countdown, right_warn, line_warn, line, power, next, counts })
    }
}
