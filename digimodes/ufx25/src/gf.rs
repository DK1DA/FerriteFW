//! GF(2^8) arithmetic with the field polynomial x^8 + x^4 + x^3 + x^2 + 1 (0x11D), α = 2. Tables are built at
//! compile time and live in flash (768 bytes).

/// The field polynomial.
pub const POLY: u16 = 0x11D;

/// α^i for i in 0..510 (doubled so a sum of two logs needs no reduction).
pub static EXP: [u8; 512] = build_exp();
/// log_α(x) for x in 1..=255; `LOG[0]` is unused (0).
pub static LOG: [u8; 256] = build_log();

const fn build_exp() -> [u8; 512] {
    let mut t = [0u8; 512];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < 255 {
        t[i] = x as u8;
        t[i + 255] = x as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= POLY;
        }
        i += 1;
    }
    t[510] = t[0];
    t[511] = t[1];
    t
}

const fn build_log() -> [u8; 256] {
    let exp = build_exp();
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 255 {
        t[exp[i] as usize] = i as u8;
        i += 1;
    }
    t
}

/// a · b.
#[inline]
pub fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        EXP[LOG[a as usize] as usize + LOG[b as usize] as usize]
    }
}

/// a · α^e (e in 0..255).
#[inline]
pub fn mul_exp(a: u8, e: usize) -> u8 {
    if a == 0 {
        0
    } else {
        EXP[LOG[a as usize] as usize + e]
    }
}

/// a / b, b not 0.
#[inline]
pub fn div(a: u8, b: u8) -> u8 {
    if a == 0 {
        0
    } else {
        EXP[LOG[a as usize] as usize + 255 - LOG[b as usize] as usize]
    }
}

/// α^e for any e.
#[inline]
pub fn pow(e: usize) -> u8 {
    EXP[e % 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field() {
        // α generates the whole multiplicative group.
        let mut seen = [false; 256];
        for i in 0..255 {
            assert!(!seen[EXP[i] as usize]);
            seen[EXP[i] as usize] = true;
        }
        for a in 1..=255u8 {
            for b in 1..=255u8 {
                assert_eq!(div(mul(a, b), b), a);
            }
        }
    }
}
