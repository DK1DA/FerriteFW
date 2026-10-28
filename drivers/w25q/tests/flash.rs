//! The driver against a simulated W25Q32 (4 MB): reads, page-split programs, erases, protection.

use std::cell::RefCell;
use std::rc::Rc;
use uradio_bus::mock::{MockDelay, MockSpi};
#[cfg(feature = "write")]
use w25q::SECTOR;
use w25q::{Error, W25q};

/// A W25Q32 model: memory, write-enable latch, a busy countdown, and the commands the driver sends.
struct Chip {
    mem: Vec<u8>,
    wel: bool,
    busy_reads: u32,
    /// Busy for ever (a dead chip).
    stuck: bool,
}

fn device(chip: Rc<RefCell<Chip>>) -> impl FnMut(&[u8], u8) -> u8 {
    move |sent, byte| {
        let mut c = chip.borrow_mut();
        let pos = sent.len();
        let op = sent.first().copied().unwrap_or(byte);
        let addr = |s: &[u8]| (s[1] as usize) << 16 | (s[2] as usize) << 8 | s[3] as usize;
        match op {
            0x9F => [0, 0xEF, 0x40, 0x16].get(pos).copied().unwrap_or(0),
            0x4B if pos >= 5 => (pos - 5) as u8 + 0xA0,
            0x05 if pos == 1 => {
                if c.stuck {
                    1
                } else if c.busy_reads > 0 {
                    c.busy_reads -= 1;
                    1
                } else {
                    0
                }
            }
            0x06 => {
                c.wel = true;
                0
            }
            0x03 if pos >= 4 => c.mem[addr(sent) + pos - 4],
            0x02 if pos >= 4 => {
                assert!(c.wel, "program without write enable");
                let a = addr(sent);
                // Within a page the address wraps; the driver must never rely on that.
                let page = a & !255;
                let off = (a + pos - 4 - page) % 256;
                assert!(a + pos - 4 < page + 256, "program crossed a page boundary");
                c.mem[page + off] &= byte;
                c.busy_reads = 2;
                0
            }
            0x20 if pos == 3 => {
                assert!(c.wel, "erase without write enable");
                let mut s = sent.to_vec();
                s.push(byte);
                let a = addr(&s);
                c.mem[a..a + 4096].fill(0xFF);
                c.busy_reads = 5;
                c.wel = false;
                0
            }
            _ => 0,
        }
    }
}

const WRITABLE: &[std::ops::Range<u32>] = &[0x02C000..0x090000, 0x200000..0x300000];

#[allow(clippy::type_complexity)] // the mock device is a closure; naming it is not possible here
fn setup() -> (W25q<MockSpi<impl FnMut(&[u8], u8) -> u8>>, Rc<RefCell<Chip>>, MockDelay) {
    let chip = Rc::new(RefCell::new(Chip { mem: vec![0xFF; 4 << 20], wel: false, busy_reads: 0, stuck: false }));
    let mut delay = MockDelay::default();
    let f = W25q::new(MockSpi::new(device(chip.clone())), &mut delay, WRITABLE).unwrap();
    (f, chip, delay)
}

#[test]
fn identifies_and_sizes() {
    let (mut f, _, _) = setup();
    assert_eq!(f.jedec_id(), 0xEF4016);
    assert_eq!(f.size(), 4 << 20);
    assert_eq!(f.unique_id(), [0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7]);
}

#[test]
#[cfg(feature = "write")]
fn program_splits_at_pages_and_reads_back() {
    let (mut f, chip, mut delay) = setup();
    let data: Vec<u8> = (0..700).map(|i| (i * 7) as u8).collect();
    // Starts 100 bytes before a page boundary: pieces of 100, 256, 256, 88.
    f.program(0x02D000 - 100, &data, &mut delay).unwrap();
    let mut back = vec![0; 700];
    f.read(0x02D000 - 100, &mut back).unwrap();
    assert_eq!(back, data);
    assert!(chip.borrow().mem[0x02D000 - 101] == 0xFF);
    let progs = f.release().txns.iter().filter(|t| t.out[0] == 0x02).count();
    assert_eq!(progs, 4);
}

#[test]
#[cfg(feature = "write")]
fn erase_without_blocking() {
    let (mut f, chip, mut delay) = setup();
    f.program(0x02F000, &[0, 1, 2], &mut delay).unwrap();
    f.start_erase(0x02F000).unwrap();
    let mut polls = 0;
    while f.busy() {
        polls += 1;
    }
    assert_eq!(polls, 5);
    assert!(chip.borrow().mem[0x02F000..0x02F000 + SECTOR as usize].iter().all(|&b| b == 0xFF));
}

#[test]
#[cfg(feature = "write")]
fn refuses_protected_and_bad_addresses() {
    let (mut f, chip, mut delay) = setup();
    // Factory calibration (0x00F000), stock data (0x000000), the bootloader area (0x300000): untouched.
    assert_eq!(f.start_erase(0x00F000), Err(Error::Protected));
    assert_eq!(f.program(0x000100, &[0], &mut delay), Err(Error::Protected));
    assert_eq!(f.start_erase(0x300000), Err(Error::Protected));
    // Straddling the end of a writable range is refused too.
    assert_eq!(f.program(0x08FFFF, &[0, 0], &mut delay), Err(Error::Protected));
    assert_eq!(f.start_erase(0x02C001), Err(Error::Unaligned));
    assert_eq!(f.read(0x3FFFFF, &mut [0, 0]), Err(Error::OutOfRange));
    assert_eq!(f.read(u32::MAX, &mut [0]), Err(Error::OutOfRange));
    assert!(chip.borrow().mem.iter().all(|&b| b == 0xFF), "nothing was written");
    // And no program or erase opcode ever reached the bus.
    assert!(!f.release().txns.iter().any(|t| matches!(t.out[0], 0x02 | 0x20 | 0x06)));
}

