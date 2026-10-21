//! The squelch thresholds from the factory calibration, exactly as stock programs them (SQL-1; stock V0.27
//! 0x0801E64C, called from its receive setup; `analysis/calibration.md` section 2, checked against the disassembly
//! Oct 12, 2026). Per squelch level 0-9: REG_78 (the RSSI open and close thresholds, less the band's RSSI offset)
//! and REG_4F (the noise threshold by band class, with the hysteresis). Stock writes no glitch thresholds here.
//! Also the factory crystal trim for receive (CAL-12, [`SqCal::rx_word`]), every build: the receive-side values the
//! Radio task takes from the sector. Read only: the calibration area is never written (SAFE).

use crate::freq::Freq10;

/// Bytes read from the start of the calibration sector (0x00F000): the tables run to 0xF21A.
pub const READ_LEN: usize = 0x220;
/// 0xF0E0 byte 0 = 0xFF: an erased sector, where stock keeps its compiled-in defaults.
const BLANK_AT: usize = 0xE0;

/// The squelch tables, about 110 bytes (the Radio task keeps one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SqCal {
    /// 0xF0C0 and 0xF0D0: the RSSI open and close thresholds per level, REG_67's scale (dBm = v / 2 - 160).
    open: [u8; 10],
    close: [u8; 10],
    /// RSSI offsets: 0xF0E0 (>= 400 MHz, 10 MHz steps; byte 0 doubles as the programmed flag), 0xF0F0 (130-180,
    /// 5 MHz), 0xF100 and the 0xF110 after it (200-300 MHz, 5 MHz, reading on into 0xF110 for 280-299 as stock
    /// does; 0xF110 alone serves 350-400), 0xF170 (15-100, 5 MHz).
    uhf: [u8; 13],
    vhf: [u8; 10],
    mid: [u8; 32],
    hf: [u8; 17],
    /// 0xF210 bytes 5-8 (noise threshold by class: >= 400 MHz, everything else, 200-300, 300-400) and byte 9
    /// (its hysteresis).
    noise: [u8; 4],
    hyst: u8,
    /// CAL-12: the factory crystal trim k (0xF210 bytes 12-13 under the 0xA0 marker, within ±20 ppm), 0 for none.
    trim: i16,
}

impl SqCal {
    /// Stock's compiled-in tables, used when the sector is blank (V0.27 .data at 0x200000E4 and 0x20000BE8).
    pub const DEFAULT: SqCal = SqCal {
        open: [0x00, 0x59, 0x5B, 0x5D, 0x5F, 0x61, 0x63, 0x66, 0x69, 0x6B],
        close: [0x00, 0x57, 0x59, 0x5B, 0x5D, 0x5F, 0x61, 0x63, 0x66, 0x69],
        uhf: [4; 13],
        vhf: [4, 4, 4, 4, 4, 4, 4, 4, 2, 2],
        mid: [0; 32],
        hf: [0; 17],
        noise: [0x2A, 0x2B, 0x38, 0x32],
        hyst: 0x02,
        trim: 0,
    };

    /// The tables from the sector's first [`READ_LEN`] bytes; stock's defaults when it is blank.
    pub fn parse(b: &[u8; READ_LEN]) -> SqCal {
        if b[BLANK_AT] == 0xFF {
            return SqCal::DEFAULT;
        }
        let mut c = SqCal::DEFAULT;
        c.open.copy_from_slice(&b[0xC0..0xCA]);
        c.close.copy_from_slice(&b[0xD0..0xDA]);
        c.uhf.copy_from_slice(&b[0xE0..0xED]);
        c.vhf.copy_from_slice(&b[0xF0..0xFA]);
        c.mid.copy_from_slice(&b[0x100..0x120]);
        c.hf.copy_from_slice(&b[0x170..0x181]);
        c.noise.copy_from_slice(&b[0x215..0x219]);
        c.hyst = b[0x219];
        c.trim = crate::txcal::valid_trim(b);
        c
    }

    /// CAL-12: the REG_38/39 word for receiving on `f`, the factory crystal trim applied as stock applies it (both
    /// chips, every receive set-up and the scope's points). The squelch tables and every check take `f` itself.
    pub fn rx_word(&self, f: Freq10) -> Freq10 {
        crate::txcal::trimmed(f, self.trim)
    }

    /// The factory trim taken (0 for none): a change retunes the chips.
    pub fn trim(&self) -> i16 {
        self.trim
    }

