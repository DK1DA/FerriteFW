//! The screen's backlight, dimmed by PWM on PC6 (manual DISP-16, DISP-23). The keypad light (PB3) stays a plain
//! on/off pin (DISP-22).
//!
//! nicFW dims PC6 with TMR3 channel 1 on its full remap (IOMUX_REMAP TMR3_MUX = 11). Ferrite's TMR3 is the Embassy
//! time driver (its channel 1 is the alarm), so the PWM comes from TMR8 channel 1 instead, PC6's own (unremapped)
//! timer function on the AT32F403A (datasheet pin table: PC6 = TMR8_CH1; TMR3_CH1 only with the remap). No IOMUX
//! remap is written. Everything that knows this is in this file, to check on the radio with `mem32`:
//!
//! - TMR8 at 0x4001_3400: CTRL1 +0x00 = 0x81 (counting, period buffered), CM1 +0x18 = 0x68 (PWM mode A, C1DT
//!   buffered), CCTRL +0x20 = 0x01 (channel 1 out, active high), DIV +0x28 = 0, PR +0x2C = 4799 (25 kHz from the
//!   120 MHz timer clock: above flicker and hearing), C1DT +0x34 = the duty in counts (0 dark, 4800 full on),
//!   BRK +0x44 bit 15 (outputs on: an advanced timer's).
//! - PC6 as a peripheral output: GPIOC CFGLR (0x4001_1000) bits 24-27 = 0xB.
use super::clock::TMR_HZ;
use super::gpio::{pin, Mode, GPIOC};
use super::regs::*;
use super::{modify, wr};

/// The PWM frequency.
pub const PWM_HZ: u32 = 25_000;
/// Counts per period (PR + 1).
pub const PERIOD: u32 = TMR_HZ / PWM_HZ;

const CTRL1_CNTEN: u32 = 1 << 0;
const CTRL1_PRBEN: u32 = 1 << 7;
/// CM1: channel 1 output, PWM mode A (C1OCTRL = 110), its data register buffered (C1OBEN).
const CM1_C1_PWM_A: u32 = 6 << 4 | 1 << 3;
const CCTRL_C1EN: u32 = 1 << 0;

/// TMR8 on, channel 1 at 0 (dark), then PC6 handed to it. Called once by the UI task before the panel lights.
pub fn init() {
    modify(CRM_APB2EN, 0, CRM_APB2_TMR8);
    wr(TMR8 + TMR_CTRL1, 0);
    wr(TMR8 + TMR_DIV, 0);
    wr(TMR8 + TMR_PR, PERIOD - 1);
    wr(TMR8 + TMR_C1DT, 0);
    wr(TMR8 + TMR_CM1, CM1_C1_PWM_A);
    wr(TMR8 + TMR_CCTRL, CCTRL_C1EN);
    wr(TMR8 + TMR_SWEVT, 1); // load PR and C1DT now
    wr(TMR8 + TMR_BRK, TMR_BRK_OEN);
    wr(TMR8 + TMR_CTRL1, CTRL1_PRBEN | CTRL1_CNTEN);
    pin(GPIOC, 6).config(Mode::AfPp);
}

/// The duty in thousandths: 0 is dark (the output held low), 1000 full on (always high).
pub fn set(permille: u16) {
    let c = PERIOD * permille.min(1000) as u32 / 1000;
    wr(TMR8 + TMR_C1DT, if permille >= 1000 { PERIOD } else { c });
}
