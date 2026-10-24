//! udsp against plain floating-point references.
#![allow(clippy::needless_range_loop)]

use udsp::fft::{cfft, real_power};
use udsp::log::log2_q8;
use udsp::mix::{lowpass, FrontEnd};
use udsp::trig::cos_sin;

#[test]
fn integer_fft_matches_dft() {
    let n = 1024;
    let x: Vec<i16> = (0..n).map(|i| (((i * 7919) % 97) as f32 * 40.0 - 1940.0 + 3000.0 * (i as f32 * 0.3).sin()) as i16).collect();
    let mut z = x.clone();
    let shift = cfft(&mut z, n / 2);
    let mut got = vec![0f64; n / 2];
    real_power(&z, n / 2, 0, 1, n / 2, |k, p| got[k] = p as f64 * 4f64.powi(shift as i32));
    let total: f64 = got.iter().sum();
    for k in 0..n / 2 {
        let (mut re, mut im) = (0f64, 0f64);
        for (i, &v) in x.iter().enumerate() {
            let a = -2.0 * std::f64::consts::PI * (k * i) as f64 / n as f64;
            re += v as f64 * a.cos();
            im += v as f64 * a.sin();
        }
        let want = re * re + im * im;
        // Within 0.2 dB for bins carrying real energy, and quantisation noise well below the total elsewhere.
        if want > total * 1e-4 {
            assert!((10.0 * (got[k] / want).log10()).abs() < 0.2, "bin {k}: {} vs {}", got[k], want);
        } else {
            assert!(got[k] < total * 1e-4, "bin {k}: {} vs {} (total {total})", got[k], want);
        }
    }
}

#[test]
fn integer_log2() {
    for x in [1u64, 2, 3, 7, 100, 1000, 65_535, 1 << 20, 123_456_789, 1 << 40, u64::MAX / 3] {
        let got = log2_q8(x) as f64 / 256.0;
        assert!((got - (x as f64).log2()).abs() < 0.01, "log2({x}) = {got}");
    }
}

#[test]
fn sine_table() {
    for k in (0..4096).step_by(37) {
        let (c, s) = cos_sin(k);
        let a = 2.0 * std::f64::consts::PI * k as f64 / 4096.0;
        assert!((c as f64 / 32767.0 - a.cos()).abs() < 1e-4 && (s as f64 / 32767.0 - a.sin()).abs() < 1e-4, "k {k}");
    }
}

#[test]
fn mixer_moves_a_tone_to_baseband_and_rejects_others() {
    // 6400/s in, decimate by 16, mix 1000 Hz to 0 Hz: a 1050 Hz tone comes out at +50 Hz; a 1500 Hz tone (beyond
    // the 200 Hz cutoff) is suppressed.
    static TAPS: [i16; 256] = lowpass::<256>(200.0 / 6400.0);
    let step = ((1000u64 << 32) / 6400) as u32;
    let run = |f: f64| {
        let mut fe = Box::new(FrontEnd::<256>::new());
        let mut out = Vec::new();
        for n in 0..6400 * 2 {
            let x = (8000.0 * (2.0 * std::f64::consts::PI * f * n as f64 / 6400.0).sin()) as i16;
            if let Some(v) = fe.push(x, &TAPS, step, 16) {
                out.push(v);
            }
        }
        out.split_off(40) // skip the filter's start-up
    };
    let pass = run(1050.0);
    let power = |v: &[(i16, i16)]| v.iter().map(|&(r, i)| r as f64 * r as f64 + i as f64 * i as f64).sum::<f64>() / v.len() as f64;
    let p_pass = power(&pass);
    assert!((p_pass.sqrt() - 4000.0).abs() < 400.0, "passband amplitude {}", p_pass.sqrt()); // half the real amplitude
    // Frequency: phase advance per output sample = 2 pi 50 / 400.
    let mut dphi = 0.0;
    for w in pass.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (ar, ai, br, bi) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
        dphi -= (ai * br - ar * bi).atan2(ar * br + ai * bi);
    }
    let f_out = dphi / (pass.len() - 1) as f64 / (2.0 * std::f64::consts::PI) * 400.0;
    assert!((f_out - 50.0).abs() < 1.0, "baseband frequency {f_out}");
    let p_stop = power(&run(1500.0));
    assert!(10.0 * (p_stop / p_pass).log10() < -40.0, "stopband {} dB", 10.0 * (p_stop / p_pass).log10());
}

