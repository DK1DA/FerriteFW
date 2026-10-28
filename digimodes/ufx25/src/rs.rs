//! Reed-Solomon codes over GF(2^8) as FX.25 uses them: 16, 32 or 64 check bytes, generator roots α^1 … α^r
//! (first consecutive root 1, primitive element α = 2, field polynomial 0x11D), the block's first byte the
//! highest-degree coefficient and the check bytes last.
//!
//! **Shortening.** A block of `n` < 255 bytes is the full 255-byte codeword with `255 - n` zero information bytes
//! *between* the data and the check bytes (not in front, as textbook shortening does). The FX.25 specification
//! does not say where the zeros go; this is what Dire Wolf transmits and expects (measured: its shortened blocks
//! have zero syndromes only in this layout; see the crate README).
//!
//! The decoder is Berlekamp-Massey, Chien search and Forney. It corrects up to r/2 wrong bytes and reports how
//! many it fixed; a block with more errors is rejected (or, rarely, miscorrected into another codeword: the AX.25
//! FCS inside catches that). Everything lives on the stack: about 0.5 KB at most while decoding, nothing between
//! calls.

use crate::gf::{self, EXP, LOG};

/// Largest number of check bytes.
pub const MAX_ROOTS: usize = 64;

/// Generator polynomial with roots α^1 … α^R, coefficient of x^i at index i (monic: index R is 1).
const fn generator<const L: usize>() -> [u8; L] {
    let r = L - 1;
    let mut g = [0u8; L];
    g[0] = 1;
    let mut i = 1;
    while i <= r {
        // g = g·(x + α^i)
        let root = const_pow(i);
        let mut j = i;
        while j > 0 {
            g[j] = g[j - 1] ^ const_mul(g[j], root);
            j -= 1;
        }
        g[0] = const_mul(g[0], root);
        i += 1;
    }
    g
}

const fn const_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        let carry = a & 0x80 != 0;
        a <<= 1;
        if carry {
            a ^= (gf::POLY & 0xFF) as u8;
        }
        b >>= 1;
    }
    p
}

const fn const_pow(e: usize) -> u8 {
    let mut x = 1u8;
    let mut i = 0;
    while i < e {
        x = const_mul(x, 2);
        i += 1;
    }
    x
}

static GEN16: [u8; 17] = generator::<17>();
static GEN32: [u8; 33] = generator::<33>();
static GEN64: [u8; 65] = generator::<65>();

/// A block had more errors than the code can correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uncorrectable;

/// One of the three FX.25 codes: RS(255, 255 - r) and its shortened forms.
#[derive(Clone, Copy, Debug)]
pub struct Rs {
    gen: &'static [u8],
}

