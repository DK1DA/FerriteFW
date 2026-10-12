//! The audio converters, each paced by a timer and fed by DMA into a ping-pong buffer (roadmap 2f, 5b, 5c):
//!
//! - **Capture:** ADC1 channel 2 (PA2, the BK4829s' EARO), triggered by TMR2 channel 2 at
//!   [`ferrite_core::audio::ADC_HZ`], DMA1 channel 1 into [`ADC_BUF`]; each half done raises DMA1 channel 1's
//!   interrupt, which signals [`ADC_HALF`]. Stock's path, at 48 kHz instead of 9.6 (`analysis/audio_path.md`:
//!   TMR2 CH2 trigger, DMA1 ch1).
//! - **Playback:** DAC1 (PA4, output buffer off as stock), triggered by TMR6's update at
//!   [`ferrite_core::audio::DAC_HZ`], DMA2 channel 3 from [`DAC_BUF`]; each half played signals [`DAC_HALF`] for
//!   the DSP task to refill. PC12 high routes DAC1 into the BK4829's microphone input while it plays, low after
//!   (stock: set at 0x080064EA, cleared at 0x08006578).
//!
//! The software reads (battery PA1, mic level PA0, the sensors: [`super::adc::read`]) go through ADC1's preempted
//! group, so they run between captures without stopping them. All of it is bench-confirm: nothing here has run
//! on a radio yet.

use super::gpio::{pin, Mode, Pin, GPIOA, GPIOC};
use super::regs::*;
use super::{clock, irq, modify, rd, wr};
use core::sync::atomic::{AtomicU16, AtomicU8, Ordering::Relaxed};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::signal::Signal;
use ferrite_core::audio::{ADC_HZ, BLOCK_DAC, BLOCK_IN, DAC_HZ};

const CRM_AHBEN: u32 = 0x4002_1014;
const CRM_AHB_DMA1: u32 = 1 << 0;
const CRM_AHB_DMA2: u32 = 1 << 1;
const CRM_APB1_TMR2: u32 = 1 << 0;

const DMA1: u32 = 0x4002_0000;
const DMA2: u32 = 0x4002_0400;
const DMA_STS: u32 = 0x00;
const DMA_CLR: u32 = 0x04;
/// Channel n's registers (1-based): control, count, peripheral and memory address.
const fn ch(dma: u32, n: u32) -> u32 {
    dma + 0x08 + 20 * (n - 1)
}
const CH_CTRL: u32 = 0x00;
const CH_DTCNT: u32 = 0x04;
const CH_PADDR: u32 = 0x08;
const CH_MADDR: u32 = 0x0C;
const DMA_EN: u32 = 1 << 0;
const DMA_FDTIEN: u32 = 1 << 1; // transfer complete
const DMA_HDTIEN: u32 = 1 << 2; // half transfer
const DMA_DTD_M2P: u32 = 1 << 4; // memory to peripheral
const DMA_LM: u32 = 1 << 5; // circular
const DMA_MINCM: u32 = 1 << 7;
const DMA_PWIDTH_16: u32 = 1 << 8;
const DMA_MWIDTH_16: u32 = 1 << 10;
const DMA_CHPL_HIGH: u32 = 2 << 12;
/// A channel's flags in DMA_STS / DMA_CLR: global, complete, half, error.
const fn flags(n: u32) -> u32 {
    0xF << (4 * (n - 1))
}
const fn half_flag(n: u32) -> u32 {
    1 << (4 * (n - 1) + 2)
}

const TMR2: u32 = 0x4000_0000;
const TMR_C2DT: u32 = 0x38;
const TMR_CM1_C2_PWM1: u32 = 6 << 12;
const TMR_CCTRL_C2EN: u32 = 1 << 4;
const TMR_CTRL1_CNTEN: u32 = 1 << 0;

const ADC_PSQ: u32 = ADC1 + 0x38; // preempted sequence: with one conversion, the channel is in bits 15-19
const ADC_PDT1: u32 = ADC1 + 0x3C;
const ADC_CTRL2_DMAEN: u32 = 1 << 8;
const ADC_CTRL2_OCTESEL_MASK: u32 = 7 << 17;
const ADC_CTRL2_OCTESEL_TMR2CH2: u32 = 3 << 17;
const ADC_CTRL2_PCTESEL_SW: u32 = 7 << 12;
const ADC_CTRL2_PCTEN: u32 = 1 << 15;
const ADC_CTRL2_PCSWTRG: u32 = 1 << 21;
const ADC_STS_PCCE: u32 = 1 << 2;
/// Channel 2's sample time code in SPT2 (bits 6-8): 28.5 cycles, stock's.
const SPT2_CH2_MASK: u32 = 7 << 6;
const SPT2_CH2_28: u32 = 3 << 6;

