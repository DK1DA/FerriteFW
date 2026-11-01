//! The radio's side of `STOCK_RESTORE` (BACKUP-7, decision 261; the protocol is [`ferrite_core::restore`]): each
//! request carried out on the chip and answered. Only inside `ranges` (the driver's own writable ranges), never
//! calibration or the update area ([`ferrite_core::restore::allowed`]); the driver's guard refuses them as well.

use crate::{Crc32, Nor, Result};
use core::ops::Range;
use ferrite_core::layout::CALIBRATION;
use ferrite_core::link::Status;
use ferrite_core::restore::{self as r, Op};

/// What the caller does after sending the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    Next,
    /// `FINISH`: reset into the bootloader, saving nothing.
    Reset,
}

async fn wait<N: Nor>(nor: &mut N) -> Result {
    while nor.busy()? {
        N::pause().await;
    }
    Ok(())
}

async fn crc<N: Nor>(nor: &mut N, addr: u32, len: u32) -> Result<u32> {
    let mut c = Crc32::new();
    let mut buf = [0u8; 256];
    let (mut at, end) = (addr, addr + len);
    while at < end {
        let n = ((end - at) as usize).min(buf.len());
        nor.read(at, &mut buf[..n])?;
        c.update(&buf[..n]);
        at += n as u32;
        if at % 0x4000 == 0 {
            N::pause().await; // a long read lets the other tasks run
        }
    }
    Ok(c.get())
}

/// The `START` answer (status `Ok` first) into `out`: the chip, the calibration sector's CRC-32 (a sanity check:
/// blank or damaged), the ranges and the MCU's unique ID `uid` (so the PC can tell the backup is this radio's,
/// decision 263). Its length.
pub async fn start<N: Nor>(nor: &mut N, chip: u32, ranges: &[Range<u32>], uid: [u32; 3], out: &mut [u8]) -> usize {
    let Ok(cal) = crc(nor, CALIBRATION.start, CALIBRATION.end - CALIBRATION.start).await else {
        out[0] = Status::Busy as u8;
        return 1;
    };
    out[0] = Status::Ok as u8;
    1 + r::start_answer(chip, cal, ranges, uid, &mut out[1..])
}

