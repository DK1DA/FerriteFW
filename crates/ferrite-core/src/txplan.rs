//! Where transmit is allowed (manual chapter 15): each region's amateur bands (TX-1), the HF windows (TX-4), the
//! emergency frequencies (TX-3) and the reasons a transmission is refused (TX-20). Plain tables and checks, so
//! the UI can say why before keying and the Radio task can apply the same rules when it keys (TX-17).

use crate::freq::{mhz, Freq10};

/// Region (MENU 0 9 4, setup page 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Region {
    Us,
    Canada,
    Uk,
    Europe,
    Australia,
    NewZealand,
    Japan,
    Other,
}

impl Region {
    pub const ALL: [Region; 8] =
        [Region::Us, Region::Canada, Region::Uk, Region::Europe, Region::Australia, Region::NewZealand, Region::Japan, Region::Other];

    pub fn name(self) -> &'static str {
        match self {
            Region::Us => "the US",
            Region::Canada => "Canada",
            Region::Uk => "the UK",
            Region::Europe => "Europe",
            Region::Australia => "Australia",
            Region::NewZealand => "New Zealand",
            Region::Japan => "Japan",
            Region::Other => "this region",
        }
    }

    /// Weather radio and SAME exist only here (WX-1, WX-3).
    pub fn has_weather(self) -> bool {
        matches!(self, Region::Us | Region::Canada)
    }
}

/// One transmit band: its name and edges (10 Hz units).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxBand {
    pub name: &'static str,
    pub lo: Freq10,
    pub hi: Freq10,
}

const fn b(name: &'static str, lo: Freq10, hi: Freq10) -> TxBand {
    TxBand { name, lo, hi }
}

/// 12 m and 10 m, the same everywhere (TX-4).
const HF: [TxBand; 2] = [b("12 m", mhz(24) + 89_000, mhz(24) + 99_000), b("10 m", mhz(28), mhz(29) + 70_000)];

/// A region's VHF/UHF amateur bands (TX-1's table): 2 m, 1.25 m where it exists, 70 cm.
pub fn bands(r: Region) -> &'static [TxBand] {
    const US: [TxBand; 3] = [b("2 m", mhz(144), mhz(148)), b("1.25 m", mhz(222), mhz(225)), b("70 cm", mhz(420), mhz(450))];
    const CA: [TxBand; 3] = [b("2 m", mhz(144), mhz(148)), b("1.25 m", mhz(219), mhz(225)), b("70 cm", mhz(430), mhz(450))];
    const AU: [TxBand; 2] = [b("2 m", mhz(144), mhz(148)), b("70 cm", mhz(420), mhz(450))];
    const NZ: [TxBand; 2] = [b("2 m", mhz(144), mhz(148)), b("70 cm", mhz(430), mhz(440))];
    const ITU: [TxBand; 2] = [b("2 m", mhz(144), mhz(146)), b("70 cm", mhz(430), mhz(440))];
    match r {
        Region::Us => &US,
        Region::Canada => &CA,
        Region::Australia => &AU,
        Region::NewZealand => &NZ,
        Region::Uk | Region::Europe | Region::Japan | Region::Other => &ITU,
    }
}

/// The emergency frequencies, each ±25 kHz (TX-3): 406.0-406.1, 243.0 and 121.5 MHz.
pub fn emergency(f: Freq10) -> bool {
    const M: Freq10 = 2_500;
    (mhz(406) - M..=mhz(406) + 10_000 + M).contains(&f)
        || (mhz(243) - M..=mhz(243) + M).contains(&f)
        || (mhz(121) + 50_000 - M..=mhz(121) + 50_000 + M).contains(&f)
}

/// Why a transmission is refused (TX-20, HF-24, XB-14), each with the fix the refused screen names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// Outside the region's bands (with Extend TX off).
    OutOfBand,
    /// Outside what the calibration and hardware allow (Extend TX on, or nowhere at all).
    NotCalibrated,
    RxOnlyChannel,
    /// Busy lockout: the channel is in use.
    Busy,
    Battery,
    /// The calibration sector or the MCU check failed (TX-13 to TX-15).
    Locked,
    /// Another transmission is in progress.
    InProgress,
    /// Automatic transmit without a callsign.
    NoCallsign,
    /// This build has no transmit code.
    RxOnlyBuild,
    /// HF mode on an SI4732 frequency or mode (HF-24).
    ReceiveOnlyHere,
    /// The crossband repeater is running (XB-14).
    Crossband,
    /// 1.25 m before its 0.25 W ceiling is measured on the bench (TX-10).
    Unmeasured125,
    /// The TX frequency needs the band relay in the other state from RX (a split or shift across 64 MHz): the
    /// relay is never switched at key-up (TX-37, as nicFW refuses it).
    RelayMismatch,
    /// The slot is in AM: the BK4829 has no AM modulator (TX-20).
    AmRxOnly,
    /// An emergency frequency without a person's second PTT (TX-3, TX-35).
    Emergency,
    /// The timeout ended the last transmission and PTT is still held (TX-30).
    ReleasePtt,
    /// The duty limit: the transmitter is cooling (TX-31).
    Hot,
}

/// TX-37: a pair is allowed only when RX and TX need the band relay in the same state.
pub fn relay_ok(rx: Freq10, tx: Freq10) -> bool {
    crate::freq::needs_relay(rx) == crate::freq::needs_relay(tx)
}

/// TX-10: 1.25 m transmits only once the PA drive ceiling that holds it to 0.25 W (TX-9) has been measured into
/// a dummy load. Until then this stays false and every transmission there is refused, by the UI and (once
/// transmit is built) the Radio task alike. Set it in the same change that adds the measured ceiling.
pub const CAP_125M_MEASURED: bool = false;

