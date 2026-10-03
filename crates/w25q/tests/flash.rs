//! The driver against a simulated W25Q32 (4 MB): reads, page-split programs, erases, protection.

use std::cell::RefCell;
use std::rc::Rc;
use uradio_bus::mock::{MockDelay, MockSpi};
use w25q::{Error, W25q, SECTOR};

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
fn stuck_busy_times_out() {
    let (mut f, chip, mut delay) = setup();
    chip.borrow_mut().stuck = true;
    assert_eq!(f.erase(0x02C000, &mut delay), Err(Error::Timeout));
    assert!(delay.total_us >= 400_000, "waited its full limit");
}