#[test]
fn interpolated_oscillator() {
    use udsp::trig::cos_sin_phase;
    for i in 0..1000u64 {
        let phase = (i * 0x0123_4567 + 0x89AB) as u32;
        let (c, s) = cos_sin_phase(phase);
        let a = 2.0 * std::f64::consts::PI * phase as f64 / 4294967296.0;
        assert!((c as f64 / 32767.0 - a.cos()).abs() < 1e-4 && (s as f64 / 32767.0 - a.sin()).abs() < 1e-4, "phase {phase}");
    }
}

#[test]
fn goertzel_fraction() {
    use udsp::goertzel::Goertzel;
    let n = 205u32;
    let block = |f: f64| -> (Goertzel, Goertzel, u64) {
        let (mut on, mut off, mut energy) = (Goertzel::new(1_000_000, 8000), Goertzel::new(1_336_000, 8000), 0u64);
        for i in 0..n {
            let x = (10000.0 * (2.0 * std::f64::consts::PI * f * i as f64 / 8000.0).sin()) as i16;
            on.push(x);
            off.push(x);
            energy += (x as i64 * x as i64) as u64;
        }
        (on, off, energy)
    };
    let (on, off, e) = block(1000.0);
    let f_on = on.fraction(n, e) as f64 / 65536.0;
    let f_off = off.fraction(n, e) as f64 / 65536.0;
    assert!((f_on - 1.0).abs() < 0.05, "on-frequency share {f_on}");
    assert!(f_off < 0.01, "far-off share {f_off}");
}

#[test]
fn real_decimator() {
    use udsp::fir::Decimator;
    static TAPS: [i16; 128] = lowpass::<128>(320.0 / 8000.0);
    let run = |f: f64| {
        let mut d = Box::new(Decimator::<128>::new());
        let mut out = Vec::new();
        for n in 0..8000 * 2 {
            let x = (10000.0 * (2.0 * std::f64::consts::PI * f * n as f64 / 8000.0).sin()) as i16;
            if let Some(y) = d.push(x, &TAPS, 10) {
                out.push(y as f64);
            }
        }
        let tail = &out[100..];
        (tail.iter().map(|y| y * y).sum::<f64>() / tail.len() as f64).sqrt()
    };
    let pass = run(100.0);
    assert!((pass / (10000.0 / 2f64.sqrt()) - 1.0).abs() < 0.05, "100 Hz passes at {pass}");
    let stop = run(1000.0);
    assert!(20.0 * (stop / pass).log10() < -40.0, "1000 Hz stopped by {} dB", 20.0 * (stop / pass).log10());
}

#[test]
fn crc16_x25_check_value() {
    use udsp::crc::{crc16_x25, crc16_x25_update, CRC16_X25_GOOD};
    assert_eq!(crc16_x25(b"123456789"), 0x906E); // the catalogued check value
    let mut frame = b"ABCDEF".to_vec();
    let fcs = crc16_x25(&frame);
    frame.extend_from_slice(&fcs.to_le_bytes());
    assert_eq!(crc16_x25_update(0xFFFF, &frame), CRC16_X25_GOOD);
}

#[test]
fn integer_fft_full_scale_does_not_overflow() {
    // Worst cases for 16-bit butterflies: full-scale noise, and a constant at full scale on both parts.
    let mut seed = 12345u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for m in [8usize, 16, 64, 512] {
        let inputs: [Vec<i16>; 2] = [
            (0..2 * m).map(|_| (rnd() >> 16) as i16).collect(),
            (0..2 * m).map(|i| if i % 2 == 0 { 32767 } else { -32768 }).collect(),
        ];
        for x in inputs {
            let mut z = x.clone();
            let shift = cfft(&mut z, m);
            let scale = 2f64.powi(shift as i32);
            let mut worst = 0f64;
            let mut peak = 0f64;
            for k in 0..m {
                let (mut re, mut im) = (0f64, 0f64);
                for i in 0..m {
                    let a = -2.0 * std::f64::consts::PI * (k * i) as f64 / m as f64;
                    let (xr, xi) = (x[2 * i] as f64, x[2 * i + 1] as f64);
                    re += xr * a.cos() - xi * a.sin();
                    im += xr * a.sin() + xi * a.cos();
                }
                let (gr, gi) = (z[2 * k] as f64 * scale, z[2 * k + 1] as f64 * scale);
                worst = worst.max(((gr - re).powi(2) + (gi - im).powi(2)).sqrt());
                peak = peak.max((re * re + im * im).sqrt());
            }
            assert!(worst < peak * 0.01, "m {m}: error {worst} against peak {peak}");
        }
    }
}
