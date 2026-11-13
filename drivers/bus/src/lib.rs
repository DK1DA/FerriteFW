//! # uradio-bus
//!
//! The small traits uradio's drivers are written against: only what the chips on the radio need, so a HAL
//! implements each in a few lines and nothing unused reaches the image. (embedded-hal's transaction-list traits
//! measured 210 bytes more for the same flash and I2C operations, per bus.)
//!
//! With the `mock` feature, [`mock`] has recording buses for host tests: every transaction is kept, and replies
//! come from a script or a simulated device.

#![cfg_attr(not(feature = "mock"), no_std)]

/// A chip on an SPI bus: chip select and byte transfers. `write` and `read` have byte-loop defaults; a HAL can
/// override them (DMA, FIFO).
pub trait Spi {
    /// Chip select: true = selected (CS low).
    fn select(&mut self, on: bool);
    /// Send a byte, return the byte received at the same time.
    fn transfer(&mut self, byte: u8) -> u8;

    fn write(&mut self, data: &[u8]) {
        for &b in data {
            self.transfer(b);
        }
    }

    fn read(&mut self, buf: &mut [u8]) {
        for b in buf {
            *b = self.transfer(0);
        }
    }
}

/// An I2C bus (7-bit addresses). Errors are only "no acknowledge".
pub trait I2c {
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack>;
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), Nack>;
}

/// The addressed device did not acknowledge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nack;

/// Busy-wait delays, for the short waits chips need between steps. Long waits are left to the caller (an async
/// timer), through the drivers' start/poll calls.
pub trait Delay {
    fn delay_us(&mut self, us: u32);
    /// A wait in ns; by default the us wait that covers it (an implementation with a finer clock overrides it).
    fn delay_ns(&mut self, ns: u32) {
        self.delay_us(ns.div_ceil(1000))
    }
}

/// An output pin.
pub trait OutPin {
    fn set(&mut self, high: bool);
}

/// An input pin.
pub trait InPin {
    fn is_high(&mut self) -> bool;
}

/// A pin that is driven or read in turn (a half-duplex data line, such as the BK4829's SDATA).
pub trait IoPin: OutPin + InPin {
    /// true = drive it (push-pull), false = release it (input with pull-up).
    fn drive(&mut self, on: bool);
}

#[cfg(feature = "mock")]
pub mod mock;
