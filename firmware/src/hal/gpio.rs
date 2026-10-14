//! GPIO: pins, modes, atomic set and clear. Mirrors fw/hal/hal_gpio.
use super::regs::*;
use super::{modify, rd, wr};

pub const GPIOA: u32 = 0x4001_0800;
pub const GPIOB: u32 = 0x4001_0C00;
pub const GPIOC: u32 = 0x4001_1000;
pub const GPIOD: u32 = 0x4001_1400;
pub const GPIOE: u32 = 0x4001_1800;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    OutPp,     // push-pull, 2 MHz
    OutPpFast, // push-pull, 50 MHz (display bus)
    OutOd,     // open drain, 2 MHz
    AfPp,      // peripheral output, push-pull, 50 MHz
    InFloat,
    InPu,
    InPd,
    Analog,
}

#[derive(Clone, Copy)]
pub struct Pin {
    pub port: u32,
    pub n: u8,
}

pub const fn pin(port: u32, n: u8) -> Pin {
    Pin { port, n }
}

impl Pin {
    #[inline(always)]
    pub fn set(self) {
        wr(self.port + GPIO_SCR, 1 << self.n);
    }

    #[inline(always)]
    pub fn clr(self) {
        wr(self.port + GPIO_CLR, 1 << self.n);
    }

    #[inline(always)]
    pub fn write(self, v: bool) {
        if v { self.set() } else { self.clr() }
    }

    pub fn read(self) -> bool {
        rd(self.port + GPIO_IDT) >> self.n & 1 != 0
    }

    /// What we drive.
    pub fn out_level(self) -> bool {
        rd(self.port + GPIO_ODT) >> self.n & 1 != 0
    }

    pub fn config(self, m: Mode) {
        let nibble = match m {
            Mode::OutPp => 0x2,
            Mode::OutPpFast => 0x3,
            Mode::OutOd => 0x6,
            Mode::AfPp => 0xB,
            Mode::InFloat => 0x4,
            Mode::InPu | Mode::InPd => 0x8,
            Mode::Analog => 0x0,
        };
        let cfg = self.port + if self.n < 8 { GPIO_CFGLR } else { GPIO_CFGHR };
        let sh = (self.n as u32 & 7) * 4;
        // The read-modify-write of the shared configuration register in a critical section: tasks on different
        // executors (the Radio task drives PB10 before a key-up, the GPS task hands it back) configure pins of
        // the same port half.
        cortex_m::interrupt::free(|_| modify(cfg, 0xF << sh, nibble << sh));
        // Pull direction comes from the output register. Set it only once the pin is an input, so releasing
        // a pin that was driven low never drives it high on the way (open-drain emulation).
        match m {
            Mode::InPu => self.set(),
            Mode::InPd => self.clr(),
            _ => {}
        }
    }

    /// Output with the level set first, so the pin never glitches.
    pub fn output(self, level: bool, m: Mode) {
        self.write(level);
        self.config(m);
    }
}

/// IOMUX and every GPIO port clocked.
pub fn clocks_on() {
    modify(CRM_APB2EN, 0, CRM_APB2_IOMUX | CRM_APB2_GPIOA_E);
}

/// JTAG off, SWD kept: PB3 (backlight 2) and PB4 (encoder A) are JTAG pins after reset.
pub fn jtag_off() {
    modify(IOMUX_REMAP, IOMUX_SWJTAG_MASK, IOMUX_SWJTAG_SWD_ONLY);
}

/// All 16 input levels of a port.
pub fn port_input(port: u32) -> u16 {
    rd(port + GPIO_IDT) as u16
}

/// One byte onto pins 8-15 of a port in one atomic set/clear, leaving pins 0-7 alone.
#[inline(always)]
pub fn write_high_byte(port: u32, b: u8) {
    wr(port + GPIO_SCR, (b as u32) << 8 | ((!b) as u32) << 24);
}

/// Reads the port's output register back: the write before it has reached the pins once this returns (the bus
/// orders the read after the write), so pin timings don't depend on the write buffer.
#[inline(always)]
pub fn sync(port: u32) {
    let _ = rd(port + GPIO_ODT);
}

/// As [`write_high_byte`], and clears the low-byte pins in `clear` in the same atomic write.
#[inline(always)]
pub fn write_high_byte_clear(port: u32, b: u8, clear: u8) {
    wr(port + GPIO_SCR, (b as u32) << 8 | ((!b) as u32) << 24 | (clear as u32) << 16);
}