#[test]
#[cfg(feature = "write")]
fn stuck_busy_times_out() {
    let (mut f, chip, mut delay) = setup();
    chip.borrow_mut().stuck = true;
    assert_eq!(f.erase(0x02C000, &mut delay), Err(Error::Timeout));
    assert!(delay.total_us >= 400_000, "waited its full limit");
}

/// The read-only build (no `write` feature, the reader image's, BACKUP-9.5): it says so, and reading the whole chip,
/// its IDs and its status sends nothing but read opcodes (read 0x03, JEDEC 0x9F, unique ID 0x4B, status 0x05, wake 0xAB).
#[test]
fn read_only_build_sends_only_reads() {
    let (mut f, chip, _) = setup();
    assert_eq!(w25q::WRITES, cfg!(feature = "write"));
    chip.borrow_mut().mem[0x00F000] = 0x5A;
    let mut b = vec![0; 4096];
    for a in (0..4u32 << 20).step_by(4096) {
        f.read(a, &mut b).unwrap();
    }
    f.read(0x00F000, &mut b).unwrap();
    assert_eq!(b[0], 0x5A);
    f.jedec_id();
    f.unique_id();
    f.status();
    assert_eq!(f.read(0x3FFFFF, &mut [0, 0]), Err(Error::OutOfRange));
    assert!(f.release().txns.iter().all(|t| matches!(t.out[0], 0x03 | 0x9F | 0x4B | 0x05 | 0xAB)));
}

/// Start-up wakes the chip (ABh alone, then at least tRES1 = 3 us) and reads the JEDEC ID; a bus with no chip
/// (all ones or all zeros) or a capacity byte out of 0x10..=0x19 is refused.
#[test]
fn start_up_wakes_then_refuses_unknown_chips() {
    for (id, ok) in [([0xFF, 0xFF, 0xFF], None), ([0, 0, 0], None), ([0xEF, 0x40, 0x0F], None), ([0xEF, 0x40, 0x1A], None), ([0xC8, 0x40, 0x10], Some(64 << 10)), ([0xC8, 0x40, 0x19], Some(32 << 20))] {
        let mut delay = MockDelay::default();
        let spi = MockSpi::new(move |sent: &[u8], _| match sent.first() {
            Some(0x9F) => id[sent.len() - 1],
            _ => 0,
        });
        match W25q::new(spi, &mut delay, WRITABLE) {
            Ok(f) => {
                assert_eq!(Some(f.size()), ok, "{id:02X?}");
                let t = f.release().txns;
                assert_eq!(t.len(), 2);
                assert_eq!((t[0].out.as_slice(), t[1].out.as_slice()), (&[0xAB][..], &[0x9F, 0, 0, 0][..]));
            }
            Err(e) => assert!(e == Error::UnknownChip && ok.is_none(), "{id:02X?}"),
        }
        assert!(delay.total_us >= 3);
    }
}

/// Power-down is B9h alone; wake is ABh alone, then a wait of at least tRES1 (3 us).
#[test]
fn power_down_and_wake() {
    let (mut f, _, _) = setup();
    let mut delay = MockDelay::default();
    f.power_down();
    f.wake(&mut delay);
    assert!(delay.total_us >= 3);
    let t = f.release().txns;
    let n = t.len();
    assert_eq!((t[n - 2].out.as_slice(), t[n - 1].out.as_slice()), (&[0xB9][..], &[0xAB][..]));
}

#[test]
#[cfg(feature = "write")]
fn program_and_erase_past_the_end_or_stuck() {
    let (mut f, chip, mut delay) = setup();
    // Past the chip's end: out of range before the writable ranges are looked at.
    assert_eq!(f.program(0x3FFFFF, &[0, 0], &mut delay), Err(Error::OutOfRange));
    assert_eq!(f.erase(0x400000, &mut delay), Err(Error::OutOfRange));
    assert_eq!(f.erase(0x000000, &mut delay), Err(Error::Protected));
    assert_eq!(delay.total_us, 50); // only the start-up wake's
    // A chip that stays busy after a page program: given up after 3 ms (tPP max), the rest not sent.
    chip.borrow_mut().stuck = true;
    assert_eq!(f.program(0x02C0F0, &[0; 32], &mut delay), Err(Error::Timeout));
    assert_eq!(delay.total_us, 50 + 3_000);
    assert_eq!(f.release().txns.iter().filter(|t| t.out[0] == 0x02).count(), 1);
}

/// A blocking erase: write enable, 20h with the address, then status polls until the chip is idle.
#[test]
#[cfg(feature = "write")]
fn blocking_erase_sends_enable_erase_then_polls() {
    let (mut f, chip, mut delay) = setup();
    chip.borrow_mut().mem[0x200000] = 0;
    f.erase(0x200000, &mut delay).unwrap();
    assert_eq!(chip.borrow().mem[0x200000], 0xFF);
    assert_eq!(delay.total_us, 50 + 5 * 50);
    let t = f.release().txns;
    let ops: Vec<&[u8]> = t[2..].iter().map(|t| t.out.as_slice()).collect();
    assert_eq!(&ops[..2], &[&[0x06][..], &[0x20, 0x20, 0x00, 0x00][..]]);
    assert_eq!(ops.len(), 2 + 6);
    assert!(ops[2..].iter().all(|o| *o == [0x05, 0]));
}
