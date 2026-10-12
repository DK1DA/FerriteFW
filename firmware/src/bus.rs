//! The board's pins and buses as uradio's bus traits, so the chip drivers run on them unchanged.

use crate::board::{FLASH_CS, LCD_CS, LCD_DATA_PORT, LCD_DC, LCD_RST, LCD_WR};
use crate::hal::gpio::{self, pin, Mode, Pin};
use crate::hal::{i2c, spi, time};
use uradio_bus::{Delay, I2c, InPin, IoPin, Nack, OutPin, Spi};

/// A push-pull output.
pub struct Out(pub Pin);
impl OutPin for Out {
    fn set(&mut self, high: bool) {
        self.0.write(high)
    }
}

/// An input (configured by board::init or the owner).
pub struct In(pub Pin);
impl InPin for In {
    fn is_high(&mut self) -> bool {
        self.0.read()
    }
}

/// A half-duplex data line: driven push-pull, or released with the pull-up (the BK4829's SDATA).
pub struct Io(pub Pin);
impl OutPin for Io {
    fn set(&mut self, high: bool) {
        self.0.write(high)
    }
}
impl InPin for Io {
    fn is_high(&mut self) -> bool {
        self.0.read()
    }
}
impl IoPin for Io {
    fn drive(&mut self, on: bool) {
        if on {
            self.0.output(self.0.out_level(), Mode::OutPp)
        } else {
            self.0.config(Mode::InPu)
        }
    }
}

/// Busy-wait microseconds (cycle counter); only for the short waits inside a chip transaction.
pub struct BusyDelay;
impl Delay for BusyDelay {
    fn delay_us(&mut self, us: u32) {
        time::delay_us(us)
    }
}

/// The SPI flash on SPI2, chip select PB12.
pub struct Flash;
impl Flash {
    pub fn new() -> Self {
        spi::spi2_init();
        FLASH_CS.output(true, Mode::OutPp);
        Flash
    }
}
impl Spi for Flash {
    fn select(&mut self, on: bool) {
        FLASH_CS.write(!on)
    }
    fn transfer(&mut self, byte: u8) -> u8 {
        spi::spi2_xfer(byte)
    }
}

/// The SI4732's bit-banged I2C.
pub struct SiBus(pub i2c::I2c);
impl I2c for SiBus {
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack> {
        if self.0.write(addr, data) { Ok(()) } else { Err(Nack) }
    }
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), Nack> {
        if self.0.read(addr, buf) { Ok(()) } else { Err(Nack) }
    }
}

/// The display's 8080 bus: data on PD8-PD15, write strobe PD0, D/C PD3, reset PD2. The panel is alone on it, so
/// chip select stays low.
pub struct Lcd;
impl Lcd {
    pub fn new() -> Self {
        LCD_CS.output(false, Mode::OutPpFast);
        LCD_WR.output(true, Mode::OutPpFast);
        LCD_DC.output(true, Mode::OutPpFast);
        LCD_RST.output(true, Mode::OutPp);
        for n in 8..16 {
            pin(LCD_DATA_PORT, n).config(Mode::OutPpFast);
        }
        Lcd
    }
}

#[inline(always)]
fn nops<const N: usize>() {
    for _ in 0..N {
        cortex_m::asm::nop();
    }
}

#[inline(always)]
fn byte(b: u8) {
    // One write to PD8-PD15, leaving PD0-PD7 alone (PD4-PD7 are the keypad sense pull-ups); latched on WR rising.
    // At 120 MHz back-to-back port writes break the ST7789's 8080 timing (one-colour fills went missing on the
    // radio, Oct 12, 2026; the 8 MHz image was fine): data setup before WR falls, WR low six nops as stock
    // (0x080293E8), then a hold with WR high before the next byte's data, so a tight fill loop keeps the write
    // cycle (66 ns), data hold (10 ns) and WR high time (15 ns) as text's slower loop does.
    gpio::write_high_byte(LCD_DATA_PORT, b);
    nops::<2>();
    LCD_WR.clr();
    nops::<6>();
    LCD_WR.set();
    nops::<4>();
}

impl st7789::Bus8080 for Lcd {
    fn command(&mut self, cmd: u8) {
        LCD_DC.clr();
        byte(cmd);
        LCD_DC.set();
    }
    fn data(&mut self, data: &[u8]) {
        for &b in data {
            byte(b)
        }
    }
    fn repeat(&mut self, color: u16, count: u32) {
        let (hi, lo) = ((color >> 8) as u8, color as u8);
        for _ in 0..count {
            byte(hi);
            byte(lo);
        }
    }
    fn reset(&mut self, active: bool) {
        LCD_RST.write(!active)
    }
}