/// One request after `START`: carried out, its answer (status first) in `out`. `START` again answers as the first
/// did.
pub async fn serve<N: Nor>(
    nor: &mut N,
    chip: u32,
    ranges: &[Range<u32>],
    uid: [u32; 3],
    payload: &[u8],
    out: &mut [u8],
) -> (usize, After) {
    let Some(op) = r::parse(payload) else {
        out[0] = Status::BadArg as u8;
        return (1, After::Next);
    };
    if let Err(s) = r::check(&op, ranges, chip) {
        out[0] = s as u8;
        return (1, After::Next);
    }
    let done = match op {
        Op::Start => return (start(nor, chip, ranges, uid, out).await, After::Next),
        Op::Finish => {
            out[0] = Status::Ok as u8;
            return (1, After::Reset);
        }
        Op::Erase(a) => match nor.start_erase(a) {
            Ok(()) => wait(nor).await,
            e => e,
        },
        Op::Program(a, d) => match nor.start_program(a, d) {
            Ok(()) => wait(nor).await,
            e => e,
        },
        Op::Crc(a, n) => match crc(nor, a, n).await {
            Ok(c) => {
                out[0] = Status::Ok as u8;
                out[1..5].copy_from_slice(&c.to_le_bytes());
                return (5, After::Next);
            }
            Err(e) => Err(e),
        },
    };
    // A driver refusal or a failed chip: `refused`, nothing more is tried for it.
    out[0] = if done.is_ok() { Status::Ok } else { Status::Refused } as u8;
    (1, After::Next)
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::block;
    use crate::sim::SimFlash;
    use ferrite_core::layout::{SETTINGS, UPDATE_AREA, WRITABLE};
    use std::vec::Vec;

    const UID: [u32; 3] = [0x3F2A_91C0, 0x7D44_0123, 0x89AB_CDEF];

    fn ask(f: &mut SimFlash, p: &[u8]) -> (u8, Vec<u8>, After) {
        let mut out = [0u8; 240];
        let (n, after) = block(serve(f, 4 << 20, &WRITABLE, UID, p, &mut out));
        (out[0], out[1..n].to_vec(), after)
    }

    fn req(op: u8, addr: u32, rest: &[u8]) -> Vec<u8> {
        let mut p = std::vec![op];
        p.extend_from_slice(&addr.to_le_bytes());
        p.extend_from_slice(rest);
        p
    }

    /// BACKUP-7: a sector erased, programmed in chunks and read back; the START answer names calibration's CRC, the
    /// ranges and the MCU's ID (decision 263); FINISH resets.
    #[test]
    fn restore_round_trip() {
        let mut f = SimFlash::new();
        f.mem[0xF000..0x10000].fill(0x5A); // calibration: never written by the simulated chip, only read
        let (s, a, _) = ask(&mut f, &[r::START]);
        assert_eq!(s, 0);
        assert_eq!(&a[..4], &(4u32 << 20).to_le_bytes());
        let mut c = Crc32::new();
        c.update(&[0x5A; 4096]);
        assert_eq!(&a[4..8], &c.get().to_le_bytes());
        assert_eq!(a[8] as usize, WRITABLE.len());
        assert_eq!(a.len(), 9 + 8 * WRITABLE.len() + r::UID_LEN);
        assert_eq!(&a[a.len() - r::UID_LEN..], &ferrite_core::uid::bytes(UID));

        let at = SETTINGS.start;
        f.mem[at as usize..at as usize + 4096].fill(0x00);
        assert_eq!(ask(&mut f, &req(r::ERASE, at, &[])).0, 0);
        assert!(f.mem[at as usize..at as usize + 4096].iter().all(|&b| b == 0xFF));
        let data: Vec<u8> = (0..4096u32).map(|i| (i * 7 + 1) as u8).collect();
        for k in (0..4096).step_by(r::CHUNK) {
            assert_eq!(ask(&mut f, &req(r::PROGRAM, at + k as u32, &data[k..k + r::CHUNK])).0, 0);
        }
        assert_eq!(&f.mem[at as usize..at as usize + 4096], &data[..]);
        let (s, a, _) = ask(&mut f, &req(r::CRC, at, &4096u32.to_le_bytes()));
        let mut c = Crc32::new();
        c.update(&data);
        assert_eq!((s, a), (0, c.get().to_le_bytes().to_vec()));
        assert_eq!(ask(&mut f, &[r::FINISH]).2, After::Reset);
    }

    /// BACKUP-7, LINK-4: calibration and the update area are refused before the chip is asked (the simulated chip
    /// would panic on them); everything else below the update area is layout 3's (decision 333).
    #[test]
    fn restore_refuses_protected_ranges() {
        let mut f = SimFlash::new();
        for a in [CALIBRATION.start, UPDATE_AREA.start, UPDATE_AREA.end - 4096] {
            assert_eq!(ask(&mut f, &req(r::ERASE, a, &[])).0, Status::Refused as u8, "{a:#x}");
            assert_eq!(ask(&mut f, &req(r::PROGRAM, a, &[0; 16])).0, Status::Refused as u8, "{a:#x}");
        }
        assert_eq!(ask(&mut f, &req(r::ERASE, SETTINGS.start + 1, &[])).0, Status::BadArg as u8);
        assert_eq!(ask(&mut f, &[7]).0, Status::BadArg as u8);
        assert_eq!(f.erases.iter().sum::<u32>(), 0, "nothing erased");
    }

    /// A chip that reports busy `busy` times after each start, and fails everything once `dead`.
    struct Slow {
        f: SimFlash,
        busy: u32,
        left: u32,
        polls: u32,
        dead: bool,
    }

    impl Nor for Slow {
        fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result {
            if self.dead { Err(crate::NorError) } else { self.f.read(addr, buf) }
        }
        fn start_program(&mut self, addr: u32, data: &[u8]) -> Result {
            self.left = self.busy;
            if self.dead { Err(crate::NorError) } else { self.f.start_program(addr, data) }
        }
        fn start_erase(&mut self, addr: u32) -> Result {
            self.left = self.busy;
            if self.dead { Err(crate::NorError) } else { self.f.start_erase(addr) }
        }
        fn busy(&mut self) -> Result<bool> {
            self.polls += 1;
            if self.left == 0 {
                return Ok(false);
            }
            self.left -= 1;
            Ok(true)
        }
        async fn pause() {}
    }

    /// BACKUP-7: an erase or program is answered only once the chip is done; a chip that fails answers `busy` to
    /// START (no calibration CRC to give) and `refused` to each request, and nothing panics.
    #[test]
    fn restore_waits_for_the_chip_and_reports_a_failed_one() {
        let mut out = [0u8; 240];
        let mut chip = Slow { f: SimFlash::new(), busy: 3, left: 0, polls: 0, dead: false };
        let at = SETTINGS.start;
        let (n, _) = block(serve(&mut chip, 4 << 20, &WRITABLE, UID, &req(r::ERASE, at, &[]), &mut out));
        assert_eq!((n, out[0], chip.polls), (1, Status::Ok as u8, 4), "three polls busy, the fourth done");
        block(serve(&mut chip, 4 << 20, &WRITABLE, UID, &req(r::PROGRAM, at, &[0x42; r::CHUNK]), &mut out));
        assert_eq!((out[0], chip.f.mem[at as usize], chip.polls), (Status::Ok as u8, 0x42, 8));
        chip.dead = true;
        let n = block(start(&mut chip, 4 << 20, &WRITABLE, UID, &mut out));
        assert_eq!((n, out[0]), (1, Status::Busy as u8));
        for p in [req(r::ERASE, at, &[]), req(r::PROGRAM, at, &[0; r::CHUNK]), req(r::CRC, at, &16u32.to_le_bytes())] {
            let (n, after) = block(serve(&mut chip, 4 << 20, &WRITABLE, UID, &p, &mut out));
            assert_eq!((n, out[0], after), (1, Status::Refused as u8, After::Next), "request {}", p[0]);
        }
    }
}
