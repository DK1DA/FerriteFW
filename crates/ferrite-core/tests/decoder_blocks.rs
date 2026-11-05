//! APRS-56.4: the DSP task's per-block work on noise, timed on the host (the radio's `work_max_us` measured
//! 6.9 ms on a 2 ms block). Run by hand: `cargo test -p ferrite-core --release --test decoder_blocks -- --ignored
//! --nocapture`.
use ferrite_core::decoders::{Decoder, Found, RATE_HZ};
use std::time::Instant;

#[test]
#[ignore]
fn afsk_block_work_on_noise() {
    let block = (RATE_HZ / 500) as usize; // 2 ms
    let mut d = Decoder::afsk();
    let mut seed = 0x1234_5678u32;
    let mut noise = || {
        let mut s = 0i32;
        for _ in 0..4 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            s += (seed >> 16) as i32 - 32768;
        }
        (s / 8) as i16
    };
    let blocks = 60 * 500;
    let mut times = Vec::with_capacity(blocks);
    let (mut frames, mut misses) = (0, 0);
    for _ in 0..blocks {
        let b: Vec<i16> = (0..block).map(|_| noise()).collect();
        let t = Instant::now();
        d.push(&b, |f| match f {
            Found::Frame(_) => frames += 1,
            Found::Miss(_) => misses += 1,
            _ => {}
        });
        times.push(t.elapsed().as_nanos() as u64);
    }
    let mut s = times.clone();
    s.sort_unstable();
    let mean = times.iter().sum::<u64>() / blocks as u64;
    println!("blocks {blocks}, mean {mean} ns, p50 {} ns, p99 {} ns, p99.9 {} ns, max {} ns", s[blocks / 2], s[blocks * 99 / 100], s[blocks * 999 / 1000], s[blocks - 1]);
    println!("over 4x mean: {}, over 10x: {}; frames {frames}, misses {misses}", times.iter().filter(|&&t| t > 4 * mean).count(), times.iter().filter(|&&t| t > 10 * mean).count());
}

/// APRS-56.4: the radio's own receive arithmetic (the decimator, then the AFSK decoder) on each WAV in
/// `$WAV_DIR` (48 kHz mono, an SDR's FM audio): frames decoded per file. Run by hand with `--ignored --nocapture`.
#[test]
#[ignore]
fn afsk_decodes_wav_dir() {
    let Ok(dir) = std::env::var("WAV_DIR") else { return };
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "wav")).collect();
    files.sort();
    for f in files {
        let b = std::fs::read(&f).unwrap();
        let pcm: Vec<i16> = b[44..].as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect();
        // as the ADC would give it: 12 bits around mid-scale
        let adc: Vec<u16> = pcm.iter().map(|&x| (2048 + x as i32 / 24).clamp(0, 4095) as u16).collect();
        // CUT=1: also the latest start (ms after the recording's first sample) that still decodes, 5 ms steps
        let latest = std::env::var("CUT").is_ok().then(|| (0..80).map(|c| c * 5).take_while(|&c| frames(&adc[c * 48..]) > 0).last());
        let n = frames(&adc);
        println!("{} {n} {latest:?}", f.file_name().unwrap().to_string_lossy());
    }
}

/// The frames the radio's arithmetic decodes from ADC samples at 48 kHz.
fn frames(adc: &[u16]) -> usize {
    use ferrite_core::audio::{Decim, BLOCK_IN, BLOCK_OUT};
    {
        let (mut dec, mut d, mut n) = (Decim::new(), Decoder::afsk(), 0);
        for blk in adc.chunks(BLOCK_IN) {
            let mut out = [0i16; BLOCK_OUT];
            let mut k = 0;
            for &s in blk {
                dec.push(s, |x| {
                    if k < BLOCK_OUT {
                        out[k] = x;
                        k += 1;
                    }
                });
            }
            d.push(&out[..k], |f| {
                if let Found::Frame(_) = f {
                    n += 1
                }
            });
        }
        n
    }
}
