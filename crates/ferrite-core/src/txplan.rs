//! Where transmit is allowed (manual chapter 15): each region's amateur bands (TX-1), the bands in 18-64 MHz (TX-4), the
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

/// A region's amateur bands in 18-64 MHz (TX-4), as stock's HF work mode allows transmit on all of 18-64 MHz:
/// 17 m, 15 m, 12 m and 10 m everywhere, and 6 m by the region's allocation: 50-54 MHz in the Americas, Australia,
/// New Zealand and Japan, 50-52 MHz in the UK, Europe and elsewhere (IARU Region 1's common part).
pub fn hf(r: Region) -> &'static [TxBand] {
    const HF4: [TxBand; 4] = [
        b("17 m", mhz(18) + 6_800, mhz(18) + 16_800),
        b("15 m", mhz(21), mhz(21) + 45_000),
        b("12 m", mhz(24) + 89_000, mhz(24) + 99_000),
        b("10 m", mhz(28), mhz(29) + 70_000),
    ];
    const W54: [TxBand; 5] = [HF4[0], HF4[1], HF4[2], HF4[3], b("6 m", mhz(50), mhz(54))];
    const W52: [TxBand; 5] = [HF4[0], HF4[1], HF4[2], HF4[3], b("6 m", mhz(50), mhz(52))];
    match r {
        Region::Us | Region::Canada | Region::Australia | Region::NewZealand | Region::Japan => &W54,
        Region::Uk | Region::Europe | Region::Other => &W52,
    }
}

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
    /// The TX frequency needs the band relay in the other state from RX (a split or shift across 64 MHz): the
    /// relay is never switched at key-up (TX-37).
    RelayMismatch,
    /// The slot is in AM: the BK4829 has no AM modulator (TX-20).
    AmRxOnly,
    /// An emergency frequency without a person's second PTT (TX-3, TX-35).
    Emergency,
    /// The timeout ended the last transmission and PTT is still held (TX-30).
    ReleasePtt,
    /// No PTT input has read released since power-on (SAFE-14).
    PttAtBoot,
    /// The duty limit: the transmitter is cooling (TX-31).
    Hot,
    /// The SI4732 is still playing a broadcast at key-up (TX-24: the UI stops it first, BCR-29; the Radio task's
    /// backstop, TX audit F11).
    Broadcast,
}

/// TX-37: a pair is allowed only when RX and TX need the band relay in the same state.
pub fn relay_ok(rx: Freq10, tx: Freq10) -> bool {
    crate::freq::needs_relay(rx) == crate::freq::needs_relay(tx)
}

/// Where transmit may go: the region's bands (TX-1) and its amateur bands in 18-64 MHz (TX-4), or, with Extend TX,
/// any frequency the hardware covers as stock's windows draw it (TX-41): all of 18-64 MHz as stock's HF work mode,
/// 136-174, 200-260 and 400-520 MHz. The calibration's own windows are intersected after this, in the Radio task's
/// sequencer (`TxCal::in_window`, `txseq::Req::cal_window`): the UI doesn't read them.
pub fn check(f: Freq10, region: Region, extend: bool) -> Result<(), Refusal> {
    if extend {
        // Calibration ∩ hardware: the TX chips' ranges; 18-64 MHz is stock's HF-mode window (CB included).
        let hw = [(18, 64), (136, 174), (200, 260), (400, 520)].iter().any(|&(lo, hi)| (mhz(lo)..mhz(hi)).contains(&f));
        return if hw { Ok(()) } else { Err(Refusal::NotCalibrated) };
    }
    if bands(region).iter().chain(hf(region)).any(|b| (b.lo..b.hi).contains(&f)) {
        Ok(())
    } else {
        Err(Refusal::OutOfBand)
    }
}

