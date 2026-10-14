//! Recording buses for host tests.

use crate::{Delay, I2c, Nack, Spi};

/// One SPI chip-select period: bytes sent, bytes received.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpiTxn {
    pub out: Vec<u8>,
    pub inp: Vec<u8>,
}

/// An SPI bus whose replies come from a device model: `reply(sent_so_far_in_this_txn, byte) -> byte`.
pub struct MockSpi<F: FnMut(&[u8], u8) -> u8> {
    pub txns: Vec<SpiTxn>,
    selected: bool,
    device: F,
}

impl<F: FnMut(&[u8], u8) -> u8> MockSpi<F> {
    pub fn new(device: F) -> Self {
        MockSpi { txns: Vec::new(), selected: false, device }
    }
}

impl<F: FnMut(&[u8], u8) -> u8> Spi for MockSpi<F> {
    fn select(&mut self, on: bool) {
        assert_ne!(on, self.selected, "chip select toggled twice the same way");
        self.selected = on;
        if on {
            self.txns.push(SpiTxn::default());
        }
    }

    fn transfer(&mut self, byte: u8) -> u8 {
        assert!(self.selected, "transfer without chip select");
        let t = self.txns.last_mut().unwrap();
        let r = (self.device)(&t.out, byte);
        t.out.push(byte);
        t.inp.push(r);
        r
    }
}

/// An I2C transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum I2cTxn {
    Write(u8, Vec<u8>),
    Read(u8, usize),
}

/// An I2C bus whose read replies come from a device model, given everything sent so far.
pub struct MockI2c<F: FnMut(&[I2cTxn], &mut [u8]) -> Result<(), Nack>> {
    pub txns: Vec<I2cTxn>,
    device: F,
}

impl<F: FnMut(&[I2cTxn], &mut [u8]) -> Result<(), Nack>> MockI2c<F> {
    pub fn new(device: F) -> Self {
        MockI2c { txns: Vec::new(), device }
    }
}

impl<F: FnMut(&[I2cTxn], &mut [u8]) -> Result<(), Nack>> I2c for MockI2c<F> {
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack> {
        self.txns.push(I2cTxn::Write(addr, data.to_vec()));
        Ok(())
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), Nack> {
        let r = (self.device)(&self.txns, buf);
        self.txns.push(I2cTxn::Read(addr, buf.len()));
        r
    }
}

/// Records the total delay asked for.
#[derive(Default)]
pub struct MockDelay {
    pub total_us: u64,
}

impl Delay for MockDelay {
    fn delay_us(&mut self, us: u32) {
        self.total_us += us as u64;
    }
}
