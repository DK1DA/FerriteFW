//! FM broadcast's shared pieces (chapter 20). Decision 352 (Nov 3, 2026) retired the FM screen (MENU 0 3 1): a
//! slot tuned into the broadcast band is the receiver (BCR-21 to BCR-29); 0 3 1 is the broadcast's settings,
//! which quick settings on a broadcast slot shows too (BCR-28).

use crate::App;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// The options (BCR-3): label, help and the setting's menu path (0 3 1 1-4, kept for the terminal and muscle
/// memory). Quick settings shows the same rows on a broadcast slot (BCR-28).
pub(crate) const OPTIONS: [(&str, crate::text::Txt, &[u8]); 4] = [
    ("Step", txt!("Tuning step: 50, 100 or 200 kHz."), &[3, 1, 1]),
    ("AGC", txt!("Automatic gain, or a fixed gain."), &[3, 1, 2]),
    ("Radio monitor", txt!("Slots keep watching; one opening interrupts."), &[3, 1, 3]),
    ("Sleep timer", txt!("End the broadcast after a time."), &[3, 1, 4]),
];

/// An option's value text, and whether it differs from its default.
pub(crate) fn option_text(app: &App, cx_dev: &ferrite_core::msg::Device, row: u8, b: &mut Buf<24>) -> bool {
    super::groups::wired(app, cx_dev, OPTIONS[row as usize % OPTIONS.len()].2, b).unwrap_or(false)
}

/// An option changed by the knob or ◀▶.
pub(crate) fn option_change(app: &mut App, row: u8, d: i32) {
    super::groups::change(app, OPTIONS[row as usize % OPTIONS.len()].2, d);
}

/// "100.5", "100.55" (10 kHz units).
pub(crate) fn fm_mhz<const N: usize>(f: u16, t: &mut Buf<N>) {
    let _ = uwrite!(t, "{}.{}", f / 100, f % 100 / 10);
    if !f.is_multiple_of(10) {
        let _ = uwrite!(t, "{}", f % 10);
    }
}
