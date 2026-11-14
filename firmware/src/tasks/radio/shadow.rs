//! The BK4829 register shadow (PWR-26.16, decision 391): a RAM copy of a chip's setting registers, kept by the one
//! write op every register write goes through ([`Radio::chip`]), so a write of the value the chip already holds is
//! skipped. Only plain settings are kept (a write to a trigger register does something each time); a soft reset
//! (REG_00) forgets them all. The chips keep their registers through sleep (`Bk4829::sleep`). Never a per-caller
//! cache of register state: those go stale when another path writes the same register (Alex, Nov 12, 2026).

use bk4829::{reg, Bus};

/// The registers kept: settings only, whose value is all a write does. REG_47, the audio route, is written for
/// each tuned chip on every receive tick (10 ms): about 75 us a write on the bus at its fixed speed.
const KEPT: [u8; 1] = [reg::AF_OUT];

/// One chip's copy: None until written (after boot or a reset).
#[derive(Default)]
pub(super) struct Shadow([Option<u16>; KEPT.len()]);

/// Bench builds: the bus's reads and writes, and the writes skipped, since boot (the cable log's `bk bus` line).
#[cfg(feature = "bench")]
pub static COUNTS: [core::sync::atomic::AtomicU32; 3] = [const { core::sync::atomic::AtomicU32::new(0) }; 3];

#[cfg(feature = "bench")]
fn count(i: usize) {
    COUNTS[i].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}
#[cfg(not(feature = "bench"))]
fn count(_: usize) {}

/// A chip's bus through its shadow.
pub(super) struct Shadowed<'a, B: Bus> {
    pub(super) bus: B,
    pub(super) shadow: &'a mut Shadow,
}

impl<B: Bus> Bus for Shadowed<'_, B> {
    fn write(&mut self, r: u8, value: u16) {
        if r == reg::ID {
            self.shadow.0 = [None; KEPT.len()]; // soft reset: the chip's registers back to its own defaults
        } else if let Some(i) = KEPT.iter().position(|&k| k == r) {
            if self.shadow.0[i] == Some(value) {
                count(2);
                return;
            }
            self.shadow.0[i] = Some(value);
        }
        count(1);
        self.bus.write(r, value)
    }
    fn read(&mut self, r: u8) -> u16 {
        count(0);
        self.bus.read(r)
    }
    fn delay_us(&mut self, us: u32) {
        self.bus.delay_us(us)
    }
}
