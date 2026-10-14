//! The factory calibration's transmit side, read only (manual chapter 35, `analysis/calibration.md`): the PA drive
//! curves at 0x00F000-0x00F1BF, the battery gauge at 0x00F200 and the TX windows at 0x00F230/0x00F240, as stock
//! uses them. The Radio task's `tx` build asks [`TxCal::dac`] for the PA drive (DAC2, 8 bits) at key-up,
//! [`TxCal::in_window`] for the windows (CAL-11) and [`Gauge::tx_ok`] for the battery (PWR-20).
//! Nothing here writes anything: the calibration area is never written (SAFE).

use crate::freq::{mhz, Freq10};

/// Bytes read from the start of the calibration sector (0x00F000): the PA curves to 0x1BF, the gauge at 0x200,
/// the tuning block at 0x210 and the TX windows to 0x24F.
pub const READ_LEN: usize = 0x250;
/// Where the blank test looks (CAL-5): 0xF0E0 byte 0 = 0xFF is an erased sector.
const BLANK_AT: usize = 0xE0;
const GAUGE_AT: usize = 0x200;
/// The TX windows (CAL-11): 0xF230 holds windows 1-3 (stock's VHF, UHF and 220 classes) at offsets 0, 5 and 10,
/// 0xF240 window 4 (350), five bytes each: the enable byte (1 on), then the low and the high edge, each two bytes of
/// BCD giving whole MHz (01 36 = 136 MHz). Stock's loader, `analysis/calibration.md`.
const WINDOWS_AT: [usize; 4] = [0x230, 0x235, 0x23A, 0x240];

/// Why the calibration locks transmit (TX-13, TX-14, CAL-5, CAL-6): the Radio task gets no [`TxCal`], and every
/// key-up is refused as Locked; receive is unaffected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// 0xF0E0 byte 0 = 0xFF: an erased sector (CAL-5).
    Blank,
    /// A PA curve entry the amateur bands reach is 0x00 or 0xFF (an erased or unprogrammed byte).
    PaCurve,
    /// The TX windows: blank (0xF230 byte 0 = 0xFF, where stock falls back to its built-in windows, which Ferrite
    /// never has: CAL-4), an enable byte other than 0x00, 0x01 or 0xFF, or an enabled window that isn't BCD with
    /// low < high (TX-14).
    Windows,
}

/// A BCD byte's value, None if a digit is over 9.
fn bcd(b: u8) -> Option<u16> {
    (b >> 4 < 10 && b & 0x0F < 10).then_some((b >> 4) as u16 * 10 + (b & 0x0F) as u16)
}

/// One TX window from its five bytes: Ok(None) when off (enable 0x00, or 0xFF: an erased 0xF240), the edges in MHz
/// when on (enable 0x01, the only value stock's check takes).
fn window(w: &[u8]) -> Result<Option<(u16, u16)>, Fault> {
    let edge = |hi: u8, lo: u8| Some(bcd(hi)? * 100 + bcd(lo)?);
    match w[0] {
        0x00 | 0xFF => Ok(None),
        0x01 => match (edge(w[1], w[2]), edge(w[3], w[4])) {
            (Some(lo), Some(hi)) if lo < hi => Ok(Some((lo, hi))),
            _ => Err(Fault::Windows),
        },
        _ => Err(Fault::Windows),
    }
}
/// Stock's gauge when 0xF200 byte 0 is 0x00 or 0xFF; the seventh is 0xBB whatever the sector says.
const GAUGE_DEFAULT: [u8; 7] = [0x81, 0x87, 0x8E, 0x93, 0x98, 0xA0, 0xBB];

/// PWR-20: the battery in mV from the 12-bit PA1 reading: (reading >> 4) x 48.2 mV, before the user trim. Stock's
/// factor; the black radio's full pack read 8.312 V on a meter (Oct 9, 2026), which is reading 172.
pub fn battery_mv(adc: u16) -> u16 {
    ((adc >> 4) as u32 * 482 / 10) as u16
}

/// The PA drive curves: 3 levels (stock order: 0 High, 1 Mid, 2 Low) x 4 rows (UHF 400 MHz up in 10 MHz steps,
/// VHF 130-180 in 3 MHz, 200-300 in 5 MHz, 350-400 in 5 MHz) x 16, and the HF rows (15 MHz up in 5 MHz, 3 levels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCal {
    pa: [[[u8; 16]; 4]; 3],
    hf: [[u8; 16]; 3],
    /// The enabled TX windows, low and high edge in MHz; (0, 0) for a window that is off.
    win: [(u16, u16); 4],
}

const UHF: usize = 0;
const VHF: usize = 1;
const R200: usize = 2;
const R350: usize = 3;

