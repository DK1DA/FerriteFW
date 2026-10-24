//! BCH correction, encode-decode round trips, and the receiver on synthetic discriminator audio.

use upocsag::encode::{encode, Content, PREAMBLE_BITS};
use upocsag::{bch, Decoder, Message, Receiver, IDLE, SYNC};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn gauss(&mut self) -> f64 {
        let u1 = ((self.next() >> 11) as f64 / (1u64 << 53) as f64).max(1e-300);
        let u2 = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

#[derive(Debug, PartialEq)]
struct Got {
    baud: u16,
    address: u32,
    function: u8,
    alpha: String,
    numeric: String,
    errors: u8,
}

fn got(m: &Message) -> Got {
    Got {
        baud: m.baud,
        address: m.address,
        function: m.function,
        alpha: m.alpha().map(|c| c as char).collect::<String>().trim_end_matches('\0').to_string(),
        numeric: m.numeric().map(|c| c as char).collect::<String>().trim_end().to_string(),
        errors: m.errors,
    }
}

#[test]
fn sync_and_idle_are_codewords() {
    assert_eq!(bch::correct(IDLE), Some((IDLE, 0)));
    assert_eq!(bch::encode(IDLE >> 11), IDLE);
    // The sync word is a codeword too, so the receiver must know where batches start.
    assert_eq!(bch::correct(SYNC), Some((SYNC, 0)));
}

#[test]
fn bch_corrects_two_errors() {
    let mut rng = Rng(1);
    for _ in 0..200 {
        let cw = bch::encode((rng.next() & 0x1F_FFFF) as u32);
        assert_eq!(bch::correct(cw), Some((cw, 0)));
        for i in 0..32 {
            assert_eq!(bch::correct(cw ^ 1 << i), Some((cw, 1)));
            for j in i + 1..32 {
                assert_eq!(bch::correct(cw ^ 1 << i ^ 1 << j), Some((cw, 2)), "bits {i} {j}");
            }
        }
    }
}

#[test]
fn bch_rejects_most_triple_errors() {
    let mut rng = Rng(2);
    let (mut wrong, mut n) = (0, 0);
    for _ in 0..20000 {
        let cw = bch::encode((rng.next() & 0x1F_FFFF) as u32);
        let mut e = 0u32;
        while e.count_ones() < 3 {
            e |= 1 << (rng.next() % 32);
        }
        n += 1;
        if let Some((c, _)) = bch::correct(cw ^ e) {
            assert_ne!(c, cw);
            wrong += 1;
        }
    }
    // Triple errors always keep odd parity, which the corrector never accepts as two errors; only a 1-error
    // mis-correction of the 31 bits plus the parity bit can let one through.
    assert!(wrong * 10 < n, "{wrong} of {n} triple errors accepted");
}

fn words(address: u32, function: u8, content: Content) -> Vec<u32> {
    let mut w = Vec::new();
    encode(address, function, content, |x| w.push(x));
    w
}

fn decode_words(w: &[u32]) -> Vec<Got> {
    let mut d: Decoder = Decoder::new();
    let mut out = Vec::new();
    for &x in w {
        if x == SYNC {
            d.sync();
        } else {
            d.word(x, |m| out.push(got(m)));
        }
    }
    d.end(|m| out.push(got(m)));
    out
}

#[test]
fn round_trip_codewords() {
    let cases: [(u32, u8, Content); 5] = [
        (1234567, 3, Content::Alpha(b"THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 1234567890")),
        (8, 0, Content::Numeric(b"0123456789 -U.][")),
        (2097151, 2, Content::Tone),
        (200, 3, Content::Alpha(b"DAPNET")),
        (7, 1, Content::Alpha(&[b'x'; 200])),
    ];
    for (a, f, c) in cases {
        let g = decode_words(&words(a, f, c));
        assert_eq!(g.len(), 1, "{a}");
        assert_eq!((g[0].address, g[0].function), (a, f));
        match c {
            Content::Alpha(t) => assert_eq!(g[0].alpha.as_bytes(), t),
            Content::Numeric(t) => assert_eq!(g[0].numeric.as_bytes(), t),
            Content::Tone => assert_eq!(g[0].alpha, ""),
        }
    }
}

#[test]
fn decoder_corrects_and_counts_errors() {
    let mut w = words(1234567, 3, Content::Alpha(b"HELLO WORLD, THIS IS A TEST"));
    let mut rng = Rng(3);
    for (k, x) in w.iter_mut().enumerate() {
        if *x != SYNC {
            *x ^= 1 << (rng.next() % 32) | 1 << (rng.next() % 32);
            if k == 16 {
                // The first message codeword (the address sits in slot 14, word 15): beyond correction.
                *x ^= 0x0F00_0000;
            }
        }
    }
    let g = decode_words(&w);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].address, 1234567);
    assert!(g[0].errors >= 1);
}

