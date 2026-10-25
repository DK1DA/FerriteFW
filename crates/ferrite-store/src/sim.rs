//! A simulated NOR flash (the first 512 KB of the radio's 4 MB) for the simulator and the tests: erased bytes
//! are 0xFF, a program only clears bits, an erase sets a 4 KB sector. Programs and erases finish as they start,
//! so the store never waits on it. It can cut the power at any point: after a set number of operations (each
//! programmed byte and each erase counts one) the operation in progress is left half done, the worst way NOR
//! leaves it, and every call fails until [`SimFlash::power_on`]. Writing outside
//! [`ferrite_core::layout::WRITABLE`] panics: calibration and the update area must never be touched.

use crate::{Nor, NorError, Result, SECTOR};
use ferrite_core::layout::{HEARD, SCRATCH_RING, WRITABLE};

/// The chip up to the end of the heard list (564 KB of the 4 MB), where everything Ferrite stores is, then the
/// scratch ring (decision 227) in a window of its own: `mem` holds the two back to back ([`SimFlash::at`]);
/// anything else is out of range here.
pub const LOW: usize = HEARD.end as usize;
pub const SIZE: usize = LOW + (SCRATCH_RING.end - SCRATCH_RING.start) as usize;
/// The whole chip, for the erase counts.
const CHIP: usize = 4 << 20;

#[derive(Clone)]
pub struct SimFlash {
    pub mem: Vec<u8>,
    /// Operations so far (programmed bytes and erases).
    pub ops: u64,
    /// Cut the power when `ops` reaches this.
    pub cut_at: Option<u64>,
    /// The power is off: everything fails.
    pub dead: bool,
    /// Erases done in each sector (decision 227's wear tests).
    pub erases: Vec<u32>,
    /// A change since the owner last looked (the simulator saves the image when it is set).
    pub dirty: bool,
    rng: u64,
}

impl SimFlash {
    /// Where chip address `addr` is in `mem`.
    pub fn at(addr: u32) -> usize {
        match addr {
            a if (a as usize) < LOW => a as usize,
            a if SCRATCH_RING.contains(&a) => LOW + (a - SCRATCH_RING.start) as usize,
            a => panic!("{a:#x}: outside the simulated flash"),
        }
    }

    /// A blank chip, as after installing (only calibration, which this never touches, would be left).
    pub fn new() -> Self {
        SimFlash { mem: vec![0xFF; SIZE], ops: 0, cut_at: None, dead: false, dirty: false, erases: vec![0; CHIP / SECTOR as usize], rng: 0x9E37_79B9_7F4A_7C15 }
    }

    /// A chip holding `image` at `at` (a saved region), blank elsewhere.
    pub fn with_image(at: u32, image: &[u8]) -> Self {
        let mut f = Self::new();
        let a = at as usize;
        let n = image.len().min(LOW - a);
        f.mem[a..a + n].copy_from_slice(&image[..n]);
        f
    }

    /// Seeds the half-done bits of a cut (tests vary it).
    pub fn seed(&mut self, s: u64) {
        self.rng = s | 1;
    }

    /// The power back on after a cut.
    pub fn power_on(&mut self) {
        self.dead = false;
        self.cut_at = None;
    }

    fn rand(&mut self) -> u8 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng as u8
    }

    fn writable(addr: u32, len: u32) {
        assert!(
            WRITABLE.iter().any(|r| addr >= r.start && addr + len <= r.end),
            "write outside the writable ranges: {addr:#x}+{len:#x}"
        );
    }

    /// One operation; false when the power goes on it.
    fn tick(&mut self) -> bool {
        self.ops += 1;
        if self.cut_at.is_some_and(|c| self.ops >= c) {
            self.dead = true;
            return false;
        }
        true
    }
}

impl Default for SimFlash {
    fn default() -> Self {
        Self::new()
    }
}

impl Nor for SimFlash {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result {
        if self.dead {
            return Err(NorError);
        }
        let a = Self::at(addr);
        buf.copy_from_slice(&self.mem[a..a + buf.len()]);
        Ok(())
    }

    fn start_program(&mut self, addr: u32, data: &[u8]) -> Result {
        if self.dead {
            return Err(NorError);
        }
        assert!(addr / crate::PAGE == (addr + data.len() as u32 - 1) / crate::PAGE, "program at {addr:#x} crosses a page");
        Self::writable(addr, data.len() as u32);
        self.dirty = true;
        for (i, &d) in data.iter().enumerate() {
            let a = Self::at(addr) + i;
            if !self.tick() {
                // Cut: this byte and the rest of the call get some of their bits, at random.
                for (k, &d) in data.iter().enumerate().skip(i) {
                    let clear = !d & self.rand();
                    self.mem[Self::at(addr) + k] &= !clear;
                }
                return Err(NorError);
            }
            self.mem[a] &= d;
        }
        Ok(())
    }

    fn start_erase(&mut self, addr: u32) -> Result {
        if self.dead {
            return Err(NorError);
        }
        assert_eq!(addr % SECTOR, 0, "erase at {addr:#x}: not a sector");
        Self::writable(addr, SECTOR);
        self.dirty = true;
        self.erases[(addr / SECTOR) as usize] += 1;
        let a = Self::at(addr);
        if !self.tick() {
            // Cut: some bits of every byte are set, at random.
            for i in 0..SECTOR as usize {
                let set = self.rand();
                self.mem[a + i] |= set;
            }
            return Err(NorError);
        }
        self.mem[a..a + SECTOR as usize].fill(0xFF);
        Ok(())
    }

    fn busy(&mut self) -> Result<bool> {
        if self.dead { Err(NorError) } else { Ok(false) }
    }

    async fn pause() {}
}
