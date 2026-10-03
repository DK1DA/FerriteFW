//! NVIC: priorities 0-15 (4 bits, lower is more urgent), enable, disable, and the start-up clear.
use super::regs::*;
use super::{wr, wr8};

pub const USART1: u16 = 37;
pub const USART3: u16 = 39;
pub const UART4: u16 = 52;

pub fn set_priority(irq: u16, prio: u8) {
    wr8(NVIC_IPR + irq as u32, prio << 4);
}

pub fn enable(irq: u16, prio: u8) {
    set_priority(irq, prio);
    wr(NVIC_ISER + 4 * (irq as u32 / 32), 1 << (irq % 32));
}

pub fn disable(irq: u16) {
    wr(NVIC_ICER + 4 * (irq as u32 / 32), 1 << (irq % 32));
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

/// Every interrupt disabled and none pending (the bootloader may have left some live).
pub fn disable_all() {
    for i in 0..8 {
        wr(NVIC_ICER + 4 * i, u32::MAX);
        wr(NVIC_ICPR + 4 * i, u32::MAX);
    }
}

/// An interrupt line, for embassy's InterruptExecutor.
#[derive(Clone, Copy)]
pub struct Irq(pub u16);

// SAFETY: the number is an interrupt line of this chip (all users pass constants below 96).
unsafe impl cortex_m::interrupt::InterruptNumber for Irq {
    fn number(self) -> u16 {
        self.0
    }
}