/// The 1.25 m band as TX-9 draws it for the cap: 200-300 MHz.
pub fn is_125m(f: Freq10) -> bool {
    (mhz(200)..mhz(300)).contains(&f)
}

/// TX-10: transmit anywhere on the 200-300 MHz PA path (the 1.25 m band, and 243.0 MHz) is held back until the
/// ceiling is measured: nothing there is capped yet, and the path's Low curve alone isn't 0.25 W (TX audit M3).
/// Once measured, the 0.25 W cap (TX-9) covers the whole path and 243.0 MHz takes the emergency double PTT (TX-3).
pub fn locked_125m(f: Freq10) -> bool {
    !CAP_125M_MEASURED && is_125m(f)
}

/// Where transmit may go: the region's bands (or, with Extend TX, any band the hardware covers) and the HF
/// windows. Extend TX never reaches below 64 MHz: the calibration has no HF window, so HF stays on the amateur
/// windows (TX-41). The calibration's own windows are intersected after this, in the Radio task's sequencer
/// (`TxCal::in_window`, `txseq::Req::cal_window`): the UI doesn't read them.
pub fn check(f: Freq10, region: Region, extend: bool) -> Result<(), Refusal> {
    if extend && f >= mhz(64) {
        // Calibration ∩ hardware: the TX chips' ranges (6 m and below 24 MHz stay locked, TX-4).
        let hw = (mhz(136)..mhz(174)).contains(&f) || (mhz(200)..mhz(260)).contains(&f) || (mhz(400)..mhz(520)).contains(&f);
        return if hw || HF.iter().any(|b| (b.lo..b.hi).contains(&f)) { Ok(()) } else { Err(Refusal::NotCalibrated) };
    }
    if bands(region).iter().chain(HF.iter()).any(|b| (b.lo..b.hi).contains(&f)) {
        Ok(())
    } else {
        Err(Refusal::OutOfBand)
    }
}

/// The region's band nearest f, for "outside the 70 cm band in Canada (430-450)".
pub fn nearest(f: Freq10, region: Region) -> TxBand {
    let d = |b: &TxBand| if f < b.lo { b.lo - f } else { f.saturating_sub(b.hi) };
    *bands(region).iter().chain(HF.iter()).min_by_key(|b| d(b)).unwrap_or(&HF[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_plans() {
        assert_eq!(check(mhz(146) + 52_000, Region::Us, false), Ok(()));
        assert_eq!(check(mhz(420) + 50_000, Region::Canada, false), Err(Refusal::OutOfBand));
        assert_eq!(check(mhz(420) + 50_000, Region::Us, false), Ok(()));
        assert_eq!(check(mhz(147), Region::Uk, false), Err(Refusal::OutOfBand));
        assert_eq!(check(mhz(28) + 40_000, Region::Japan, false), Ok(()));
        assert_eq!(check(mhz(52), Region::Us, true), Err(Refusal::OutOfBand), "6 m locked");
        assert_eq!(check(mhz(162) + 55_000, Region::Us, true), Ok(()));
        assert_eq!(nearest(mhz(420) + 50_000, Region::Canada).name, "70 cm");
    }

    /// TX-4, TX-41: below 64 MHz only 12 m and 10 m transmit, Extend TX or not; CB and 15 m never; 6 m locked.
    #[test]
    fn extend_stops_at_64_mhz() {
        assert_eq!(check(mhz(27) + 18_500, Region::Us, true), Err(Refusal::OutOfBand), "CB, even with Extend TX");
        assert_eq!(check(mhz(21) + 20_000, Region::Us, true), Err(Refusal::OutOfBand), "15 m");
        assert_eq!(check(mhz(28) + 40_000, Region::Us, true), Ok(()), "10 m stays open");
        assert_eq!(check(mhz(24) + 94_000, Region::Uk, true), Ok(()), "12 m stays open");
        assert_eq!(check(mhz(162) + 55_000, Region::Us, true), Ok(()), "above 64 MHz Extend still widens");
    }

    /// CAL-15, TX-10: no transmit anywhere on the 200-300 MHz PA path until its power ceiling is measured: the
    /// 1.25 m band and 243.0 MHz alike (TX audit M3); the edges are the path's.
    #[test]
    fn unmeasured_125m_is_locked() {
        assert!(locked_125m(mhz(223) + 50_000));
        assert!(locked_125m(mhz(243)), "243.0 MHz: on the same uncapped path");
        assert!(locked_125m(mhz(200)) && locked_125m(mhz(300) - 1));
        assert!(!locked_125m(mhz(200) - 1) && !locked_125m(mhz(300)));
        assert!(!locked_125m(mhz(146) + 52_000) && !locked_125m(mhz(446)));
        assert!(is_125m(mhz(243)), "the 0.25 W cap will apply there too");
    }

    #[test]
    fn emergency_frequencies() {
        assert!(emergency(mhz(121) + 50_000));
        assert!(emergency(mhz(243) + 2_000));
        assert!(emergency(mhz(406) + 5_000));
        assert!(!emergency(mhz(121) + 60_000));
        assert!(!emergency(mhz(146) + 52_000));
    }

    #[test]
    fn relay_pairs() {
        assert!(relay_ok(mhz(446), mhz(146) + 52_000), "cross-band, same relay state");
        assert!(!relay_ok(mhz(146) + 52_000, mhz(50) + 12_500), "2 m RX, 6 m TX");
        assert!(!relay_ok(mhz(28) + 40_000, mhz(146) + 52_000), "10 m RX, 2 m TX");
        assert!(relay_ok(mhz(28) + 40_000, mhz(29) + 60_000));
    }
}