const DAC_CTRL_D1OBDIS: u32 = 1 << 1;
const DAC_CTRL_D1DMAEN: u32 = 1 << 12;

/// DMA1 channel 1 (ADC1) and DMA2 channel 3 (DAC1) interrupt lines.
pub const IRQ_ADC_DMA: u16 = 11;
pub const IRQ_DAC_DMA: u16 = 58;
/// Above the DSP executor (6): a half is announced at once.
const PRIO: u8 = 5;

/// PC12: DAC1 into the transmitting BK4829's microphone input (stock's APRS switch; in `board::SAFE_LOW`).
pub const MIC_INJECT: Pin = pin(GPIOC, 12);

/// The capture buffer: two halves of [`BLOCK_IN`] samples, written by DMA (an AtomicU16 is a u16 in memory).
pub static ADC_BUF: [AtomicU16; 2 * BLOCK_IN] = [const { AtomicU16::new(0) }; 2 * BLOCK_IN];
/// The playback buffer: two halves of [`BLOCK_DAC`] DAC codes, read by DMA.
pub static DAC_BUF: [AtomicU16; 2 * BLOCK_DAC] = [const { AtomicU16::new(0) }; 2 * BLOCK_DAC];
/// Which half is ready (0 first, 1 second): captured, or played and free to refill.
pub static ADC_HALF: Signal<M, u8> = Signal::new();
pub static DAC_HALF: Signal<M, u8> = Signal::new();
/// Halves the DSP task didn't take in time (it was busy): a capture overrun, counted for the bench.
pub static ADC_LATE: AtomicU8 = AtomicU8::new(0);

/// A timer's reload for `hz` from its clock (TMR2 and TMR6 are on APB1; with APB1 /2 they run at twice it, 120 MHz;
/// stock's 9599.2 Hz is 120 MHz / 12501). Fits 16 bits down to 1.9 kHz.
fn reload(hz: u32) -> u32 {
    (clock::TMR_HZ + hz / 2) / hz
}

/// Capture on: PA2 analog, TMR2 at the ADC rate, ADC1 on its trigger, DMA1 channel 1 circular. ADC1 must have
/// been through [`super::adc::init`].
pub fn capture_start() {
    pin(GPIOA, 2).config(Mode::Analog);
    modify(CRM_AHBEN, 0, CRM_AHB_DMA1);
    modify(CRM_APB1EN, 0, CRM_APB1_TMR2);
    let c = ch(DMA1, 1);
    wr(c + CH_CTRL, 0);
    wr(DMA1 + DMA_CLR, flags(1));
    wr(c + CH_PADDR, ADC_ODT);
    wr(c + CH_MADDR, ADC_BUF.as_ptr() as u32);
    wr(c + CH_DTCNT, ADC_BUF.len() as u32);
    wr(c + CH_CTRL, DMA_MINCM | DMA_LM | DMA_PWIDTH_16 | DMA_MWIDTH_16 | DMA_CHPL_HIGH | DMA_HDTIEN | DMA_FDTIEN);
    modify(c + CH_CTRL, 0, DMA_EN);
    // ADC1: channel 2 alone in the ordinary group, short sample time, each conversion on TMR2 CH2, to DMA.
    modify(ADC_SPT2, SPT2_CH2_MASK, SPT2_CH2_28);
    wr(ADC_OSQ1, 0);
    wr(ADC_OSQ3, 2);
    modify(ADC_CTRL2, ADC_CTRL2_OCTESEL_MASK, ADC_CTRL2_OCTESEL_TMR2CH2 | ADC_CTRL2_OCTEN | ADC_CTRL2_DMAEN);
    // TMR2 CH2: PWM at the ADC rate, its compare edge the trigger.
    let pr = reload(ADC_HZ);
    wr(TMR2 + TMR_CTRL1, 0);
    wr(TMR2 + TMR_DIV, 0);
    wr(TMR2 + TMR_PR, pr - 1);
    wr(TMR2 + TMR_C2DT, pr / 2);
    wr(TMR2 + TMR_CM1, TMR_CM1_C2_PWM1);
    wr(TMR2 + TMR_CCTRL, TMR_CCTRL_C2EN);
    wr(TMR2 + TMR_SWEVT, 1);
    irq::enable(IRQ_ADC_DMA, PRIO);
    wr(TMR2 + TMR_CTRL1, TMR_CTRL1_CNTEN);
}

