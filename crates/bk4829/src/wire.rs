//! The BK4829's 3-wire bus, bit-banged: SCK and SDATA shared by both chips, a chip select each.
//!
//! A transfer: CS low, 8 address bits (bit 7 = read), then 16 data bits, MSB first; the MCU drives SDATA and the
//! chip latches on SCK rising. For a read the MCU releases SDATA after the address and the chip drives it.
//!
//! The datasheet leaves open whether data bit 15 is valid before or after the first falling edge of the read.
//! Each read takes 17 samples, which hold both alignments; [`ThreeWire::detect`] picks the one where REG_00 reads
//! 0x4829 (the chip ID), and reads use it from then on.

use crate::Bus;
use uradio_bus::{Delay, IoPin, OutPin};

/// Where data bit 15 of a read is valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// With SCK still high after the last address bit (the BK4819 drivers' timing).
    BeforeFall,
    /// After the first falling edge.
    AfterFall,
}

/// SCK, SDATA and a delay; chip selects are passed per transfer, so both chips share one.
pub struct ThreeWire<K: OutPin, S: IoPin, D: Delay> {
    sck: K,
    sda: S,
    delay: D,
    align: Align,
}

/// REG_00 holds the chip ID.
pub const ID: u16 = 0x4829;

/// Half an SCK period, in microseconds: stock's (V0.27 0x0801DE60 waits `delay(5)` between edges, 155 loop turns,
/// about 5 us at 120 MHz, so SCK runs near 100 kHz). 1 us worked on the 8 MHz bring-up clock only because call
/// overhead stretched it; at 120 MHz chip 0 stopped answering (Oct 12, 2026).
pub const HALF_US: u32 = 5;

impl<K: OutPin, S: IoPin, D: Delay> ThreeWire<K, S, D> {
    /// Leaves SCK low and SDATA released. Reads use [`Align::BeforeFall`] until [`ThreeWire::detect`].
    pub fn new(mut sck: K, mut sda: S, delay: D) -> Self {
        sck.set(false);
        sda.drive(false);
        ThreeWire { sck, sda, delay, align: Align::BeforeFall }
    }

    pub fn align(&self) -> Align {
        self.align
    }

    fn address(&mut self, out: u8) {
        self.sda.drive(true);
        for i in (0..8).rev() {
            self.sck.set(false);
            self.sda.set(out >> i & 1 != 0);
            self.delay.delay_us(HALF_US);
            self.sck.set(true);
            self.delay.delay_us(HALF_US);
        }
    }

    /// 17 samples: the first with SCK still high after the address, then one after each of 16 falling edges.
    fn samples(&mut self, cs: &mut impl OutPin, reg: u8) -> u32 {
        self.sck.set(false);
        cs.set(false);
        self.delay.delay_us(HALF_US);
        self.address(0x80 | (reg & 0x7F));
        self.sda.drive(false); // turnaround
        self.delay.delay_us(HALF_US);
        let mut bits = self.sda.is_high() as u32;
        for k in 1..=16 {
            self.sck.set(false);
            self.delay.delay_us(if k == 1 { 2 * HALF_US } else { HALF_US });
            bits = bits << 1 | self.sda.is_high() as u32;
            if k < 16 {
                self.sck.set(true);
                self.delay.delay_us(HALF_US);
            }
        }
        cs.set(true);
        self.sck.set(false);
        bits
    }

    pub fn read(&mut self, cs: &mut impl OutPin, reg: u8) -> u16 {
        let bits = self.samples(cs, reg);
        match self.align {
            Align::BeforeFall => (bits >> 1) as u16,
            Align::AfterFall => bits as u16,
        }
    }

    pub fn write(&mut self, cs: &mut impl OutPin, reg: u8, value: u16) {
        self.sck.set(false);
        cs.set(false);
        self.delay.delay_us(HALF_US);
        self.address(reg & 0x7F);
        for i in (0..16).rev() {
            self.sck.set(false);
            self.sda.set(value >> i & 1 != 0);
            self.delay.delay_us(HALF_US);
            self.sck.set(true);
            self.delay.delay_us(HALF_US);
        }
        self.sck.set(false);
        cs.set(true);
        self.sda.drive(false);
    }

    /// Reads the chip ID with both alignments; keeps the one that gives it. None: no BK4829 answered.
    pub fn detect(&mut self, cs: &mut impl OutPin) -> Option<Align> {
        let bits = self.samples(cs, 0x00);
        let align = if bits as u16 == ID {
            Align::AfterFall
        } else if (bits >> 1) as u16 == ID {
            Align::BeforeFall
        } else {
            return None;
        };
        self.align = align;
        Some(align)
    }

    /// One chip on the bus, for the driver.
    pub fn chip<'a, C: OutPin>(&'a mut self, cs: &'a mut C) -> Chip<'a, K, S, D, C> {
        Chip { wire: self, cs }
    }

    pub fn release(self) -> (K, S, D) {
        (self.sck, self.sda, self.delay)
    }
}

/// A chip select with the bus: what [`crate::Bk4829`] talks through.
pub struct Chip<'a, K: OutPin, S: IoPin, D: Delay, C: OutPin> {
    wire: &'a mut ThreeWire<K, S, D>,
    cs: &'a mut C,
}

impl<K: OutPin, S: IoPin, D: Delay, C: OutPin> Bus for Chip<'_, K, S, D, C> {
    fn write(&mut self, reg: u8, value: u16) {
        self.wire.write(self.cs, reg, value)
    }
    fn read(&mut self, reg: u8) -> u16 {
        self.wire.read(self.cs, reg)
    }
}