#[test]
fn small_buffer_truncates() {
    let mut d: Decoder<4> = Decoder::new();
    let mut n = 0;
    for x in words(9, 3, Content::Alpha(b"A LONG MESSAGE FOR A SMALL BUFFER")) {
        if x == SYNC {
            d.sync();
        } else {
            d.word(x, |m| {
                n += 1;
                assert!(m.truncated && m.bits() == 32);
                assert_eq!(m.alpha().map(|c| c as char).collect::<String>(), "A LO");
            });
        }
    }
    assert_eq!(n, 1);
}

/// Discriminator audio for codewords at `baud`: preamble, NRZ bits at ±amp, a first-order low-pass at about
/// 0.7 x baud, then offset and noise.
fn audio(w: &[u32], baud: u32, rate: u32, invert: bool, dc: f64, noise: f64, rng: &mut Rng) -> Vec<i16> {
    let mut bits: Vec<u32> = (0..PREAMBLE_BITS).map(|i| (i as u32 + 1) & 1).collect();
    for &x in w {
        bits.extend((0..32).rev().map(|k| (x >> k) & 1));
    }
    let mut out: Vec<i16> = (0..rate / 5).map(|_| (noise * rng.gauss()) as i16).collect();
    let a = 1.0 - (-2.0 * std::f64::consts::PI * 0.7 * baud as f64 / rate as f64).exp();
    let (mut y, mut t) = (0.0, 0.0);
    let per = rate as f64 / baud as f64;
    for b in bits {
        // POCSAG: a 1 is the lower frequency, so the lower discriminator level (for a non-inverting receiver).
        let v = if (b == 1) != invert { -8000.0 } else { 8000.0 };
        t += per;
        while (out.len() as f64) < t + (rate / 5) as f64 {
            y += a * (v - y);
            out.push((y + dc + noise * rng.gauss()).clamp(-32768.0, 32767.0) as i16);
        }
    }
    out.extend((0..rate / 5).map(|_| (dc + noise * rng.gauss()) as i16));
    out
}

fn receive(samples: &[i16], rate: u32) -> Vec<Got> {
    let mut rx: Receiver = Receiver::new(rate);
    let mut out = Vec::new();
    for &x in samples {
        rx.push(x, |m| out.push(got(m)));
    }
    rx.flush(|m| out.push(got(m)));
    out
}

#[test]
fn receiver_every_rate() {
    let text = b"FERRITE POCSAG TEST 0123456789";
    let w = words(1234567, 3, Content::Alpha(text));
    let mut rng = Rng(4);
    for rate in [9600, 22050, 32000] {
        for baud in [512u16, 1200, 2400] {
            for invert in [false, true] {
                let a = audio(&w, baud as u32, rate, invert, 3000.0, 1500.0, &mut rng);
                let g = receive(&a, rate);
                assert_eq!(g.len(), 1, "{rate} S/s, {baud} baud, inverted {invert}: {g:?}");
                assert_eq!(g[0].baud, baud);
                assert_eq!(g[0].address, 1234567);
                assert_eq!(g[0].alpha.as_bytes(), text);
            }
        }
    }
}