    /// REG_78 and REG_4F for receiving `f` at squelch `level` (0-9), as stock computes them:
    ///
    /// | Range (MHz) | RSSI offset | Noise class |
    /// | --- | --- | --- |
    /// | 400 up | 0xF0E0[(MHz-400)/10], at most 12 | 0 |
    /// | 300-400 | 0xF110[(MHz-350)/5] | 3 |
    /// | 130-180 | 0xF0F0[(MHz-130)/5], at most 9 | 1 |
    /// | 200-300 | 0xF100[(MHz-200)/5] (280-299 read on into 0xF110) | 2 |
    /// | 108-130 | none: open = the close threshold, close = it less 10 | 1 |
    /// | 15-108 | 0xF170[(MHz-15)/5], at most 16 (100-108: the 95 MHz entry) | 1 |
    /// An erased entry (0xFF) takes the nearest programmed one below it ([`nearest`]).
    /// | 180-200 | 0xF0F0[9], the 175 MHz entry | 1 |
    /// | below 15 | none: open threshold 0, always open (not receivable) | 1 |
    ///
    /// The offset comes off both thresholds only while it is below the open one (level 0's open 0 never). Stock
    /// quirks not copied (SQL-15, decision 222): the close byte left over from a register (Ferrite writes 0, which
    /// can't matter with the open threshold at 0 or at the offset); 300-345 MHz indexing RAM past the 0xF110 table
    /// (Ferrite uses its entry 0, the 350 MHz point); and 100-108 and 180-200 MHz, where stock has no table and
    /// leaves the RSSI threshold at 0 whatever the level (Ferrite takes the nearest calibrated point, so the levels
    /// work there too), and an erased entry (stock takes 0xFF as the offset, so none applies). At 26.020-26.110 MHz (CB) REG_4F is stock's fixed 0x2322.
    pub fn regs(&self, f: Freq10, level: u8) -> (u16, u16) {
        let l = level.min(9) as usize;
        let mhz = (f / 100_000) as i32;
        let (open, close) = (self.open[l], self.close[l]);
        let with_off = |t: &[u8], i: i32| {
            let off = nearest(t, i as usize);
            if open > off { (open - off, close.wrapping_sub(off)) } else { (open, 0) }
        };
        let ((o, c), class) = if f >= 40_000_000 {
            (with_off(&self.uhf, ((mhz - 400) / 10).clamp(0, 12)), 0)
        } else if f >= 30_000_000 {
            (with_off(&self.mid[16..], ((mhz - 350) / 5).clamp(0, 9)), 3)
        } else if (13_000_000..20_000_000).contains(&f) {
            (with_off(&self.vhf, ((mhz - 130) / 5).min(9)), 1)
        } else if f >= 20_000_000 {
            (with_off(&self.mid, ((mhz - 200) / 5).clamp(0, 19)), 2)
        } else if (10_800_000..13_600_000).contains(&f) {
            ((close, close.wrapping_sub(10)), 1)
        } else if f >= 1_500_000 {
            (with_off(&self.hf, ((mhz - 15) / 5).min(16)), 1)
        } else {
            ((0, 0), 1)
        };
        let n = (self.noise[class] & 0x7F) as u16;
        let r4f = if (2_602_000..2_611_000).contains(&f) { 0x2322 } else { n << 8 | (n as u8).wrapping_sub(self.hyst) as u16 };
        ((o as u16) << 8 | c as u16, r4f)
    }
}