impl TxCal {
    /// The curves and windows from the sector's first [`READ_LEN`] bytes; None when [`TxCal::read`] finds a fault.
    pub fn parse(b: &[u8; READ_LEN]) -> Option<TxCal> {
        TxCal::read(b).ok()
    }

    /// The curves and windows, or why transmit is locked: the sector blank (CAL-5), a curve the radio's bands use
    /// holding a 0 or 0xFF (CAL-6's plausibility), the TX windows blank or invalid (TX-14).
    pub fn read(b: &[u8; READ_LEN]) -> Result<TxCal, Fault> {
        if b[BLANK_AT] == 0xFF {
            return Err(Fault::Blank);
        }
        let mut c = TxCal { pa: [[[0; 16]; 4]; 3], hf: [[0; 16]; 3], win: [(0, 0); 4] };
        for (l, level) in c.pa.iter_mut().enumerate() {
            for (r, row) in level.iter_mut().enumerate() {
                let at = 0x40 * l + 0x10 * r;
                row.copy_from_slice(&b[at..at + 16]);
            }
        }
        for (l, row) in c.hf.iter_mut().enumerate() {
            let at = 0x190 + 0x10 * l;
            row.copy_from_slice(&b[at..at + 16]);
        }
        // The entries the lookup can reach on the amateur bands: UHF 0-12, VHF 0-15, HF 0-11, every level.
        let bad = |v: &[u8]| v.iter().any(|&x| x == 0 || x == 0xFF);
        let reached = c.pa.iter().any(|l| bad(&l[UHF][..13]) || bad(&l[VHF])) || c.hf.iter().any(|r| bad(&r[..12]));
        if reached {
            return Err(Fault::PaCurve);
        }
        if b[WINDOWS_AT[0]] == 0xFF {
            return Err(Fault::Windows);
        }
        for (w, &at) in c.win.iter_mut().zip(WINDOWS_AT.iter()) {
            *w = window(&b[at..at + 5])?.unwrap_or((0, 0));
        }
        Ok(c)
    }

    /// CAL-11, TX-1: `f` lies in one of this radio's enabled TX windows, low <= f < high as stock checks it
    /// (0x0800BD8C). Below 64 MHz the calibration has no window (TX-41): `txplan`'s HF windows alone decide there.
    pub fn in_window(&self, f: Freq10) -> bool {
        f < mhz(64) || self.win.iter().any(|&(lo, hi)| (mhz(lo as u32)..mhz(hi as u32)).contains(&f))
    }

    /// The PA drive for transmitting on `f` at Ferrite's `power` (0 Low, 1 Mid, 2 High), stock's lookup
    /// (`pa_cal_lookup`): the row by frequency, the entry by its step, clamped as stock clamps, except that a
    /// frequency stock reads past a row for (280-300 MHz, 560 MHz up) takes the row's last entry instead.
    pub fn dac(&self, f: Freq10, power: u8) -> u8 {
        let level = 2 - power.min(2) as usize; // stock's order: 0 is High
        let mhz = f / 100_000;
        let p = &self.pa[level];
        match mhz {
            400.. => p[UHF][(((mhz - 400) / 10) as usize).min(15)],
            350..=399 => p[R350][((mhz - 350) / 5) as usize],
            300..=349 => p[R350][0],
            // CAL-14, TX-9: the 200-300 MHz PA path (1.25 m, and 243.0 MHz) takes the Low curve whatever the
            // level; the bench-measured 0.25 W ceiling clamps it further once known (TX-10).
            200..=299 => self.pa[2][R200][(((mhz - 200) / 5) as usize).min(15)],
            130..=199 => p[VHF][(((mhz - 130) / 3) as usize).min(15)],
            18..=63 => self.hf[level][(((mhz - 15) / 5) as usize).min(11)],
            // Below 18 MHz and 64-130 MHz: entry 0 of the High VHF row, whatever the level (stock).
            _ => self.pa[0][VHF][0],
        }
    }
}

/// The battery gauge (PWR-20): seven thresholds on the PA1 reading >> 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gauge(pub [u8; 7]);

impl Gauge {
    /// 0xF200's seven bytes, or stock's defaults when byte 0 is 0x00 or 0xFF; the seventh is always 0xBB.
    pub fn parse(b: &[u8; READ_LEN]) -> Gauge {
        let mut g = [0u8; 7];
        g.copy_from_slice(&b[GAUGE_AT..GAUGE_AT + 7]);
        if g[0] == 0 || g[0] == 0xFF {
            g = GAUGE_DEFAULT;
        }
        g[6] = 0xBB;
        Gauge(g)
    }

    /// Transmit allowed on this reading (the 12-bit PA1 conversion): at or above the first threshold (the
    /// cut-off, about 6.2 V) and not above the seventh (about 9 V, stock's level 7).
    /// PWR-20's level 0-6 for the PA1 reading: thresholds at or below it (stock's gauge), as a percentage.
    pub fn percent(&self, adc: u16) -> u8 {
        let v = (adc >> 4).min(255) as u8;
        (self.0[..6].iter().filter(|&&t| v >= t).count() * 100 / 6) as u8
    }

