//! RDS (Radio Data System) from the four 16-bit blocks of each group: programme identification, station
//! name (PS, group 0) and radio text (group 2).

#[derive(Clone, Copy)]
pub struct Rds {
    pub pi: u16,
    pub ps: [u8; 8],  // space padded
    pub rt: [u8; 64], // NUL ends it, like the C string it mirrors
    pub groups: u32,  // groups accepted
    pub ps_seen: u8,  // bit per 2-character segment received
    pub rt_ab: u8,    // text A/B flag: a change means new text
}

fn clean(c: u8) -> u8 {
    if (0x20..0x7F).contains(&c) { c } else { b' ' }
}

impl Rds {
    /// All zero, as the status starts; `init()` is the state after a retune.
    pub const fn zeroed() -> Self {
        Rds { pi: 0, ps: [0; 8], rt: [0; 64], groups: 0, ps_seen: 0, rt_ab: 0 }
    }

    pub const fn init() -> Self {
        Rds { ps: [b' '; 8], rt_ab: 0xFF, ..Self::zeroed() }
    }

    /// The radio text up to its end (0x0D, stored as NUL).
    pub fn text(&self) -> &[u8] {
        cstr(&self.rt)
    }

    pub fn ps_complete(&self) -> bool {
        self.ps_seen == 0x0F
    }

    /// ble = block error levels from the receiver, 2 bits per block (A in bits 7:6 ... D in 1:0); blocks with
    /// uncorrectable errors (level 3) are ignored.
    pub fn feed(&mut self, a: u16, b: u16, c: u16, d: u16, ble: u8) {
        let (ea, eb, ec, ed) = (ble >> 6 & 3, ble >> 4 & 3, ble >> 2 & 3, ble & 3);
        if eb == 3 {
            return; // no group type without block B
        }
        if ea != 3 {
            self.pi = a;
        }
        self.groups += 1;
        let version_b = b >> 11 & 1 != 0;
        match b >> 12 {
            0 if ed != 3 => {
                // 0A/0B: two PS characters in block D
                let seg = (b & 3) as usize;
                self.ps[seg * 2] = clean((d >> 8) as u8);
                self.ps[seg * 2 + 1] = clean(d as u8);
                self.ps_seen |= 1 << seg;
            }
            2 => {
                // 2A: 4 characters in C and D; 2B: 2 in D
                let ab = (b >> 4 & 1) as u8;
                if ab != self.rt_ab {
                    self.rt = [0; 64];
                    self.rt_ab = ab;
                }
                let seg = (b & 0xF) as usize;
                if !version_b {
                    if ec == 3 || ed == 3 {
                        return;
                    }
                    self.rt[seg * 4..seg * 4 + 4].copy_from_slice(&[(c >> 8) as u8, c as u8, (d >> 8) as u8, d as u8]);
                } else {
                    if ed == 3 {
                        return;
                    }
                    self.rt[seg * 2..seg * 2 + 2].copy_from_slice(&[(d >> 8) as u8, d as u8]);
                }
                // 0x0D ends the text; everything else unprintable becomes a space
                for ch in self.rt.iter_mut() {
                    if *ch == 0x0D {
                        *ch = 0;
                        break;
                    }
                    if *ch != 0 && !(0x20..=0x7E).contains(ch) {
                        *ch = b' ';
                    }
                }
            }
            _ => {}
        }
    }
}

/// Up to the first NUL.
pub fn cstr(b: &[u8]) -> &[u8] {
    &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]
}

