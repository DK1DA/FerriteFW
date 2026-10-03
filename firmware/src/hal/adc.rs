//! ADC1, single software-triggered conversions (a bring-up tool, not the audio path). Mirrors hal_adc.
use super::gpio::{pin, Mode, GPIOA};
use super::regs::*;
use super::{modify, rd, time, wr};

pub const CH_PA0: u32 = 0;
pub const CH_PA1: u32 = 1;
pub const CH_PA2: u32 = 2;
pub const CH_TEMP: u32 = 16;
pub const CH_VREF: u32 = 17;

/// PA0-PA2 analog, sensor on, calibrated.
pub fn init() {
    for n in 0..3 {
        pin(GPIOA, n).config(Mode::Analog);
    }
    modify(CRM_CFG, CRM_CFG_ADCDIV_MASK, 0); // ADC clock = APB2 / 2 = 4 MHz on the reset tree
    modify(CRM_APB2EN, 0, CRM_APB2_ADC1);
    wr(ADC_CTRL1, 0);
    wr(ADC_SPT1, 0x00FF_FFFF); // 239.5 cycles everywhere: high-impedance dividers
    wr(ADC_SPT2, 0x3FFF_FFFF); // and the sensor's 17 us minimum
    wr(ADC_OSQ1, 0); // one conversion per sequence
    wr(ADC_CTRL2, ADC_CTRL2_ADCEN | ADC_CTRL2_ITSRVEN | ADC_CTRL2_OCTEN | ADC_CTRL2_OCTESEL_SW);
    time::delay_us(20); // power-up before calibration
    modify(ADC_CTRL2, 0, ADC_CTRL2_ADCALINIT);
    while rd(ADC_CTRL2) & ADC_CTRL2_ADCALINIT != 0 {}
    modify(ADC_CTRL2, 0, ADC_CTRL2_ADCAL);
    while rd(ADC_CTRL2) & ADC_CTRL2_ADCAL != 0 {}
}

/// One conversion, 12 bits, ~65 us; 0 if the ADC does not answer within 1 s.
pub fn read(ch: u32) -> u16 {
    wr(ADC_OSQ3, ch & 0x1F);
    modify(ADC_CTRL2, 0, ADC_CTRL2_OCSWTRG);
    let start = time::cycles();
    while rd(ADC_STS) & ADC_STS_OCCE == 0 {
        if time::cycles().wrapping_sub(start) > 8_000_000 {
            return 0;
        }
    }
    (rd(ADC_ODT) & 0xFFF) as u16
}

/// Mean of n conversions.
pub fn read_avg(ch: u32, n: u32) -> u16 {
    let sum: u32 = (0..n).map(|_| read(ch) as u32).sum();
    ((sum + n / 2) / n) as u16
}
