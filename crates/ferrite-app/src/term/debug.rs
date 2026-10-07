//! The terminal's debug commands (manual chapter 37, TERM-21 to TERM-24): MCU memory, the GPIO ports, BK4829
//! registers and SI4732 commands, for bring-up without a custom build each time. A command becomes a [`Job`]:
//! the firmware takes its requests one at a time ([`crate::term::Term::debug_req`]), answers them
//! ([`crate::term::Term::debug_answer`]), and the console prints the answers as lines. Memory is read by the
//! firmware where the terminal runs; registers by the Radio task, which owns the chips and refuses writes that
//! could key the transmitter (ferrite_core::msg::bk_write_refused).

use super::cmd::Line;
use ferrite_core::msg::{DebugAns, DebugReq, DEBUG_MEM};

/// GPIO port A's input data register; ports B to E follow every 0x400, the output register 4 bytes after.
pub const GPIOA_IDR: u32 = 0x4001_0808;
/// Ports A to E.
pub const GPIO_PORTS: u16 = 5;
/// BK4829 registers 0x00 to 0x7F.
pub const BK_REGS: u16 = 0x80;
/// The longest `mem` (bytes) and `mem32` (words) read.
pub const MEM_MAX: u16 = 1024;
/// A request not answered in this long ends the job (a chip that never answers, a build without the path).
pub const ANSWER_MS: u32 = 2000;

/// What a debug command asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `mem ADDR [LEN]` bytes, `mem32 ADDR [WORDS]` 32-bit words (`len` in bytes).
    Mem { addr: u32, len: u16, words: bool },
    /// `gpio`: each port's input and output registers.
    Gpio,
    /// `bk CHIP REG`.
    BkRead { chip: u8, reg: u8 },
    /// `bk CHIP REG VALUE`.
    BkWrite { chip: u8, reg: u8, val: u16 },
    /// `bk CHIP dump`: 0x00 to 0x7F.
    BkDump { chip: u8 },
    /// `si BYTES...`, `si prop P [V]`: a command and how many bytes come back.
    Si { cmd: [u8; 8], n: u8, reply: u8 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    /// The next request is ready for the firmware.
    Ask,
    /// Asked at this time (ms); no answer yet.
    Wait(u32),
    /// An answer to print.
    Have,
    Over,
}

/// What [`Job::line`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// A line to print is in the buffer.
    Line,
    /// Waiting for the firmware.
    Wait,
    Over,
}

/// A debug command in progress.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Job {
    kind: Kind,
    /// Bytes, ports or registers done.
    done: u16,
    st: St,
    ans: Option<DebugAns>,
    /// Lines printed from the current answer.
    line: u8,
    /// `bk dump`: the current row of eight registers.
    acc: [u16; 8],
}

impl Job {
    pub fn new(kind: Kind) -> Self {
        Job { kind, done: 0, st: St::Ask, ans: None, line: 0, acc: [0; 8] }
    }

    /// The next request for the firmware, once; `now` starts its answer time.
    pub fn request(&mut self, now: u32) -> Option<DebugReq> {
        if self.st != St::Ask {
            return None;
        }
        self.st = St::Wait(now);
        Some(match self.kind {
            Kind::Mem { addr, len, words } => {
                let n = (len - self.done).min(DEBUG_MEM as u16) as u8;
                DebugReq::Mem { addr: addr + self.done as u32, n, words }
            }
            Kind::Gpio => DebugReq::Mem { addr: GPIOA_IDR + self.done as u32 * 0x400, n: 8, words: true },
            Kind::BkRead { chip, reg } => DebugReq::BkRead { chip, reg },
            Kind::BkWrite { chip, reg, val } => DebugReq::BkWrite { chip, reg, val },
            Kind::BkDump { chip } => DebugReq::BkRead { chip, reg: self.done as u8 },
            Kind::Si { cmd, n, reply } => DebugReq::Si { cmd, n, reply },
        })
    }

    /// The firmware's answer to the last request.
    pub fn answer(&mut self, a: DebugAns) {
        if let St::Wait(_) = self.st {
            (self.ans, self.line, self.st) = (Some(a), 0, St::Have);
        }
    }