/// Capture off: the timer stopped, ADC1 back to software triggers, DMA off.
pub fn capture_stop() {
    wr(TMR2 + TMR_CTRL1, 0);
    irq::disable(IRQ_ADC_DMA);
    modify(ch(DMA1, 1) + CH_CTRL, DMA_EN, 0);
    modify(ADC_CTRL2, ADC_CTRL2_OCTESEL_MASK | ADC_CTRL2_DMAEN, super::regs::ADC_CTRL2_OCTESEL_SW);
}

/// DMA1 channel 1: a capture half is full.
pub fn adc_isr() {
    let sts = rd(DMA1 + DMA_STS);
    wr(DMA1 + DMA_CLR, flags(1));
    if ADC_HALF.signaled() {
        ADC_LATE.fetch_add(1, Relaxed);
    }
    ADC_HALF.signal(if sts & half_flag(1) != 0 { 0 } else { 1 });
}

/// Playback on: PA4 analog, DAC1 on TMR6's update at the DAC rate, DMA2 channel 3 circular from [`DAC_BUF`]
/// (the caller has filled both halves), PC12 high.
pub fn play_start() {
    pin(GPIOA, 4).config(Mode::Analog);
    modify(CRM_AHBEN, 0, CRM_AHB_DMA2);
    modify(CRM_APB1EN, 0, CRM_APB1_TMR6 | CRM_APB1_DAC);
    modify(CRM_APB1RST, CRM_APB1_DAC, 0);
    let c = ch(DMA2, 3);
    wr(c + CH_CTRL, 0);
    wr(DMA2 + DMA_CLR, flags(3));
    wr(c + CH_PADDR, DAC_DHR12R1);
    wr(c + CH_MADDR, DAC_BUF.as_ptr() as u32);
    wr(c + CH_DTCNT, DAC_BUF.len() as u32);
    wr(c + CH_CTRL, DMA_DTD_M2P | DMA_MINCM | DMA_LM | DMA_PWIDTH_16 | DMA_MWIDTH_16 | DMA_CHPL_HIGH | DMA_HDTIEN | DMA_FDTIEN);
    modify(c + CH_CTRL, 0, DMA_EN);
    // DAC1: channel 2 (the PA drive, `hal::pa`) left exactly as it is.
    modify(DAC_CTRL, !DAC_CTRL_CH2_MASK, DAC_CTRL_EN1 | DAC_CTRL_TEN1 | DAC_CTRL_TSEL1_TMR6 | DAC_CTRL_D1OBDIS | DAC_CTRL_D1DMAEN);
    wr(TMR6 + TMR_CTRL1, 0);
    wr(TMR6 + TMR_DIV, 0);
    wr(TMR6 + TMR_PR, reload(DAC_HZ) - 1);
    wr(TMR6 + TMR_CTRL2, TMR_CTRL2_PTOS_OVF);
    wr(TMR6 + TMR_SWEVT, 1);
    MIC_INJECT.output(true, Mode::OutPp);
    irq::enable(IRQ_DAC_DMA, PRIO);
    wr(TMR6 + TMR_CTRL1, TMR_CTRL1_CNTEN);
}

/// Playback off: PC12 low first, then the timer, DMA and DAC1's channel (mid-scale left on PA4).
pub fn play_stop() {
    MIC_INJECT.write(false);
    wr(TMR6 + TMR_CTRL1, 0);
    irq::disable(IRQ_DAC_DMA);
    modify(ch(DMA2, 3) + CH_CTRL, DMA_EN, 0);
    modify(DAC_CTRL, DAC_CTRL_D1DMAEN | DAC_CTRL_TEN1, 0);
}

/// DMA2 channel 3: a playback half has gone out and may be refilled.
pub fn dac_isr() {
    let sts = rd(DMA2 + DMA_STS);
    wr(DMA2 + DMA_CLR, flags(3));
    DAC_HALF.signal(if sts & half_flag(3) != 0 { 0 } else { 1 });
}

/// One conversion through the preempted group (software read while the capture runs): 12 bits, 0 if the ADC
/// doesn't answer within about 1 s.
pub fn read_preempted(chan: u32) -> u16 {
    wr(ADC_PSQ, (chan & 0x1F) << 15);
    modify(ADC_CTRL2, 7 << 12, ADC_CTRL2_PCTESEL_SW | ADC_CTRL2_PCTEN);
    modify(ADC_STS, ADC_STS_PCCE, 0);
    modify(ADC_CTRL2, 0, ADC_CTRL2_PCSWTRG);
    let start = super::time::cycles();
    while rd(ADC_STS) & ADC_STS_PCCE == 0 {
        if super::time::cycles().wrapping_sub(start) > super::clock::SYSCLK_HZ {
            return 0;
        }
    }
    modify(ADC_STS, ADC_STS_PCCE, 0);
    (rd(ADC_PDT1) & 0xFFF) as u16
}