/// SQL-15 (decision 222): a table's offset at `i`, or, where the factory left it erased (0xFF), the nearest
/// programmed entry below it (0 if none). Stock takes 0xFF as it is, so the offset never applies there and the close
/// threshold is a leftover: both radios' 265-279 MHz (0xF100 entries 13-15) and 95-108 MHz (0xF170 entry 16).
fn nearest(t: &[u8], i: usize) -> u8 {
    t[..=i].iter().rev().copied().find(|&v| v != 0xFF).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;

    /// The black radio's calibration sector (both radios' are identical, Oct 12, 2026), the tables stock reads.
    fn black() -> SqCal {
        let mut b = [0xFFu8; READ_LEN];
        let at = |b: &mut [u8; READ_LEN], o: usize, v: &[u8]| b[o..o + v.len()].copy_from_slice(v);
        at(&mut b, 0xC0, &[0x00, 0x59, 0x5B, 0x5D, 0x5F, 0x61, 0x63, 0x65, 0x67, 0x69]);
        at(&mut b, 0xD0, &[0x00, 0x55, 0x57, 0x59, 0x5B, 0x5D, 0x5F, 0x61, 0x63, 0x65]);
        at(&mut b, 0xE0, &[6; 13]);
        at(&mut b, 0xF0, &[9, 9, 8, 7, 6, 5, 2, 0, 0, 0]);
        at(&mut b, 0x100, &[0x0D; 13]);
        at(&mut b, 0x110, &[8, 7, 7, 8, 8, 8, 8, 8, 8, 8]);
        at(&mut b, 0x170, &[0; 16]);
        at(&mut b, 0x210, &[0x37, 0x46, 0x3E, 0x1F, 0x1F, 0x28, 0x2A, 0x2A, 0x2A, 0x01, 0x08]);
        SqCal::parse(&b)
    }

    /// SQL-1, SQL-15, SQL-16.1: stock's thresholds for this radio's calibration. 443.475 at level 4: open 0x5F - 6,
    /// close 0x5B - 6 (about -115.5 and -117.5 dBm), noise 40 with 1 of hysteresis; at the power-on 0x4846 UHF
    /// stayed open.
    #[test]
    fn stock_thresholds() {
        let c = black();
        let at = |f, l| c.regs(f, l);
        assert_eq!(at(mhz(443) + 47_500, 4), (0x5955, 0x2827));
        assert_eq!(at(mhz(446), 4), (0x5955, 0x2827));
        assert_eq!(at(mhz(446), 9), (0x635F, 0x2827));
        assert_eq!(at(mhz(446), 1), (0x534F, 0x2827));
        assert_eq!(at(mhz(146) + 52_000, 4), (0x5854, 0x2A29), "130-180: 0xF0F0[3] = 7");
        assert_eq!(at(mhz(162) + 55_000, 4), (0x5D59, 0x2A29), "0xF0F0[6] = 2");
        assert_eq!(at(mhz(118) + 95_000, 4), (0x5B51, 0x2A29), "airband: open = close, close - 10");
        assert_eq!(at(mhz(50), 4), (0x5F5B, 0x2A29), "15-100: offset 0");
        assert_eq!(at(mhz(222), 4), (0x524E, 0x2A29), "200-300: 0xF100[4] = 13");
        assert_eq!(at(mhz(290), 4), (0x5854, 0x2A29), "280-299 read on into 0xF110");
        assert_eq!(at(mhz(355), 4), (0x5854, 0x2A29), "300-400: 0xF110[1]");
        assert_eq!(at(mhz(190), 4), (0x5F5B, 0x2A29), "180-200: the 175 MHz entry (0), not stock's always open");
        assert_eq!(at(mhz(104), 4), (0x5F5B, 0x2A29), "100-108: the nearest programmed entry, not always open");
        assert_eq!(at(mhz(97), 4), (0x5F5B, 0x2A29), "95-100: 0xF170[16] is 0xFF: the 90 MHz entry");
        assert_eq!(at(mhz(270), 4), (0x524E, 0x2A29), "265-279: 0xF100[13] is 0xFF: the 260 MHz entry, 13");
        assert_eq!(at(mhz(26) + 6_500, 4), (0x5F5B, 0x2322), "CB: stock's fixed noise value");
        for f in [mhz(146), mhz(446), mhz(118)] {
            assert_eq!(at(f, 0).0 >> 8, 0, "level 0: open threshold 0, always open");
        }
    }

    /// SQL-16.4: a blank sector keeps stock's compiled-in tables.
    #[test]
    fn blank_sector_uses_stocks_defaults() {
        let c = SqCal::parse(&[0xFF; READ_LEN]);
        assert_eq!(c, SqCal::DEFAULT);
        assert_eq!(c.regs(mhz(446), 4), (0x5B59, 0x2A28), "open 0x5F - 4, close 0x5D - 4");
        assert_eq!(c.regs(mhz(190), 4), (0x5D5B, 0x2B29), "180-200: the default 175 MHz offset, 2");
        assert_eq!(c.rx_word(mhz(446)), mhz(446), "no trim");
    }

    /// CAL-12 (decision 222): the factory trim on receive, within ±20 ppm, under the marker on a programmed sector;
    /// none on both radios (marker 0xFF), none over 20 ppm (TX locks then, CAL-6).
    #[test]
    fn receive_trim() {
        assert_eq!(black().rx_word(mhz(146) + 52_000), mhz(146) + 52_000, "both radios: no marker");
        let mut b = [0x00u8; READ_LEN];
        b[0x21B..0x21E].copy_from_slice(&[0xA0, 0x64, 0x00]); // k = +100
        let c = SqCal::parse(&b);
        assert_eq!((c.trim(), c.rx_word(mhz(435) + 50_000)), (100, mhz(435) + 50_000 + 100));
        b[0x21C..0x21E].copy_from_slice(&2000i16.to_le_bytes());
        assert_eq!(SqCal::parse(&b).trim(), 0, "46 ppm: invalid, untrimmed");
        b[0xE0] = 0xFF;
        b[0x21C..0x21E].copy_from_slice(&100i16.to_le_bytes());
        assert_eq!(SqCal::parse(&b).trim(), 0, "a blank sector has none, as stock");
    }
}