#[test]
fn receiver_in_noise() {
    // Noise at half the signal's level: about 6 dB SNR at the slicer before averaging.
    let w = words(400, 0, Content::Numeric(b"5551234"));
    let mut rng = Rng(5);
    let mut ok = 0;
    for _ in 0..10 {
        let g = receive(&audio(&w, 1200, 9600, false, 0.0, 4000.0, &mut rng), 9600);
        ok += g.iter().any(|m| m.address == 400 && m.numeric == "5551234") as u32;
    }
    assert!(ok >= 9, "{ok} of 10");
}

#[test]
fn silence_and_noise_find_nothing() {
    let mut rng = Rng(6);
    let a: Vec<i16> = (0..9600 * 30).map(|_| (6000.0 * rng.gauss()) as i16).collect();
    assert!(receive(&a, 9600).is_empty());
    assert!(receive(&vec![0; 9600], 9600).is_empty());
    assert!(receive(&vec![20000; 9600], 9600).is_empty());
}

#[test]
fn decoder_without_sync_keeps_frame_in_range() {
    let mut d: Decoder = Decoder::new();
    d.sync();
    for _ in 0..20 {
        d.word(IDLE, |_| panic!("idle words carry no message"));
    }
    // Capcode 1008 belongs in frame 0; past the batch the frame is capped, so the address stays within 21 bits.
    d.word(bch::encode((1008 >> 3) << 2), |_| {});
    let mut got = None;
    d.end(|m| got = Some(m.address));
    assert_eq!(got.map(|a| a >> 3), Some(1008 >> 3));
}

#[test]
fn signal_lost_mid_message_reports_it_once_without_noise() {
    let text = b"A MESSAGE THAT IS CUT SHORT BY A FADE";
    let w = words(1234567, 3, Content::Alpha(text));
    // Keep the first batch and three codewords of the second, then noise.
    let mut rng = Rng(7);
    let cut: Vec<u32> = w[..17 + 1 + 3].iter().copied().chain((0..40).map(|_| rng.next() as u32)).collect();
    let a = audio(&cut, 1200, 9600, false, 0.0, 300.0, &mut rng);
    let g = receive(&a, 9600);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0].address, 1234567);
    assert!(text.starts_with(g[0].alpha.trim_end_matches('\0').as_bytes()), "{:?}", g[0].alpha);
    assert!(g[0].alpha.len() >= 10);
}

#[test]
fn back_to_back_rates_hand_over() {
    let mut rng = Rng(8);
    let mut a = Vec::new();
    let plan = [(512u16, 100u32), (2400, 200), (1200, 300), (512, 400)];
    for (baud, addr) in plan {
        let w = words(addr, 3, Content::Alpha(b"HANDOVER"));
        a.extend(audio(&w, baud as u32, 22050, false, 0.0, 500.0, &mut rng));
    }
    let g = receive(&a, 22050);
    let got: Vec<(u16, u32)> = g.iter().map(|m| (m.baud, m.address)).collect();
    assert_eq!(got, plan.to_vec());
}

#[test]
fn rate_limits() {
    let mut rng = Rng(9);
    for (rate, baud) in [(9600, 2400u32), (32768, 512), (32768, 2400), (9600, 512)] {
        let w = words(5000, 3, Content::Alpha(b"EDGE OF THE RANGE"));
        let g = receive(&audio(&w, baud, rate, true, -2000.0, 800.0, &mut rng), rate);
        assert!(g.iter().any(|m| m.address == 5000 && m.alpha == "EDGE OF THE RANGE"), "{rate} {baud}: {g:?}");
    }
}

#[test]
#[should_panic]
fn encode_rejects_bad_capcode() {
    encode(1 << 21, 0, Content::Tone, |_| {});
}
