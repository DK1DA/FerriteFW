//! The flash as the store uses it: reads that see a write in progress the way it will end, and programs and
//! erases that wait for the chip without holding it.
//!
//! On the radio the UI reads channel records while the Storage task writes. Two windows would show it bytes
//! that are neither old nor new, so reads look elsewhere for them:
//! - a record programmed into erased bytes: the record itself, from RAM ([`Flash::program_record`]);
//! - a sector being erased and copied back from the scratch sector: the scratch copy, or erased bytes when
//!   nothing comes back ([`Flash::redirect`]).

use crate::{Crc32, Nor, Result, PAGE, SECTOR};
use core::cell::{Cell, RefCell};
use ferrite_core::layout::SCRATCH;

/// The largest record programmed in place (a channel).
const PEND_MAX: usize = 64;

pub struct Flash<N: Nor> {
    nor: RefCell<N>,
    /// This sector's reads come from the scratch sector (true) or read erased (false).
    redirect: Cell<Option<(u32, bool)>>,
    /// A record being programmed in place, and its bytes.
    pending: Cell<Option<(u32, usize)>>,
    pend: RefCell<[u8; PEND_MAX]>,
}

impl<N: Nor> Flash<N> {
    pub fn new(nor: N) -> Self {
        Flash { nor: RefCell::new(nor), redirect: Cell::new(None), pending: Cell::new(None), pend: RefCell::new([0xFF; PEND_MAX]) }
    }

    pub fn into_inner(self) -> N {
        self.nor.into_inner()
    }

    pub fn nor(&self) -> core::cell::RefMut<'_, N> {
        self.nor.borrow_mut()
    }

    /// What a reader should see at `addr` (`buf` stays inside one sector).
    pub fn read(&self, addr: u32, buf: &mut [u8]) -> Result {
        match self.redirect.get() {
            Some((s, true)) if (s..s + SECTOR).contains(&addr) => self.raw_read(SCRATCH.start + addr - s, buf)?,
            Some((s, false)) if (s..s + SECTOR).contains(&addr) => buf.fill(0xFF),
            _ => self.raw_read(addr, buf)?,
        }
        if let Some((p, n)) = self.pending.get() {
            let pend = self.pend.borrow();
            for (i, b) in buf.iter_mut().enumerate() {
                let a = addr + i as u32;
                if (p..p + n as u32).contains(&a) {
                    *b = pend[(a - p) as usize];
                }
            }
        }
        Ok(())
    }

    /// The bytes in the chip.
    pub fn raw_read(&self, addr: u32, buf: &mut [u8]) -> Result {
        self.nor.borrow_mut().read(addr, buf)
    }

    /// Whether `len` bytes at `addr` are erased.
    pub fn blank(&self, addr: u32, len: u32) -> Result<bool> {
        let mut buf = [0u8; 64];
        let mut a = addr;
        while a < addr + len {
            let n = (addr + len - a).min(64) as usize;
            self.raw_read(a, &mut buf[..n])?;
            if buf[..n].iter().any(|&b| b != 0xFF) {
                return Ok(false);
            }
            a += n as u32;
        }
        Ok(true)
    }

    /// CRC-32 of `len` bytes at `addr`.
    pub fn crc(&self, addr: u32, len: u32) -> Result<u32> {
        let mut buf = [0u8; 64];
        let mut c = Crc32::new();
        let mut a = addr;
        while a < addr + len {
            let n = (addr + len - a).min(64) as usize;
            self.raw_read(a, &mut buf[..n])?;
            c.update(&buf[..n]);
            a += n as u32;
        }
        Ok(c.get())
    }

    async fn wait(&self) -> Result {
        while self.nor.borrow_mut().busy()? {
            N::pause().await;
        }
        Ok(())
    }

    /// Programs `data` at `addr` (erased bytes), page by page; all-erased pieces are skipped.
    pub async fn program(&self, addr: u32, data: &[u8]) -> Result {
        let mut a = addr;
        let mut rest = data;
        while !rest.is_empty() {
            let n = ((PAGE - a % PAGE) as usize).min(rest.len());
            if rest[..n].iter().any(|&b| b != 0xFF) {
                self.nor.borrow_mut().start_program(a, &rest[..n])?;
                self.wait().await?;
            }
            a += n as u32;
            rest = &rest[n..];
        }
        Ok(())
    }

    pub async fn erase(&self, addr: u32) -> Result {
        self.nor.borrow_mut().start_erase(addr)?;
        self.wait().await
    }

    /// Erases a sector that readers should see as erased from the start (settings reset).
    pub async fn erase_blanked(&self, addr: u32) -> Result {
        self.redirect.set(Some((addr, false)));
        let r = self.erase(addr).await;
        self.redirect.set(None);
        r
    }

    /// Programs a record (up to 64 bytes) into erased bytes; readers see it whole meanwhile.
    pub async fn program_record(&self, addr: u32, rec: &[u8]) -> Result {
        self.pend.borrow_mut()[..rec.len()].copy_from_slice(rec);
        self.pending.set(Some((addr, rec.len())));
        let r = self.program(addr, rec).await;
        self.pending.set(None);
        r
    }

    /// Reads of `sector` come from the scratch sector until [`Flash::end_redirect`].
    pub fn redirect(&self, sector: u32) {
        self.redirect.set(Some((sector, true)));
    }

    pub fn end_redirect(&self) {
        self.redirect.set(None);
    }

    /// Copies `len` bytes from `from` to `to` (erased).
    pub async fn copy(&self, from: u32, to: u32, len: u32) -> Result {
        let mut buf = [0u8; PAGE as usize];
        let mut off = 0;
        while off < len {
            let n = (len - off).min(PAGE) as usize;
            self.raw_read(from + off, &mut buf[..n])?;
            self.program(to + off, &buf[..n]).await?;
            off += n as u32;
        }
        Ok(())
    }

    /// Whether `len` bytes at `a` and `b` are the same.
    pub fn same(&self, a: u32, b: u32, len: u32) -> Result<bool> {
        let (mut x, mut y) = ([0u8; 64], [0u8; 64]);
        let mut off = 0;
        while off < len {
            let n = (len - off).min(64) as usize;
            self.raw_read(a + off, &mut x[..n])?;
            self.raw_read(b + off, &mut y[..n])?;
            if x[..n] != y[..n] {
                return Ok(false);
            }
            off += n as u32;
        }
        Ok(true)
    }
}