impl Rs {
    /// The code with `check` check bytes: 16, 32 or 64 (anything else panics).
    pub fn new(check: usize) -> Self {
        let gen: &'static [u8] = match check {
            16 => &GEN16,
            32 => &GEN32,
            64 => &GEN64,
            _ => panic!("FX.25 uses 16, 32 or 64 check bytes"),
        };
        Rs { gen }
    }

    /// Check bytes.
    pub fn check(&self) -> usize {
        self.gen.len() - 1
    }

    /// Fill `block[k..]` with check bytes for the data `block[..k]`; `block.len()` is k + check, at most 255.
    pub fn encode(&self, block: &mut [u8], k: usize) {
        let r = self.check();
        assert!(block.len() == k + r && block.len() <= 255);
        let gap = 255 - block.len();
        // rem[0] is the highest-degree coefficient of the running remainder.
        let mut rem = [0u8; MAX_ROOTS];
        let rem = &mut rem[..r];
        let mut feed = |d: u8| {
            let fb = d ^ rem[0];
            for i in 0..r - 1 {
                rem[i] = rem[i + 1] ^ gf::mul(fb, self.gen[r - 1 - i]);
            }
            rem[r - 1] = gf::mul(fb, self.gen[0]);
        };
        for i in 0..k {
            feed(block[i]);
        }
        for _ in 0..gap {
            feed(0);
        }
        block[k..].copy_from_slice(rem);
    }

    /// Correct `block` (data `block[..k]`, check bytes after) in place. Returns the number of bytes corrected.
    pub fn decode(&self, block: &mut [u8], k: usize) -> Result<usize, Uncorrectable> {
        let r = self.check();
        let n = block.len();
        assert!(n == k + r && n <= 255);
        let gap = 255 - n;

        // Syndromes S_i = c(α^(i+1)).
        let mut s = [0u8; MAX_ROOTS];
        let mut any = 0u8;
        for i in 0..r {
            let e = i + 1;
            let mut acc = 0u8;
            for &c in &block[..k] {
                acc = gf::mul_exp(acc, e) ^ c;
            }
            acc = gf::mul_exp(acc, (e * gap) % 255);
            for &c in &block[k..] {
                acc = gf::mul_exp(acc, e) ^ c;
            }
            s[i] = acc;
            any |= acc;
        }
        if any == 0 {
            return Ok(0);
        }

        // Berlekamp-Massey: the error locator Λ (lambda), degree l.
        let mut lambda = [0u8; MAX_ROOTS + 1];
        let mut prev = [0u8; MAX_ROOTS + 1];
        lambda[0] = 1;
        prev[0] = 1;
        let (mut l, mut m, mut b) = (0usize, 1usize, 1u8);
        for i in 0..r {
            let mut d = s[i];
            for j in 1..=l {
                d ^= gf::mul(lambda[j], s[i - j]);
            }
            if d == 0 {
                m += 1;
                continue;
            }
            let coef = gf::div(d, b);
            if 2 * l <= i {
                let saved = lambda;
                for j in m..=r {
                    lambda[j] ^= gf::mul(coef, prev[j - m]);
                }
                l = i + 1 - l;
                prev = saved;
                b = d;
                m = 1;
            } else {
                for j in m..=r {
                    lambda[j] ^= gf::mul(coef, prev[j - m]);
                }
                m += 1;
            }
        }
        if l == 0 || 2 * l > r {
            return Err(Uncorrectable);
        }

        // Ω(x) = S(x)·Λ(x) mod x^r (only degrees below l matter).
        let mut omega = [0u8; MAX_ROOTS / 2];
        for i in 0..l {
            let mut acc = 0u8;
            for j in 0..=i {
                acc ^= gf::mul(s[i - j], lambda[j]);
            }
            omega[i] = acc;
        }

        // Chien search over the real positions only; Forney for each root found.
        let mut found = 0usize;
        let mut fixes = [(0u8, 0u8); MAX_ROOTS / 2];
        for j in 0..n {
            // Degree of block[j] in the 255-byte codeword.
            let p = if j < k { 254 - j } else { n - 1 - j };
            let xinv = (255 - p) % 255; // log of X^-1
            let mut v = 0u8;
            for i in 0..=l {
                if lambda[i] != 0 {
                    v ^= EXP[LOG[lambda[i] as usize] as usize + (xinv * i) % 255];
                }
            }
            if v != 0 {
                continue;
            }
            if found == l {
                return Err(Uncorrectable);
            }
            // e = Ω(X^-1) / Λ'(X^-1) (first root α^1, so no X factor).
            let mut num = 0u8;
            for i in 0..l {
                num ^= gf::mul(omega[i], gf::pow(xinv * i));
            }
            let mut den = 0u8;
            let mut i = 1;
            while i <= l {
                den ^= gf::mul(lambda[i], gf::pow(xinv * (i - 1)));
                i += 2;
            }
            if den == 0 || num == 0 {
                return Err(Uncorrectable);
            }
            fixes[found] = (j as u8, gf::div(num, den));
            found += 1;
        }
        if found != l {
            return Err(Uncorrectable);
        }
        for &(j, e) in &fixes[..found] {
            block[j as usize] ^= e;
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift32
    struct Rng(u32);
    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u32) as usize
        }
    }

    fn syndromes_zero(rs: &Rs, block: &[u8], k: usize) -> bool {
        let mut b = [0u8; 255];
        b[..block.len()].copy_from_slice(block);
        rs.decode(&mut b[..block.len()], k) == Ok(0)
    }

    #[test]
    fn generator_roots() {
        for (g, r) in [(&GEN16[..], 16), (&GEN32[..], 32), (&GEN64[..], 64)] {
            assert_eq!(g[r], 1);
            for i in 1..=r {
                // g(α^i) = 0
                let mut v = 0u8;
                for (d, &c) in g.iter().enumerate() {
                    v ^= gf::mul(c, gf::pow(i * d));
                }
                assert_eq!(v, 0, "r {r} root {i}");
            }
        }
    }

    /// The compile-time generator builder, run again at run time, gives the flash tables; its own field arithmetic
    /// agrees with the table-driven one.
    #[test]
    fn const_builders_match_tables() {
        assert_eq!(generator::<17>(), GEN16);
        assert_eq!(generator::<33>(), GEN32);
        assert_eq!(generator::<65>(), GEN64);
        // (x + α)(x + α^2) = x^2 + (α + α^2) x + α^3 = x^2 + 6x + 8.
        assert_eq!(generator::<3>(), [8, 6, 1]);
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                assert_eq!(const_mul(a, b), gf::mul(a, b), "{a} * {b}");
            }
        }
        for e in 0..600 {
            assert_eq!(const_pow(e), gf::pow(e), "α^{e}");
        }
    }

    #[test]
    fn code_sizes() {
        assert_eq!(Rs::new(16).check(), 16);
        assert_eq!(Rs::new(32).check(), 32);
        assert_eq!(Rs::new(64).check(), 64);
    }

    #[test]
    #[should_panic(expected = "16, 32 or 64")]
    fn other_sizes_panic() {
        Rs::new(8);
    }

    #[test]
    fn random_errors() {
        let mut rng = Rng(0x1234_5678);
        let sizes = [(255, 239), (144, 128), (80, 64), (48, 32), (255, 223), (160, 128), (96, 64), (64, 32), (255, 191), (192, 128), (128, 64)];
        for &(n, k) in &sizes {
            let rs = Rs::new(n - k);
            let t = (n - k) / 2;
            for trial in 0..200 {
                let mut block = [0u8; 255];
                for b in &mut block[..k] {
                    *b = rng.next() as u8;
                }
                rs.encode(&mut block[..n], k);
                assert!(syndromes_zero(&rs, &block[..n], k));
                let clean = block;
                // Up to t errors at distinct places: always corrected.
                let nerr = if trial == 0 { 0 } else { 1 + rng.below(t) };
                let mut hit = [false; 255];
                let mut placed = 0;
                while placed < nerr {
                    let j = rng.below(n);
                    if hit[j] {
                        continue;
                    }
                    hit[j] = true;
                    block[j] ^= 1 + rng.below(255) as u8;
                    placed += 1;
                }
                assert_eq!(rs.decode(&mut block[..n], k), Ok(nerr), "n {n} k {k} errors {nerr}");
                assert_eq!(&block[..n], &clean[..n]);
            }
        }
    }

    #[test]
    fn beyond_t() {
        // More than t errors: rejected, or (rarely) "corrected" into a different codeword; never the original.
        let mut rng = Rng(0xDEAD_BEEF);
        let (mut rejected, mut wrong) = (0, 0);
        for &(n, k) in &[(255usize, 239usize), (48, 32), (96, 64), (128, 64)] {
            let rs = Rs::new(n - k);
            let t = (n - k) / 2;
            for _ in 0..300 {
                let mut block = [0u8; 255];
                for b in &mut block[..k] {
                    *b = rng.next() as u8;
                }
                rs.encode(&mut block[..n], k);
                let clean = block;
                let nerr = t + 1 + rng.below(t);
                let mut hit = [false; 255];
                let mut placed = 0;
                while placed < nerr {
                    let j = rng.below(n);
                    if !hit[j] {
                        hit[j] = true;
                        block[j] ^= 1 + rng.below(255) as u8;
                        placed += 1;
                    }
                }
                match rs.decode(&mut block[..n], k) {
                    Err(Uncorrectable) => rejected += 1,
                    Ok(c) => {
                        assert!(c <= t);
                        assert_ne!(&block[..n], &clean[..n]);
                        assert!(syndromes_zero(&rs, &block[..n], k), "a miscorrection must still be a codeword");
                        wrong += 1;
                    }
                }
            }
        }
        // Miscorrection is rare for these codes.
        assert!(wrong * 50 < rejected, "rejected {rejected}, miscorrected {wrong}");
    }
}
