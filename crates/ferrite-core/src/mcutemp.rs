//! The MCU's die temperature for the Status page (chapter 8): the AT32F403A's internal sensor (ADC channel 16)
//! scaled by VDDA from the internal reference (channel 17), with the datasheet's typical values
//! (DS_AT32F403A V2.04, Table 55: V25 1.28 V, Avg_Slope -4.26 mV/°C; Temperature = (V25 - VTS) / Avg_Slope + 25).
//! It reads the die, not the PA. The datasheet puts the line's offset up to 50 °C apart from chip to chip, so the
//! reading is uncalibrated until 0 9 7 4's one-point offset is set (SBAR-21); informational only.
//!
//! Fixed Oct 13, 2026: the slope was taken as +4.1 mV/°C (STM32F1's sensor, which falls with temperature, as
//! hwtest's `fw/src/core/analog.c` still does). The AT32's slope is negative: its sensor's voltage rises with
//! temperature, so the old formula read low (about 12 °C in a warm room) and fell as the radio warmed.

/// The internal reference's typical voltage (mV).
pub const VREFINT_MV: u32 = 1200;
/// The sensor's typical voltage at 25 °C (mV).
pub const V25_MV: i32 = 1280;
/// Its typical slope, Avg_Slope (µV per °C): negative, the voltage rises with temperature.
pub const SLOPE_UV: i32 = -4260;

/// Tenths of a degree C from 12-bit readings of the sensor and the reference; None if either reads 0.
pub fn temp_c10(raw_temp: u16, raw_vref: u16) -> Option<i16> {
    if raw_temp == 0 || raw_vref == 0 {
        return None;
    }
    let vdda_mv = VREFINT_MV * 4095 / raw_vref as u32;
    let uv = (raw_temp as u64 * vdda_mv as u64 * 1000 / 4095) as i32;
    let c10 = 250 + (V25_MV * 1000 - uv) * 10 / SLOPE_UV;
    Some(c10.clamp(-400, 1250) as i16)
}

/// A slow average of tenths (one eighth of each new reading), so one noisy conversion doesn't move the row.
pub fn smooth(prev: Option<i16>, new: i16) -> i16 {
    match prev {
        None => new,
        Some(p) => (p as i32 + (new as i32 - p as i32) / 8) as i16,
    }
}

/// Whole degrees, rounded, for the Status page.
pub fn whole(c10: i16) -> i8 {
    ((c10 as i32 + if c10 >= 0 { 5 } else { -5 }) / 10).clamp(-128, 127) as i8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VDDA 3.3 V: the reference reads 1200 / 3300 x 4095 = 1489.
    const VREF_3V3: u16 = 1489;

    fn raw_at(mv: u32) -> u16 {
        (mv * 4095 / 3300) as u16
    }

    /// SBAR-20: Table 55's formula: 1.28 V is 25 °C, 4.26 mV more per degree warmer (Avg_Slope is negative).
    #[test]
    fn datasheet_points() {
        assert_eq!(temp_c10(raw_at(1280), VREF_3V3).map(whole), Some(25));
        assert_eq!(temp_c10(raw_at(1280 + 85), VREF_3V3).map(whole), Some(45), "warmer: higher");
        assert_eq!(temp_c10(raw_at(1280 - 43), VREF_3V3).map(whole), Some(15), "cooler: lower");
        // The bug's reading on the radio: a sensor 12 °C-worth above V25 read as 12 °C below.
        let warm = temp_c10(raw_at(1280 + 51), VREF_3V3).map(whole).unwrap();
        assert!((36..=37).contains(&warm), "{warm}");
        assert_eq!(temp_c10(0, VREF_3V3), None);
        assert_eq!(temp_c10(1000, 0), None);
    }

    #[test]
    fn smoothing_and_rounding() {
        assert_eq!(smooth(None, 300), 300);
        assert_eq!(smooth(Some(300), 380), 310);
        assert_eq!(whole(345), 35);
        assert_eq!(whole(-45), -5);
    }
}
