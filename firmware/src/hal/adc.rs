//! ADC1, single software-triggered conversions (a bring-up tool, not the audio path). Mirrors hal_adc.
use super::gpio::{pin, Mode, GPIOA};
use super::regs::*;
use super::{modify, rd, time, wr};

pub const CH_PA0: u32 = 0;
pub const CH_PA1: u32 = 1;
pub const CH_PA2: u32 = 2;
pub const CH_TEMP: u32 = 16;
pub const CH_VREF: u32 = 17;

/// ADC1 is set up (once, by whichever task needs it first: the Radio task's battery read, the DSP task's capture).
static READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// PA0-PA2 analog, sensor on, calibrated; once (later calls return at once). Interrupts are off for its
/// ~150 us, so two tasks can't set it up at the same time.
pub fn init() {
    cortex_m::interrupt::free(|_| {
        if !READY.swap(true, core::sync::atomic::Ordering::Relaxed) {
            init_once();
        }
    })
}

/// ADC clock = APB2 / 6 = 10 MHz, stock's (28 MHz max). At a slow bench clock (`super::clock`, APB2 at 20 MHz or
/// less) / 2 instead, so a 252-cycle conversion still fits the audio capture's 104 us sample period.
const ADCDIV: u32 = if super::clock::APB2_HZ <= 20_000_000 { 0b00 } else { 0b10 };
const ADC_HZ: u32 = super::clock::APB2_HZ / if ADCDIV == 0 { 2 } else { 6 };
const _: () = assert!(ADC_HZ <= 28_000_000 && 252 * 1_000_000 / ADC_HZ < 100);

fn init_once() {
    for n in 0..3 {
        pin(GPIOA, n).config(Mode::Analog);
    }
    modify(CRM_CFG, CRM_CFG_ADCDIV_MASK, ADCDIV << 14);
    modify(CRM_APB2EN, 0, CRM_APB2_ADC1);
    wr(ADC_CTRL1, 0);
    wr(ADC_SPT1, 0x00FF_FFFF); // 239.5 cycles everywhere: high-impedance dividers
    wr(ADC_SPT2, 0x3FFF_FFFF); // and the sensor's 17 us minimum
    wr(ADC_OSQ1, 0); // one conversion per sequence
    wr(ADC_CTRL2, ADC_CTRL2_ADCEN | ADC_CTRL2_OCTEN | ADC_CTRL2_OCTESEL_SW); // the sensors off: `sensors`
    time::delay_us(20); // power-up before calibration
    modify(ADC_CTRL2, 0, ADC_CTRL2_ADCALINIT);
    while rd(ADC_CTRL2) & ADC_CTRL2_ADCALINIT != 0 {}
    modify(ADC_CTRL2, 0, ADC_CTRL2_ADCAL);
    while rd(ADC_CTRL2) & ADC_CTRL2_ADCAL != 0 {}
}

/// One conversion, 12 bits, ~65 us; 0 if the ADC does not answer within 1 s. Through the preempted group, so it
/// works the same while the audio capture owns the ordinary group (`super::audio`).
pub fn read(ch: u32) -> u16 {
    super::audio::read_preempted(ch)
}

/// PWR-23: the temperature sensor and internal reference powered (ITSRVEN) only around their reads; their start-up
/// time (10 us at most, datasheet) is waited on the way on.
pub fn sensors(on: bool) {
    if on {
        modify(ADC_CTRL2, 0, ADC_CTRL2_ITSRVEN);
        time::delay_us(10);
    } else {
        modify(ADC_CTRL2, ADC_CTRL2_ITSRVEN, 0);
    }
}

/// Mean of n conversions.
pub fn read_avg(ch: u32, n: u32) -> u16 {
    let sum: u32 = (0..n).map(|_| read(ch) as u32).sum();
    ((sum + n / 2) / n) as u16
}
