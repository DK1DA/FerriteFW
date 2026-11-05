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
