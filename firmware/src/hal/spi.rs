//! SPI2 master, mode 0, PB13 SCK / PB14 MISO / PB15 MOSI. Chip selects belong to the drivers.
use super::gpio::{pin, Mode, GPIOB};
use super::regs::*;
use super::{modify, rd, wr};

/// APB1 / 4 = 15 MHz, stock's divider (SPI2 CTRL1 0x34F at 0x08016C00); at another clock (`super::clock`), the
/// fastest divider that stays at or under 15 MHz.
const MDIV: u32 = {
    let mut n = 0;
    while super::clock::APB1_HZ >> (n + 1) > 15_000_000 {
        n += 1;
    }
    n
};
const _: () = assert!(super::clock::SYSCLK_HZ != 120_000_000 || MDIV == 1);

pub fn spi2_init() {
    modify(CRM_APB1EN, 0, CRM_APB1_SPI2);
    wr(SPI2 + SPI_CTRL1, 0);
    pin(GPIOB, 13).output(false, Mode::AfPp);
    pin(GPIOB, 14).config(Mode::InPu);
    pin(GPIOB, 15).output(false, Mode::AfPp);
    wr(SPI2 + SPI_CTRL1, SPI_CTRL1_MSTEN | SPI_CTRL1_SWCSEN | SPI_CTRL1_SWCSIL | MDIV << 3); // /4 at 120 MHz, mode 0, 8-bit
    modify(SPI2 + SPI_CTRL1, 0, SPI_CTRL1_SPIEN);
}

pub fn spi2_xfer(out: u8) -> u8 {
    while rd(SPI2 + SPI_STS) & SPI_STS_TDBE == 0 {}
    wr(SPI2 + SPI_DT, out as u32);
    while rd(SPI2 + SPI_STS) & SPI_STS_RDBF == 0 {}
    rd(SPI2 + SPI_DT) as u8
}
