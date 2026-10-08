//! The factory calibration's transmit side, read only (manual chapter 35, `analysis/calibration.md`): the PA drive
//! curves at 0x00F000-0x00F1BF and the battery gauge at 0x00F200, as stock uses them. The Radio task's `tx` build
//! asks [`TxCal::dac`] for the PA drive (DAC2, 8 bits) at key-up and [`Gauge::tx_ok`] for the battery (PWR-20).
//! Nothing here writes anything: the calibration area is never written (SAFE).

use crate::freq::Freq10;

/// Bytes read from the start of the calibration sector (0x00F000): the PA curves to 0x1BF and the gauge at 0x200.
pub const READ_LEN: usize = 0x208;
/// Where the blank test looks (CAL-5): 0xF0E0 byte 0 = 0xFF is an erased sector.
const BLANK_AT: usize = 0xE0;
const GAUGE_AT: usize = 0x200;
/// Stock's gauge when 0xF200 byte 0 is 0x00 or 0xFF; the seventh is 0xBB whatever the sector says.
const GAUGE_DEFAULT: [u8; 7] = [0x81, 0x87, 0x8E, 0x93, 0x98, 0xA0, 0xBB];

/// The PA drive curves: 3 levels (stock order: 0 High, 1 Mid, 2 Low) x 4 rows (UHF 400 MHz up in 10 MHz steps,
/// VHF 130-180 in 3 MHz, 200-300 in 5 MHz, 350-400 in 5 MHz) x 16, and the HF rows (15 MHz up in 5 MHz, 3 levels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCal {
    pa: [[[u8; 16]; 4]; 3],
    hf: [[u8; 16]; 3],
}

const UHF: usize = 0;
const VHF: usize = 1;
const R200: usize = 2;
const R350: usize = 3;

impl TxCal {
    /// The curves from the sector's first [`READ_LEN`] bytes; None when the sector is blank (CAL-5) or a
    /// curve the radio's bands use holds a 0 or 0xFF (an erased or unprogrammed byte: CAL-6's plausibility).
    pub fn parse(b: &[u8; READ_LEN]) -> Option<TxCal> {
        if b[BLANK_AT] == 0xFF {
            return None;
        }
        let mut c = TxCal { pa: [[[0; 16]; 4]; 3], hf: [[0; 16]; 3] };
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
        (!reached).then_some(c)
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
            200..=299 => p[R200][(((mhz - 200) / 5) as usize).min(15)],
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
    use crate::freq::mhz;

    /// A sector with every curve entry `base + level * 10 + index`, and the gauge given.
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
        b
    }

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

    #[test]
    fn lookup_as_stock() {
        let c = TxCal::parse(&sector(GAUGE_DEFAULT)).unwrap();
        // High is stock's level 0: 146.52 MHz is VHF entry (146-130)/3 = 5.
        assert_eq!(c.dac(mhz(146) + 52_000, 2), 100 + VHF as u8 * 4 + 5);
        assert_eq!(c.dac(mhz(146) + 52_000, 0), 100 + 80 + VHF as u8 * 4 + 5, "Low is stock's level 2");
        assert_eq!(c.dac(mhz(446), 1), 100 + 40 + 4, "UHF entry (446-400)/10 = 4, Mid");
        assert_eq!(c.dac(mhz(190), 2), 100 + VHF as u8 * 4 + 15, "175-200 MHz: entry 15");
        assert_eq!(c.dac(mhz(223) + 50_000, 2), 100 + R200 as u8 * 4 + 4);
        assert_eq!(c.dac(mhz(28) + 40_000, 2), 60 + 2, "10 m: HF entry (28-15)/5 = 2");
        assert_eq!(c.dac(mhz(70), 0), 100 + VHF as u8 * 4, "64-130 MHz: High VHF entry 0, any level");
        assert_eq!(c.dac(mhz(290), 2), 100 + R200 as u8 * 4 + 15, "280-300: the row's last entry, not the next row");
    }

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
