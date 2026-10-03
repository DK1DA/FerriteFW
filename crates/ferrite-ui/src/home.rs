//! The home screen, first cut: status bar, the frequency, a signal meter, the hint bar. The look review's layout
//! (three slots, `docs/firmware-design.md`, "Screens") replaces it; this one proves the path from the Radio
//! task's [`Live`] snapshot to the panel.

use crate::assets::*;
use crate::gfx::{Align, Gfx, BG, BLUE, GREEN, LINE, SUB, TEXT};
use crate::panel::Panel;
use crate::text::Buf;
use crate::v;
use ferrite_core::freq;
use ferrite_core::msg::Live;

/// S-meter: -127 dBm (S0) to -53 dBm (S9 + 20) across 224 px.
fn meter_px(dbm: i16) -> i32 {
    ((dbm as i32 + 127).clamp(0, 74) * 224) / 74
}

/// Whole screen.
pub fn draw(lcd: &mut impl Panel, live: &Live) {
    lcd.fill(0, 0, 240, 320, BG);
    // Status bar (its icons come with the look review's layout)
    lcd.text4(8, 15, b"Ferrite", &UI_12B, SUB, BG);
    lcd.fill(0, 20, 240, 1, LINE);
    // Hint bar
    lcd.fill(0, 300, 240, 1, LINE);
    let x = lcd.text4(8, 314, b"Menu", &UI_12B, TEXT, BG);
    lcd.text4(8 + x + 4, 314, b"settings", &UI_12, SUB, BG);
    update(lcd, live);
}

/// Only what changes with the receiver: frequency, meter, squelch.
pub fn update(lcd: &mut impl Panel, live: &Live) {
    let mut b = [0u8; 16];
    let n = freq::format(live.freq, &mut b);
    // MHz part big, the last two digits small: "146.520" + "00".
    let split = n.saturating_sub(3);
    lcd.fill(0, 60, 240, 60, BG);
    let w = lcd.text4_aligned(200, 110, Align::Right, &b[..split], &DIG_46, TEXT, BG);
    let _ = w;
    lcd.text4(204, 110, &b[split + 1..n], &DIG_26, SUB, BG);
    lcd.text4_aligned(232, 132, Align::Right, b"MHz", &UI_12, SUB, BG);

    let px = meter_px(live.rssi);
    lcd.fill(8, 160, px, 8, if live.squelch_open { GREEN } else { BLUE });
    lcd.fill(8 + px, 160, 224 - px, 8, LINE);
    let mut t = Buf::<16>::new();
    lcd.fill(8, 172, 224, 20, BG);
    lcd.text4(8, 186, v!(t, "{} dBm", live.rssi), &UI_12, SUB, BG);
    if live.squelch_open {
        lcd.text4_aligned(232, 186, Align::Right, b"RX", &UI_12B, GREEN, BG);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::FrameBuffer;

    #[test]
    fn meter() {
        assert_eq!(meter_px(-140), 0);
        assert_eq!(meter_px(-53), 224);
        assert_eq!(meter_px(0), 224);
    }

    #[test]
    fn draws() {
        let mut fb = FrameBuffer::new();
        draw(&mut fb, &Live { freq: freq::mhz(146) + 52_000, rssi: -90, squelch_open: true, chips: 3, ..Default::default() });
        // Something was drawn in the frequency area and the meter is green up to -90 dBm.
        assert!((60..120).any(|y| (0..240).any(|x| fb.at(x, y) != BG)));
        assert_eq!(fb.at(8 + meter_px(-90) - 1, 164), GREEN);
        assert_eq!(fb.at(8 + meter_px(-90), 164), LINE);
    }
}
