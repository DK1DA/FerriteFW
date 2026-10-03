//! Core, AHB and APB clocks. This image stays on the 8 MHz internal oscillator (HICK) the chip starts on;
//! raising it (stock runs 120 MHz from the crystal) is a later bring-up step. Mirrors fw/hal/hal_clock.
use super::regs::*;
use super::{modify, rd};

pub const SYSCLK_HZ: u32 = 8_000_000;
pub const APB1_HZ: u32 = SYSCLK_HZ;
pub const APB2_HZ: u32 = SYSCLK_HZ;

/// The app is entered by a jump from the bootloader, not a reset, so the clock tree is whatever the
/// bootloader left. Put it back to the reset state: SCLK from HICK, PLL off, AHB/APB undivided. Every
/// timing in the image (tick, UART divisor, LCD strobes) assumes this.
pub fn init() {
    modify(CRM_CTRL, 0, CRM_CTRL_HICKEN);
    while rd(CRM_CTRL) & CRM_CTRL_HICKSTBL == 0 {}
    modify(CRM_CFG, CRM_CFG_SCLKSEL_MASK, 0);
    while rd(CRM_CFG) & CRM_CFG_SCLKSTS_MASK != 0 {}
    modify(CRM_CFG, CRM_CFG_DIV_MASK, 0);
    modify(CRM_CTRL, CRM_CTRL_PLLEN, 0);
}

/// True once `init` has run (only the STM32-common bits are checked).
pub fn is_reset_tree() -> bool {
    rd(CRM_CFG) & (CRM_CFG_SCLKSTS_MASK | CRM_CFG_DIV_MASK) == 0
}
