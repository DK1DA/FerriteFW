//! Bit-banged I2C master. SCL is push-pull (single master, like stock); SDA is open-drain in software:
//! driven low as an output, released as an input with the internal pull-up. Mirrors hal_i2c_soft.
use super::gpio::{Mode, Pin};
use super::time::delay_us;

pub struct I2c {
    pub scl: Pin,
    pub sda: Pin,
    pub half_us: u32,
}

impl I2c {
    fn sda_low(&self) {
        self.sda.output(false, Mode::OutPp);
    }
    fn sda_rel(&self) {
        self.sda.config(Mode::InPu);
    }
    fn half(&self) {
        delay_us(self.half_us);
    }

    pub fn init(&self) {
        self.sda_rel();
        self.scl.output(true, Mode::OutPp);
        // Unstick a slave holding SDA low mid-byte: clock until it lets go, then STOP.
        for _ in 0..9 {
            if self.sda.read() {
                break;
            }
            self.scl.clr();
            self.half();
            self.scl.set();
            self.half();
        }
        self.sda_low();
        self.half();
        self.sda_rel();
        self.half();
    }

    /// Both lines to inputs.
    pub fn release(&self) {
        self.scl.config(Mode::InPu);
        self.sda.config(Mode::InPu);
    }

    fn start(&self) {
        self.sda_rel();
        self.scl.set();
        self.half();
        self.sda_low();
        self.half();
        self.scl.clr();
        self.half();
    }

    fn stop(&self) {
        self.sda_low();
        self.half();
        self.scl.set();
        self.half();
        self.sda_rel();
        self.half();
    }

    fn put(&self, v: u8) -> bool {
        for i in (0..8).rev() {
            if v >> i & 1 != 0 { self.sda_rel() } else { self.sda_low() }
            self.half();
            self.scl.set();
            self.half();
            self.scl.clr();
        }
        self.sda_rel();
        self.half();
        self.scl.set();
        self.half();
        let ack = !self.sda.read();
        self.scl.clr();
        self.half();
        ack
    }

    fn get(&self, ack: bool) -> u8 {
        let mut v = 0;
        self.sda_rel();
        for _ in 0..8 {
            self.half();
            self.scl.set();
            self.half();
            v = v << 1 | self.sda.read() as u8;
            self.scl.clr();
        }
        if ack {
            self.sda_low();
        }
        self.half();
        self.scl.set();
        self.half();
        self.scl.clr();
        self.sda_rel();
        v
    }

    /// False on NACK.
    pub fn write(&self, addr7: u8, buf: &[u8]) -> bool {
        self.start();
        let mut ok = self.put(addr7 << 1);
        for &b in buf {
            if !ok {
                break;
            }
            ok = self.put(b);
        }
        self.stop();
        ok
    }

    pub fn read(&self, addr7: u8, buf: &mut [u8]) -> bool {
        self.start();
        let ok = self.put(addr7 << 1 | 1);
        if ok {
            let n = buf.len();
            for (i, b) in buf.iter_mut().enumerate() {
                *b = self.get(i + 1 < n);
            }
        }
        self.stop();
        ok
    }
}