    /// Stock's defaults, for builds that don't read the calibration (the receive-only build reads no PA curves).
    pub const fn defaults() -> Gauge {
        Gauge(GAUGE_DEFAULT)
    }

    pub fn tx_ok(&self, adc: u16) -> bool {
        let v = (adc >> 4).min(255) as u8;
        v >= self.0[0] && v <= self.0[6]
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Gauge(GAUGE_DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The black radio's tuning block and TX windows (0xF210-0xF24F, `backups/blackradio/cal.bin`): 136-174,
    /// 400-520, 200-260 and 330-400 MHz, all enabled; no crystal trim (byte 11 0xFF).
    const BLACK_F210: [u8; 0x40] = [
        0x37, 0x46, 0x3e, 0x1f, 0x1f, 0x28, 0x2a, 0x2a, 0x2a, 0x01, 0x08, 0xff, 0x00, 0x00, 0xff, 0xff, //
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, //
        0x01, 0x01, 0x36, 0x01, 0x74, 0x01, 0x04, 0x00, 0x05, 0x20, 0x01, 0x02, 0x00, 0x02, 0x60, 0xff, //
        0x01, 0x03, 0x30, 0x04, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, //
    ];

    /// A sector with every curve entry `base + level * 10 + index`, the gauge given, and the black radio's
    /// tuning block and windows.
    fn sector(gauge: [u8; 7]) -> [u8; READ_LEN] {
        let mut b = [0x55u8; READ_LEN];
        for l in 0..3 {
            for r in 0..4 {
                for i in 0..16 {
                    b[0x40 * l + 0x10 * r + i] = (100 + l * 40 + r * 4 + i) as u8;
                }
            }
            for i in 0..16 {
                b[0x190 + 0x10 * l + i] = (60 + l * 40 + i) as u8;
            }
        }
        b[BLANK_AT] = 0x00;
        b[GAUGE_AT..GAUGE_AT + 7].copy_from_slice(&gauge);
        b[0x210..0x250].copy_from_slice(&BLACK_F210);
        b
    }

    /// TX-13, CAL-4, CAL-5: calibration is checked, never replaced: an erased sector or a hole in a used row locks
    /// transmit instead of falling back to built-in curves.
    #[test]
    fn blank_and_holes_refuse() {
        let mut b = sector(GAUGE_DEFAULT);
        assert!(TxCal::parse(&b).is_some());
        b[BLANK_AT] = 0xFF;
        assert_eq!(TxCal::parse(&b), None, "CAL-5: an erased sector");
        let mut b = sector(GAUGE_DEFAULT);
        b[0x40 + 0x10 * VHF + 5] = 0xFF;
        assert_eq!(TxCal::parse(&b), None, "a hole in the Mid VHF row");
        let mut b = sector(GAUGE_DEFAULT);
        b[UHF * 0x10 + 14] = 0xFF;
        assert!(TxCal::parse(&b).is_some(), "UHF entries past 520 MHz aren't reached on amateur bands");
    }

    /// CAL-11, TX-1, TX-14: the windows as stock checks them (low <= f < high, whole MHz from BCD), the black
    /// radio's four; below 64 MHz no window applies (TX-41).
    #[test]
    fn tx_windows_as_stock() {
        let c = TxCal::read(&sector(GAUGE_DEFAULT)).unwrap();
        for (f, inside) in [
            (mhz(146) + 52_000, true),
            (mhz(136), true),
            (mhz(136) - 1, false),
            (mhz(174) - 1, true),
            (mhz(174), false),
            (mhz(190), false),
            (mhz(223) + 50_000, true),
            (mhz(260), false),
            (mhz(330), true),
            (mhz(446), true),
            (mhz(520), false),
            (mhz(28) + 40_000, true),
            (mhz(52), true),
        ] {
            assert_eq!(c.in_window(f), inside, "{f}");
        }
        // A window turned off (enable 0x00) is out, whatever its edges hold.
        let mut b = sector(GAUGE_DEFAULT);
        b[0x235] = 0x00;
        b[0x236] = 0xAB;
        let c = TxCal::read(&b).unwrap();
        assert!(!c.in_window(mhz(446)) && c.in_window(mhz(146)));
        // An erased 0xF240: window 4 off, the rest kept.
        let mut b = sector(GAUGE_DEFAULT);
        b[0x240..0x250].fill(0xFF);
        let c = TxCal::read(&b).unwrap();
        assert!(!c.in_window(mhz(350)) && c.in_window(mhz(446)));
    }

    /// TX-13, TX-14, CAL-4: blank or invalid windows lock transmit instead of falling back to stock's built-in
    /// ones: 0xF230 byte 0 erased, a digit over 9, low not below high, an enable byte stock doesn't write.
    #[test]
    fn bad_windows_lock() {
        let bad = |at: usize, v: u8| {
            let mut b = sector(GAUGE_DEFAULT);
            b[at] = v;
            TxCal::read(&b).err()
        };
        assert_eq!(bad(0x230, 0xFF), Some(Fault::Windows), "blank: never stock's built-in windows");
        assert_eq!(bad(0x232, 0x3A), Some(Fault::Windows), "not BCD");
        assert_eq!(bad(0x238, 0x03), Some(Fault::Windows), "UHF high edge 320, below its low edge 400");
        assert_eq!(bad(0x234, 0x36), Some(Fault::Windows), "VHF low = high (136)");
        assert_eq!(bad(0x23A, 0x02), Some(Fault::Windows), "enable byte 2");
        assert_eq!(bad(0x234, 0x75), None, "136-175 MHz: still a window");
        assert!(TxCal::parse(&sector(GAUGE_DEFAULT)).is_some());
    }

    /// CAL-14: the 1.25 m PA path (200-300 MHz, 243.0 MHz included) takes the Low curve at every level, so the
    /// emergency frequency's double PTT can't drive it at High.
    #[test]
    fn band_125m_low_curve_only() {
        let c = TxCal::parse(&sector(GAUGE_DEFAULT)).unwrap();
        for f in [mhz(223) + 50_000, mhz(243), mhz(299)] {
            let low = c.dac(f, 0);
            assert_eq!(c.dac(f, 1), low);
            assert_eq!(c.dac(f, 2), low);
        }
        assert_ne!(c.dac(mhz(146), 2), c.dac(mhz(146), 0), "other bands keep their levels");
    }

    /// TX-8, CAL-8, CAL-9, TX-11, CAL-10: the PA drive comes only from the band's calibration curve at the chosen
    /// level (1.25 m: the Low curve, CAL-14), by stock's lookup (High is stock's row 0, Low its row 2), and never reads past its row's 16 entries.
    #[test]
    fn lookup_as_stock() {
        let c = TxCal::parse(&sector(GAUGE_DEFAULT)).unwrap();
        // High is stock's level 0: 146.52 MHz is VHF entry (146-130)/3 = 5.
        assert_eq!(c.dac(mhz(146) + 52_000, 2), 100 + VHF as u8 * 4 + 5);
        assert_eq!(c.dac(mhz(146) + 52_000, 0), 100 + 80 + VHF as u8 * 4 + 5, "Low is stock's level 2");
        assert_eq!(c.dac(mhz(446), 1), 100 + 40 + 4, "UHF entry (446-400)/10 = 4, Mid");
        assert_eq!(c.dac(mhz(190), 2), 100 + VHF as u8 * 4 + 15, "175-200 MHz: entry 15");
        assert_eq!(c.dac(mhz(223) + 50_000, 2), 100 + 80 + R200 as u8 * 4 + 4, "1.25 m: the Low curve (CAL-14)");
        assert_eq!(c.dac(mhz(28) + 40_000, 2), 60 + 2, "10 m: HF entry (28-15)/5 = 2");
        assert_eq!(c.dac(mhz(70), 0), 100 + VHF as u8 * 4, "64-130 MHz: High VHF entry 0, any level");
        assert_eq!(c.dac(mhz(290), 2), 100 + 80 + R200 as u8 * 4 + 15, "280-300: the row's last entry, not the next row");
    }

    /// PWR-20: the battery gauge is stock's: the PA1 reading against the seven thresholds at 0xF200 (stock's
    /// defaults when blank), transmit refused below the cut-off and above about 9 V.
    #[test]
    fn gauge() {
        let g = Gauge::parse(&sector([0xFF; 7]));
        assert_eq!(g, Gauge::default(), "blank: stock's defaults");
        assert!(!g.tx_ok(0x80 << 4), "below the cut-off");
        assert!(g.tx_ok(0x81 << 4) && g.tx_ok(0xBB << 4));
        assert!(!g.tx_ok(0xBC << 4), "over about 9 V");
        let g = Gauge::parse(&sector([0x70, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0]));
        assert_eq!(g.0[6], 0xBB, "the seventh is always 0xBB");
        assert!(g.tx_ok(0x70 << 4));
    }
}

#[cfg(test)]
mod battery_tests {
    use super::*;

    /// PWR-20: stock's factor and gauge; the black radio's 8.312 V pack sits on the top level.
    #[test]
    fn battery_reading() {
        assert_eq!(battery_mv(172 << 4), 8290);
        let g = Gauge::defaults();
        assert_eq!(g.percent(172 << 4), 100);
        assert_eq!(g.percent(0x80 << 4), 0);
        assert_eq!(g.percent(0x93 << 4), 66);
    }
}
