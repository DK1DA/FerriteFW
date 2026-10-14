//! The bit-banged bus against a simulated BK4829 at the pin level: writes land in its registers, reads come back
//! with either read alignment once detected, and the chip select picks the chip.

use bk4829::wire::{Align, ThreeWire, ID};
use bk4829::Bus;
use std::cell::RefCell;
use std::rc::Rc;
use uradio_bus::{Delay, InPin, IoPin, OutPin};

/// Two chips on shared SCK and SDATA.
struct Sim {
    sck: bool,
    mcu_drives: bool,
    mcu_sda: bool,
    chips: [Chip; 2],
}

struct Chip {
    selected: bool,
    align: Align,
    regs: [u16; 128],
    /// Bits clocked in this select.
    bits: u32,
    shift: u32,
    /// Driving a read: the value and how many bits have gone out.
    out: Option<(u16, u32)>,
}

impl Chip {
    fn new(align: Align) -> Self {
        let mut regs = [0u16; 128];
        regs[0] = ID;
        Chip { selected: false, align, regs, bits: 0, shift: 0, out: None }
    }
    fn sda(&self) -> Option<bool> {
        let (v, n) = self.out?;
        (n < 16).then(|| v >> (15 - n) & 1 != 0)
    }
}

impl Sim {
    fn edge(&mut self, rising: bool) {
        let sda = self.mcu_sda;
        for c in self.chips.iter_mut().filter(|c| c.selected) {
            if let Some((v, n)) = c.out {
                // Reading: the chip moves to the next bit on each falling edge (BeforeFall: bit 15 already out after
                // the address; AfterFall: bit 15 comes out on the first fall).
                if !rising {
                    c.out = Some((v, n.wrapping_add(1))); // AfterFall starts at u32::MAX: bit 15 on the first fall
                }
                continue;
            }
            if !rising {
                continue;
            }
            assert!(self.mcu_drives, "chip latched a bit while the MCU released SDATA");
            c.shift = c.shift << 1 | sda as u32;
            c.bits += 1;
            if c.bits == 8 && c.shift & 0x80 != 0 {
                let v = c.regs[(c.shift & 0x7F) as usize];
                c.out = Some((v, if c.align == Align::BeforeFall { 0 } else { u32::MAX }));
            }
            if c.bits == 24 {
                c.regs[(c.shift >> 16 & 0x7F) as usize] = c.shift as u16;
            }
        }
    }
    fn sda(&self) -> bool {
        let driven: Vec<bool> = self.chips.iter().filter(|c| c.selected).filter_map(|c| c.sda()).collect();
        if self.mcu_drives {
            assert!(driven.is_empty(), "bus fight");
            return self.mcu_sda;
        }
        driven.first().copied().unwrap_or(true) // pull-up
    }
}

type S = Rc<RefCell<Sim>>;
struct Sck(S);
struct Sda(S);
struct Cs(S, usize);
struct NoDelay;

impl OutPin for Sck {
    fn set(&mut self, high: bool) {
        let mut s = self.0.borrow_mut();
        if s.sck != high {
            s.sck = high;
            s.edge(high);
        }
    }
}
impl OutPin for Sda {
    fn set(&mut self, high: bool) {
        self.0.borrow_mut().mcu_sda = high;
    }
}
impl InPin for Sda {
    fn is_high(&mut self) -> bool {
        self.0.borrow().sda()
    }
}
impl IoPin for Sda {
    fn drive(&mut self, on: bool) {
        self.0.borrow_mut().mcu_drives = on;
    }
}
impl OutPin for Cs {
    fn set(&mut self, high: bool) {
        let mut s = self.0.borrow_mut();
        let c = &mut s.chips[self.1];
        c.selected = !high;
        c.bits = 0;
        c.shift = 0;
        c.out = None;
    }
}
impl Delay for NoDelay {
    fn delay_us(&mut self, _: u32) {}
}

fn bus(align: Align) -> (S, ThreeWire<Sck, Sda, NoDelay>, Cs, Cs) {
    let s = Rc::new(RefCell::new(Sim {
        sck: false,
        mcu_drives: false,
        mcu_sda: false,
        chips: [Chip::new(align), Chip::new(align)],
    }));
    let w = ThreeWire::new(Sck(s.clone()), Sda(s.clone()), NoDelay);
    let (c0, c1) = (Cs(s.clone(), 0), Cs(s.clone(), 1));
    (s, w, c0, c1)
}

#[test]
fn both_alignments_detected_and_read() {
    for align in [Align::BeforeFall, Align::AfterFall] {
        let (sim, mut w, mut c0, mut c1) = bus(align);
        c0.set(true);
        c1.set(true);
        assert_eq!(w.detect(&mut c0), Some(align));
        w.write(&mut c0, 0x38, 0xA5C3);
        w.chip(&mut c1).write(0x38, 0x1234);
        assert_eq!(sim.borrow().chips[0].regs[0x38], 0xA5C3);
        assert_eq!(sim.borrow().chips[1].regs[0x38], 0x1234);
        assert_eq!(w.read(&mut c0, 0x38), 0xA5C3);
        assert_eq!(w.chip(&mut c1).read(0x38), 0x1234);
        assert_eq!(w.read(&mut c1, 0x00), ID);
        // Every register value comes back whole: walking ones.
        for b in 0..16 {
            w.write(&mut c0, 0x40, 1 << b);
            assert_eq!(w.read(&mut c0, 0x40), 1 << b, "{align:?} bit {b}");
        }
    }
}

#[test]
fn nobody_there() {
    let (sim, mut w, mut c0, _) = bus(Align::BeforeFall);
    sim.borrow_mut().chips[0].regs[0] = 0xFFFF; // a released, pulled-up line
    assert_eq!(w.detect(&mut c0), None);
    assert_eq!(w.align(), Align::BeforeFall);
}
