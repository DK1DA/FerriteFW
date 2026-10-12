//! Core, AHB and APB clocks: stock's tree (V0.27 clock setup 0x08022694, analysis/peripherals_irq.md). The 8 MHz
//! crystal (HEXT) x 15 through the PLL = 120 MHz SYSCLK; AHB /1 = 120 MHz, APB1 /2 = 60 MHz, APB2 /2 = 60 MHz;
//! timers on either APB run at twice it, 120 MHz. Fixed for the life of the image (no runtime switching).
use super::regs::*;
use super::{modify, rd, wr};

pub const SYSCLK_HZ: u32 = 120_000_000;
pub const AHB_HZ: u32 = SYSCLK_HZ;
pub const APB1_HZ: u32 = 60_000_000;
pub const APB2_HZ: u32 = 60_000_000;
/// TMR clocks on APB1 and APB2: twice the bus clock, since both APB dividers are not 1.
pub const TMR_HZ: u32 = 120_000_000;

/// CRM_MISC2 bits 4-5: AUTO_STEP_EN, set by stock around the switch to the PLL so the core steps up gradually.
const CRM_MISC2: u32 = CRM + 0x54;
const AUTO_STEP: u32 = 3 << 4;
/// CRM_CFG: PLLRC (16), PLLHEXTDIV (17), PLLMULT (18-21 and 29-30), PLLRANGE (31): stock clears these with
/// 0x1FC0FFFF and sets 0x80350000 (HEXT undivided, multiplier code 13 = x15, range above 72 MHz).
const PLL_CLEAR: u32 = 0x1FC0_FFFF;
const PLL_SET: u32 = 0x8035_0000;
/// APB2 /2 (bits 11-13 = 0b100) and APB1 /2 (bits 8-10 = 0b100); AHB stays /1.
const APB2_DIV2: u32 = 0b100 << 11;
const APB1_DIV2: u32 = 0b100 << 8;

/// The app is entered by a jump from the bootloader, not a reset, so the clock tree is whatever the bootloader
/// left: back to HICK first (the PLL can only be set up while not in use), then stock's sequence. Every timing
/// in the image (tick, UART and SPI dividers, timers, LCD strobes) follows the constants above.
///
/// If the crystal doesn't start (stock waits 0x3000 polls), the image can't keep its timings: it stops here
/// with a panic, which the fault path shows (red LED, `*** FAULT` on the cable).
pub fn init() {
    // Back to the reset state: SCLK from HICK, PLL off, AHB/APB undivided.
    modify(CRM_CTRL, 0, CRM_CTRL_HICKEN);
    while rd(CRM_CTRL) & CRM_CTRL_HICKSTBL == 0 {}
    modify(CRM_CFG, CRM_CFG_SCLKSEL_MASK, 0);
    while rd(CRM_CFG) & CRM_CFG_SCLKSTS_MASK != 0 {}
    modify(CRM_CFG, CRM_CFG_DIV_MASK, 0);
    modify(CRM_CTRL, CRM_CTRL_PLLEN, 0);

    // Stock: HEXT on, wait for it.
    modify(CRM_CTRL, 0, CRM_CTRL_HEXTEN);
    let mut n = 0u32;
    while rd(CRM_CTRL) & CRM_CTRL_HEXTSTBL == 0 {
        n += 1;
        if n == 0x3000 * 16 {
            panic!("8 MHz crystal did not start");
        }
    }
    // APB dividers, then the PLL from HEXT x 15.
    modify(CRM_CFG, 7 << 11, APB2_DIV2);
    modify(CRM_CFG, 7 << 8, APB1_DIV2);
    wr(CRM_CFG, rd(CRM_CFG) & PLL_CLEAR | PLL_SET);
    modify(CRM_CTRL, 0, CRM_CTRL_PLLEN);
    while rd(CRM_CTRL) & CRM_CTRL_PLLSTBL == 0 {}
    // Switch with the automatic step on, as stock does, then off again.
    modify(CRM_MISC2, 0, AUTO_STEP);
    modify(CRM_CFG, CRM_CFG_SCLKSEL_MASK, 2);
    while rd(CRM_CFG) & CRM_CFG_SCLKSTS_MASK != 2 << 2 {}
    modify(CRM_MISC2, AUTO_STEP, 0);
}

/// True once `init` has run: SYSCLK from the PLL.
pub fn on_pll() -> bool {
    rd(CRM_CFG) & CRM_CFG_SCLKSTS_MASK == 2 << 2
}
