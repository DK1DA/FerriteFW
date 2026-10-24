//! The packed (M4 DSP-instruction) FFT and mixer filter against the plain ones: bit-identical on random and
//! edge-case inputs. On the PC the instructions are the emulations in `udsp::arm`; tools/check-m4 runs the same
//! comparisons with the real instructions.
#![allow(clippy::needless_range_loop)] // indexed loops read better in DSP code

use udsp::arm::{cdot, cdot_plain, pack, smlad, smuadx, smusd, unpack};
use udsp::fft::{cfft_packed, cfft_plain};

/// A small deterministic generator (no dependencies).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    /// Mostly uniform 16-bit values, with the extremes and small values over-represented.
    fn i16(&mut self) -> i16 {
        match self.next() % 8 {
            0 => [i16::MIN, i16::MAX, -1, 0, 1, i16::MIN + 1][(self.next() % 6) as usize],
            1 => (self.next() % 64) as i16 - 32,
            _ => self.next() as i16,
        }
    }
}

const EDGES: [i16; 8] = [i16::MIN, i16::MIN + 1, -32767, -1, 0, 1, 32766, i16::MAX];

#[test]
fn instructions_match_their_definitions() {
    let mut r = Rng(1);
    let mut words: Vec<(i16, i16)> = EDGES.iter().flat_map(|&a| EDGES.iter().map(move |&b| (a, b))).collect();
    words.extend((0..20_000).map(|_| (r.i16(), r.i16())));
    for (i, &(a, b)) in words.iter().enumerate() {
        let (c, d) = words[(i * 7 + 3) % words.len()];
        let (x, y) = (pack(a, b), pack(c, d));
        assert_eq!(unpack(x), (a as i32, b as i32));
        let (a, b, c, d) = (a as i64, b as i64, c as i64, d as i64);
        let wrap = |v: i64| v as i32; // modulo 2^32, as the instructions
        assert_eq!(smusd(x, y), wrap(a * c - b * d), "smusd");
        assert_eq!(smuadx(x, y), wrap(a * d + b * c), "smuadx");
        let acc = r.next() as i32;
        assert_eq!(smlad(x, y, acc), wrap(acc as i64 + a * c + b * d), "smlad");
    }
}

#[test]
fn fft_packed_is_bit_identical() {
    let mut r = Rng(2);
    for &m in &[1usize, 2, 4, 8, 16, 64, 256, 1024, 2048, 4096] {
        for trial in 0..if m >= 1024 { 6 } else { 40 } {
            let z: Vec<i16> = match trial {
                0 => vec![i16::MAX; 2 * m],
                1 => vec![i16::MIN; 2 * m],
                2 => (0..2 * m).map(|i| if i % 2 == 0 { i16::MAX } else { i16::MIN }).collect(),
                3 => (0..2 * m).map(|i| EDGES[i % EDGES.len()]).collect(),
                4 => (0..2 * m).map(|i| ((i as f64 * 0.37).sin() * 12_000.0) as i16).collect(), // a tone, no halving
                _ => (0..2 * m).map(|_| r.i16()).collect(),
            };
            let (mut a, mut b) = (z.clone(), z.clone());
            let (sa, sb) = (cfft_plain(&mut a, m), cfft_packed(&mut b, m));
            assert_eq!(sa, sb, "shift, m = {m}, trial {trial}");
            assert_eq!(a, b, "m = {m}, trial {trial}");
        }
    }
}

#[test]
fn fft_packed_leaves_the_rest_alone() {
    // A longer buffer than 2m: only the first 2m values are the FFT's.
    let mut r = Rng(3);
    let z: Vec<i16> = (0..600).map(|_| r.i16()).collect();
    let (mut a, mut b) = (z.clone(), z.clone());
    cfft_plain(&mut a, 256);
    cfft_packed(&mut b, 256);
    assert_eq!(a, b);
    assert_eq!(&b[512..], &z[512..]);
}


#[test]
fn complex_dot_is_bit_identical() {
    let mut r = Rng(4);
    for n in 0..70 {
        for _ in 0..30 {
            let b: Vec<i16> = (0..n + 1).map(|_| r.i16()).collect();
            let v: Vec<[i16; 2]> = (0..n + 1).map(|_| [r.i16(), r.i16()]).collect();
            // Offsets 0 and 1 test unaligned 32-bit loads.
            let (ov, ob) = ((r.next() % 2) as usize, (r.next() % 2) as usize);
            assert_eq!(cdot(&v[ov..ov + n], &b[ob..ob + n]), cdot_plain(&v[ov..ov + n], &b[ob..ob + n]), "n = {n}");
        }
    }
    // Sums that wrap: every product at its largest.
    let (v, h) = (vec![[i16::MIN, i16::MIN]; 4097], vec![i16::MIN; 4097]);
    assert_eq!(cdot(&v, &h), cdot_plain(&v, &h));
}

#[test]
fn filters_are_bit_identical() {
    // The filters through their public API, packed against a plain re-implementation.
    use udsp::fir::Decimator;
    use udsp::mix::{lowpass, FrontEnd};
    const L: usize = 63;
    const TAPS: [i16; L] = lowpass::<L>(0.1);
    let mut r = Rng(5);
    let mut d: Decimator<L> = Decimator::new();
    let mut fe: FrontEnd<L> = FrontEnd::new();
    let mut ring = [0i16; L];
    let mut cring = [[0i16; 2]; L];
    let (mut pos, mut phase) = (0usize, 0u32);
    for i in 0..20_000usize {
        let x = r.i16();
        // Plain decimator.
        ring[pos] = x;
        let (c, s) = udsp::trig::cos_sin((phase >> 20) as usize);
        cring[pos] = [((x as i32 * c) >> 15) as i16, (-(x as i32 * s) >> 15) as i16];
        pos = (pos + 1) % L;
        phase = phase.wrapping_add(123_456_789);
        let got = d.push(x, &TAPS, 4);
        let gotc = fe.push(x, &TAPS, 123_456_789, 4);
        if (i + 1) % 4 == 0 {
            let mut acc = 0i32;
            let mut cr = (0i32, 0i32);
            for k in 0..L {
                let idx = (pos + k) % L; // oldest first
                acc = acc.wrapping_add(ring[idx] as i32 * TAPS[k] as i32);
                cr.0 = cr.0.wrapping_add(cring[idx][0] as i32 * TAPS[k] as i32);
                cr.1 = cr.1.wrapping_add(cring[idx][1] as i32 * TAPS[k] as i32);
            }
            assert_eq!(got, Some((acc >> 15).clamp(-32768, 32767) as i16), "sample {i}");
            let want = ((cr.0 >> 15).clamp(-32768, 32767) as i16, (cr.1 >> 15).clamp(-32768, 32767) as i16);
            assert_eq!(gotc, Some(want), "complex, sample {i}");
        } else {
            assert_eq!((got, gotc), (None, None));
        }
    }
}