/// The region's band nearest f, for "outside the 70 cm band in Canada (430-450)".
pub fn nearest(f: Freq10, region: Region) -> TxBand {
    let d = |b: &TxBand| if f < b.lo { b.lo - f } else { f.saturating_sub(b.hi) };
    *bands(region).iter().chain(hf(region)).min_by_key(|b| d(b)).unwrap_or(&hf(region)[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TX-1.1: the region's band plan: inside it transmit is allowed, outside it refused.
    #[test]
    fn region_plans() {
        assert_eq!(check(mhz(146) + 52_000, Region::Us, false), Ok(()));
        assert_eq!(check(mhz(420) + 50_000, Region::Canada, false), Err(Refusal::OutOfBand));
        assert_eq!(check(mhz(420) + 50_000, Region::Us, false), Ok(()));
        assert_eq!(check(mhz(147), Region::Uk, false), Err(Refusal::OutOfBand));
        assert_eq!(check(mhz(28) + 40_000, Region::Japan, false), Ok(()));
        assert_eq!(check(mhz(162) + 55_000, Region::Us, true), Ok(()));
        assert_eq!(nearest(mhz(420) + 50_000, Region::Canada).name, "70 cm");
    }

    /// TX-9, TX-10.4, TX-10.5, SAFE-12.4: 1.25 m transmits like any band where the region has it (the US 222-225,
    /// Canada 219-225); Extend TX opens stock's 200-260 MHz window, 243.0 MHz included (its double PTT is TX-3's, in
    /// the sequencer), and stops at 260 MHz.
    #[test]
    fn band_125m_as_stock() {
        assert_eq!(check(mhz(223) + 50_000, Region::Us, false), Ok(()));
        assert_eq!(check(mhz(219) + 50_000, Region::Canada, false), Ok(()));
        assert_eq!(check(mhz(219) + 50_000, Region::Us, false), Err(Refusal::OutOfBand));
        assert_eq!(check(mhz(223) + 50_000, Region::Uk, false), Err(Refusal::OutOfBand), "no 1.25 m in the UK");
        assert_eq!(check(mhz(243), Region::Us, true), Ok(()));
        assert_eq!(check(mhz(200), Region::Uk, true), Ok(()));
        assert_eq!(check(mhz(260), Region::Us, true), Err(Refusal::NotCalibrated), "stock's window ends at 260");
    }

    /// TX-4, TX-41, SAFE-12.4: the region's amateur bands in 18-64 MHz (17 m to 6 m, 6 m by the region's allocation); with
    /// Extend TX, all of 18-64 MHz as stock's HF work mode, CB included; below 18 MHz never.
    #[test]
    fn hf_as_stock() {
        for f in [mhz(18) + 10_000, mhz(21) + 20_000, mhz(24) + 94_000, mhz(28) + 40_000, mhz(51)] {
            assert_eq!(check(f, Region::Uk, false), Ok(()), "{f}");
        }
        assert_eq!(check(mhz(52) + 52_500, Region::Us, false), Ok(()), "6 m: 50-54 in the US");
        assert_eq!(check(mhz(53), Region::Japan, false), Ok(()), "and Japan");
        assert_eq!(check(mhz(52) + 52_500, Region::Europe, false), Err(Refusal::OutOfBand), "50-52 in Europe");
        assert_eq!(check(mhz(27) + 18_500, Region::Us, false), Err(Refusal::OutOfBand), "CB: Extend TX only");
        assert_eq!(check(mhz(21) + 45_000, Region::Us, false), Err(Refusal::OutOfBand), "15 m's top edge");
        assert_eq!(check(mhz(27) + 18_500, Region::Us, true), Ok(()), "CB with Extend TX, as stock");
        assert_eq!(check(mhz(18), Region::Us, true), Ok(()));
        assert_eq!(check(mhz(64) - 1, Region::Us, true), Ok(()));
        assert_eq!(check(mhz(18) - 1, Region::Us, true), Err(Refusal::NotCalibrated), "below 18 MHz: never");
        assert_eq!(check(mhz(64), Region::Us, true), Err(Refusal::NotCalibrated), "64-136 MHz: never");
        assert_eq!(check(mhz(121) + 50_000, Region::Us, true), Err(Refusal::NotCalibrated), "airband");
        assert_eq!(nearest(mhz(55), Region::Us).name, "6 m");
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