    /// The next line into `out`, if there is one now.
    pub fn line(&mut self, now: u32, out: &mut Line) -> Step {
        out.clear();
        match self.st {
            St::Over => return Step::Over,
            St::Ask => return Step::Wait,
            St::Wait(at) if now.wrapping_sub(at) >= ANSWER_MS => {
                self.st = St::Over;
                out.push(b"  no answer (the firmware didn't reply in 2 s)");
                return Step::Line;
            }
            St::Wait(_) => return Step::Wait,
            St::Have => {}
        }
        let Some(a) = self.ans else { return Step::Over };
        match (self.kind, a) {
            (_, DebugAns::Refused(why)) => {
                out.push(b"  refused: ");
                out.push(why.as_bytes());
                self.st = St::Over;
                Step::Line
            }
            (Kind::Mem { len, words, .. }, DebugAns::Mem { addr, data, n }) => {
                let k = self.line as usize * 16;
                if k < n as usize {
                    mem_line(addr + k as u32, &data[k..(k + 16).min(n as usize)], words, out);
                    self.line += 1;
                    return Step::Line;
                }
                self.done += n as u16;
                self.st = if self.done < len && n > 0 { St::Ask } else { St::Over };
                self.line(now, out)
            }
            (Kind::Gpio, DebugAns::Mem { data, .. }) => {
                let word = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
                let (idr, odr) = (word(0) as u16, word(4) as u16);
                put!(out, "  P{}  in ", ["A", "B", "C", "D", "E"][self.done as usize % 5]);
                bits16(idr, out);
                out.push(b"   out ");
                bits16(odr, out);
                self.done += 1;
                self.st = if self.done < GPIO_PORTS { St::Ask } else { St::Over };
                Step::Line
            }
            (Kind::BkRead { .. }, DebugAns::Reg { chip, reg, val }) => {
                put!(out, "  bk{} REG_", chip);
                hex(reg as u32, 2, out);
                out.push(b" = 0x");
                hex(val as u32, 4, out);
                out.push(b"   ");
                bits16(val, out);
                self.st = St::Over;
                Step::Line
            }
            (Kind::BkWrite { val, .. }, DebugAns::Reg { chip, reg, val: back }) => {
                put!(out, "  bk{} REG_", chip);
                hex(reg as u32, 2, out);
                out.push(b" <- 0x");
                hex(val as u32, 4, out);
                out.push(b", reads back 0x");
                hex(back as u32, 4, out);
                if back != val {
                    out.push(b" (differs: read-only or self-clearing bits)");
                }
                self.st = St::Over;
                Step::Line
            }
            (Kind::BkDump { chip }, DebugAns::Reg { val, .. }) => {
                self.acc[self.done as usize % 8] = val;
                self.done += 1;
                self.st = if self.done < BK_REGS { St::Ask } else { St::Over };
                if !self.done.is_multiple_of(8) {
                    return Step::Wait;
                }
                put!(out, "  bk{} ", chip);
                hex(self.done as u32 - 8, 2, out);
                out.push(b":");
                for v in self.acc {
                    out.push(b" ");
                    hex(v as u32, 4, out);
                }
                Step::Line
            }
            (Kind::Si { cmd, n: cn, .. }, DebugAns::Si { data, n }) => {
                out.push(b"  si");
                for b in &cmd[..cn as usize] {
                    out.push(b" ");
                    hex(*b as u32, 2, out);
                }
                out.push(b" ->");
                for b in &data[..n as usize] {
                    out.push(b" ");
                    hex(*b as u32, 2, out);
                }
                self.st = St::Over;
                Step::Line
            }
            _ => {
                out.push(b"  unexpected answer");
                self.st = St::Over;
                Step::Line
            }
        }
    }
}

/// `digits` hex digits of v, capitals.
pub fn hex(v: u32, digits: u32, out: &mut Line) {
    for i in (0..digits).rev() {
        out.push(&[b"0123456789ABCDEF"[(v >> (i * 4)) as usize & 15]]);
    }
}

/// 16 bits in four groups, bit 15 first: "0001 0010 0011 0100".
fn bits16(v: u16, out: &mut Line) {
    for i in (0..16).rev() {
        out.push(if v >> i & 1 != 0 { b"1" } else { b"0" });
        if i % 4 == 0 && i != 0 {
            out.push(b" ");
        }
    }
}

/// One dump line: the address, then 16 bytes and their characters, or 4 words.
fn mem_line(addr: u32, d: &[u8], words: bool, out: &mut Line) {
    out.push(b"  ");
    hex(addr, 8, out);
    out.push(b" ");
    if words {
        for w in d.chunks(4) {
            let mut b = [0u8; 4];
            b[..w.len()].copy_from_slice(w);
            out.push(b" ");
            hex(u32::from_le_bytes(b), 8, out);
        }
        return;
    }
    for b in d {
        out.push(b" ");
        hex(*b as u32, 2, out);
    }
    for _ in d.len()..16 {
        out.push(b"   ");
    }
    out.push(b"  |");
    for &b in d {
        out.push(&[if (0x20..0x7F).contains(&b) { b } else { b'.' }]);
    }
    out.push(b"|");
}
