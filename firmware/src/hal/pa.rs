//! The PA drive (APC) on DAC channel 2, PA5: only in the `tx` build, only from the Radio task's path plans
//! (`Act::PaRamp`, `Act::PaZero`). Off, PA5 is a GPIO driven low and channel 2 disabled, the safe state's own
//! levels; on, PA5 is analog and channel 2 holds the calibrated 8-bit value (stock's DHR8R2 write, TX-23).
use super::gpio::{pin, Mode, GPIOA};
use super::regs::*;
use super::{modify, wr};

/// The PA drive to `v` (0-255 = 0 V to Vref). Channel 1 (the audio tone) is left as it is. The `tx-dry` build,
/// and a `tx` build with Dry TX on (TX-48, [`crate::chan::DRY_TX`]), hold it at 0 whatever `v` is: the one choke
/// point, as no other code writes DHR8R2 or makes PA5 analog.
pub fn set(v: u8) {
    let dry = cfg!(feature = "tx-dry") || crate::chan::DRY_TX.load(core::sync::atomic::Ordering::SeqCst);
    let v = ferrite_core::txseq::pa_drive(v, dry);
    if v == 0 {
        return zero();
    }
    modify(CRM_APB1EN, 0, CRM_APB1_DAC);
    modify(CRM_APB1RST, CRM_APB1_DAC, 0);
    wr(DAC_DHR8R2, v as u32);
    modify(DAC_CTRL, 0, DAC_CTRL_EN2);
    pin(GPIOA, 5).config(Mode::Analog);
}

/// PA drive 0 V: the data 0, channel 2 off, PA5 back to a GPIO driven low.
pub fn zero() {
    wr(DAC_DHR8R2, 0);
    modify(DAC_CTRL, DAC_CTRL_EN2, 0);
    pin(GPIOA, 5).output(false, Mode::OutPp);
}
