//! Round trips through the modulator and demodulator, clean and with noise and twist.

use uafsk::demod::Config;
use uafsk::{Demodulator, Modulator, TxConfig};

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

fn frame(rng: &mut Rng, len: usize) -> Vec<u8> {
    // Two AX.25 addresses (shifted ASCII), UI control, no-L3 PID, then information.
    let mut f: Vec<u8> = b"APRS  ".iter().map(|c| c << 1).collect();
    f.push(0x60);
    f.extend(b"N0CALL".iter().map(|c| c << 1));
    f.push(0x61);
    f.extend([0x03, 0xF0]);
    f.extend((0..len).map(|_| (rng.next() % 256) as u8));
    f
}

/// Audio for frames, each with its own preamble, plus `space_db` of twist (space louder) and noise of `noise_rms`.
fn audio(frames: &[Vec<u8>], rate: u32, space_db: f64, noise_rms: f64, rng: &mut Rng) -> Vec<i16> {
    let cfg = TxConfig { rate_hz: rate, preamble: 20, postamble: 2, amp: 10000 };
    let mut out = Vec::new();
    for f in frames {
        let s: Vec<f64> = Modulator::new(f, cfg).map(|v| v as f64).collect();
        // Twist: a first-order high-shelf around 1700 Hz is close enough to radio pre-emphasis for this test.
        let g = 10f64.powf(space_db / 20.0);
        let mut prev = 0.0;
        for &v in &s {
            let hp = v - prev;
            prev = v;
            let y = v + (g - 1.0) * hp * 0.9;
            out.push((y + noise_rms * rng.gauss()).clamp(-32768.0, 32767.0) as i16);
        }
        out.extend(std::iter::repeat_n(0i16, rate as usize / 10));
    }
    out
}

fn decode(samples: &[i16], rate: u32, slicers: usize) -> Vec<Vec<u8>> {
    let mut got = Vec::new();
    if slicers == 1 {
        let mut d = Box::new(Demodulator::<1>::new(Config::new(rate)));
        for &x in samples {
            d.push(x, |f| got.push(f.to_vec()));
        }
    } else {
        let mut d = Box::new(Demodulator::<3>::new(Config::new(rate)));
        for &x in samples {
            d.push(x, |f| got.push(f.to_vec()));
        }
    }
    got
}

#[test]
fn clean_round_trip_at_several_rates() {
    let mut rng = Rng(3);
    let frames: Vec<Vec<u8>> = (0..20).map(|i| frame(&mut rng, 1 + i * 13)).collect();
    for rate in [9600, 12000, 22050, 44100, 48000] {
        let a = audio(&frames, rate, 0.0, 0.0, &mut rng);
        let got = decode(&a, rate, 1);
        assert_eq!(got, frames, "rate {rate}");
    }
}

#[test]
fn noise_and_twist() {
    let mut rng = Rng(5);
    let frames: Vec<Vec<u8>> = (0..40).map(|_| frame(&mut rng, 40)).collect();
    println!("decoded of {} frames at 9600/s (1 slicer, 3 slicers):", frames.len());
    for twist in [-6.0, 0.0, 6.0] {
        for noise in [1000.0, 2500.0, 4000.0] {
            let a = audio(&frames, 9600, twist, noise, &mut rng);
            let (one, three) = (decode(&a, 9600, 1), decode(&a, 9600, 3));
            let ok = |g: &Vec<Vec<u8>>| frames.iter().filter(|f| g.contains(f)).count();
            assert!(g_subset(&one, &frames) && g_subset(&three, &frames), "decoded a frame that was not sent");
            println!("  twist {twist:+} dB, noise {noise}: {} / {}", ok(&one), ok(&three));
            if noise <= 1000.0 {
                assert_eq!(ok(&three), frames.len(), "twist {twist}");
            }
        }
    }
}

fn g_subset(got: &[Vec<u8>], sent: &[Vec<u8>]) -> bool {
    got.iter().all(|g| sent.contains(g))
}

#[test]
fn steady_tone_silence_and_long_ones_find_nothing() {
    use uafsk::hdlc::Deframer;
    let mut d = Deframer::new();
    for _ in 0..10_000 {
        assert!(d.push(1).is_none());
    }
    for rate in [9600, 44100] {
        let mut rx: Demodulator = Demodulator::new(Config::new(rate));
        let mut n = 0;
        for i in 0..rate as usize * 3 {
            // 1200 Hz for a second, then silence, then 2200 Hz.
            let f = if i < rate as usize { 1200.0 } else if i < 2 * rate as usize { 0.0 } else { 2200.0 };
            let x = (10000.0 * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()) as i16;
            rx.push(x, |_| n += 1);
        }
        assert_eq!(n, 0);
    }
}

#[test]
fn deframer_limits() {
    use uafsk::hdlc::{Deframer, Framer, MAX_FRAME, MIN_FRAME};
    let run = |frame: &[u8], extra: &[u8]| -> Vec<Vec<u8>> {
        let mut d = Deframer::new();
        let mut out = Vec::new();
        for &b in extra {
            d.push(b);
        }
        for b in Framer::new(frame, 2, 2) {
            if let Some(f) = d.push(b) {
                out.push(f.to_vec());
            }
        }
        out
    };
    // Shortest and longest frames pass; one byte shorter or longer does not.
    let ok_short = vec![0x55u8; MIN_FRAME - 2];
    assert_eq!(run(&ok_short, &[]), vec![ok_short.clone()]);
    assert!(run(&ok_short[1..], &[]).is_empty());
    let ok_long = vec![0x7Eu8; MAX_FRAME - 2]; // flag bytes in the data: all stuffed
    assert_eq!(run(&ok_long, &[]), vec![ok_long.clone()]);
    assert!(run(&vec![0x7Eu8; MAX_FRAME - 1], &[]).is_empty());
    // Bits before the first flag, at any alignment, do not matter.
    for k in 0..8 {
        assert_eq!(run(&ok_short, &[1, 0, 1, 1, 0, 0, 1][..k.min(7)]), vec![ok_short.clone()]);
    }
}

#[test]
fn carrier_detect_follows_the_packet() {
    let mut rng = Rng(11);
    let f = frame(&mut rng, 60);
    let rate = 9600;
    let quiet = rate as usize; // a second of noise either side
    let mut audio: Vec<i16> = (0..quiet).map(|_| (2000.0 * rng.gauss()) as i16).collect();
    let start = audio.len();
    audio.extend(Modulator::new(&f, TxConfig { rate_hz: rate, preamble: 20, postamble: 2, amp: 10000 }));
    let end = audio.len();
    audio.extend((0..quiet).map(|_| (2000.0 * rng.gauss()) as i16));
    let mut d: Demodulator = Demodulator::new(Config::new(rate));
    let mut busy = Vec::new();
    let mut decoded = false;
    for &x in &audio {
        d.push(x, |_| decoded = true);
        busy.push(d.busy());
    }
    assert!(decoded);
    // Busy within about 15 flags (100 ms) of the preamble starting, then to the end of the packet; quiet in the
    // noise before and after.
    let preamble = 20 * 8 * rate as usize / 1200;
    assert!(!busy[..start].iter().any(|&b| b), "busy on noise before");
    let first_ms = busy[start..].iter().position(|&b| b).unwrap() * 1000 / rate as usize;
    assert!(first_ms <= 110, "busy only after {first_ms} ms");
    assert!(busy[start + preamble..end - 20].iter().all(|&b| b), "not busy during the packet");
    assert!(!busy[end + rate as usize / 10..].iter().any(|&b| b), "busy on noise after");
}

/// Tones (true = the lower) for a frame: flags, the stuffed frame with its FCS, NRZI (a 0 changes the tone).
fn tones(f: &[u8]) -> Vec<bool> {
    let mut t = false;
    uafsk::hdlc::Framer::new(f, 4, 2)
        .map(|b| {
            if b == 0 {
                t = !t;
            }
            t
        })
        .collect()
}

/// Tones back to bits into a deframer with repair `k`, each tone's confidence from `conf`: the frames out and
/// whether each was repaired.
fn deframe(tones: &[bool], k: u8, conf: impl Fn(usize) -> u32) -> Vec<(Vec<u8>, bool)> {
    let mut d = uafsk::hdlc::Deframer::new();
    d.set_repair(k);
    let mut last = false;
    let mut out = Vec::new();
    for (i, &t) in tones.iter().enumerate() {
        let bit = (t == last) as u8;
        last = t;
        let got = d.push_soft(bit, conf(i)).map(|(f, _)| f.to_vec());
        if let Some(f) = got {
            out.push((f, d.repaired()));
        }
    }
    out
}

/// One wrong tone decision anywhere in a frame (not next to a stuffed bit or the flags) is repaired when it is
/// among the least confident decisions, and only then; with repair off the frame is lost.
#[test]
fn repair_flips_the_least_confident_tone() {
    let mut rng = Rng(7);
    let f = frame(&mut rng, 40);
    let clean = tones(&f);
    assert_eq!(deframe(&clean, 3, |_| 1000), vec![(f.clone(), false)]);
    let (mut fixed, mut tried) = (0, 0);
    for p in 40..clean.len() - 24 {
        let mut t = clean.clone();
        t[p] = !t[p];
        // Off: the frame is lost (or, rarely, the error broke the framing).
        assert!(deframe(&t, 0, |_| 1000).iter().all(|(g, _)| *g != f));
        // The wrong tone least confident, two decoys a little less sure than the rest.
        let got = deframe(&t, 3, |i| if i == p { 10 } else if i == p / 2 || i == p / 3 { 50 } else { 1000 });
        // Confident about the wrong tone, unsure about three right ones: nothing to repair.
        let not = deframe(&t, 3, |i| if i == p { 1000 } else if i % 50 == 0 { 10 } else { 1000 });
        assert!(not.iter().all(|(g, _)| *g != f), "repaired a confident error at {p}");
        tried += 1;
        if got == vec![(f.clone(), true)] {
            fixed += 1;
        }
    }
    // The rest sit next to stuffed bits, where a flip would change the stuffing: left alone.
    assert!(fixed * 10 >= tried * 8, "repaired {fixed} of {tried}");
}

/// A repair must leave a sane AX.25 address field: a frame whose addresses are not callsigns is not repaired.
#[test]
fn repair_needs_ax25_addresses() {
    let f: Vec<u8> = (0..30u8).map(|i| i.wrapping_mul(37) | 1).collect();
    let mut t = tones(&f);
    assert_eq!(deframe(&t, 3, |_| 1000).len(), 1);
    let p = t.len() / 2;
    t[p] = !t[p];
    assert!(deframe(&t, 3, |i| if i == p { 10 } else { 1000 }).is_empty());
}

/// FX.25 through the modem: ufx25's encoder, `Modulator::from_bits`, noise, `Demodulator` with its FX.25 receivers.
/// Returns (plain frames, FX.25 frames that were not duplicates with their corrected bytes, FX.25 duplicates).
type Fx25Run = (Vec<Vec<u8>>, Vec<(Vec<u8>, u8)>, usize);

fn fx25_run(audio: &[i16], rate: u32, deferred: bool) -> Fx25Run {
    let mut d: Box<Demodulator> = Box::new(Demodulator::new(Config::new(rate)));
    let (mut plain, mut fx, mut dups) = (Vec::new(), Vec::new(), 0);
    for chunk in audio.chunks(24) {
        for &x in chunk {
            d.push(x, |f| plain.push(f.to_vec()));
        }
        if deferred {
            // As a firmware does: take the block out after each batch of samples, decode it elsewhere.
            if let Some(mut b) = d.take_fx25() {
                if let Some(f) = b.decode() {
                    if f.duplicate {
                        dups += 1;
                    } else {
                        fx.push((f.frame.to_vec(), f.corrected));
                    }
                }
            }
        } else {
            d.decode_fx25(|f| fx.push((f.frame.to_vec(), f.corrected)));
        }
    }
    (plain, fx, dups)
}

fn fx25_audio(rng: &mut Rng, f: &[u8], check: usize, wrong: usize, noise: f64, rate: u32) -> Vec<i16> {
    let mut enc = uafsk::ufx25::Encoder::new(f, check).unwrap();
    let n = enc.block().len();
    let mut hit = vec![false; n];
    let mut left = wrong;
    while left > 0 {
        let j = (rng.next() % n as u64) as usize;
        if !hit[j] {
            hit[j] = true;
            enc.block_mut()[j] ^= 1 + (rng.next() % 255) as u8;
            left -= 1;
        }
    }
    let cfg = TxConfig { rate_hz: rate, preamble: 20, postamble: 3, amp: 10000 };
    let mut audio: Vec<i16> = (0..rate as usize / 4).map(|_| (noise * rng.gauss()) as i16).collect();
    let tx: Vec<i16> = Modulator::from_bits(enc.bits(cfg.preamble, cfg.postamble), cfg).collect();
    audio.extend(tx.iter().map(|&x| (x as f64 + noise * rng.gauss()).clamp(-32768.0, 32767.0) as i16));
    audio.extend((0..rate as usize / 4).map(|_| (noise * rng.gauss()) as i16));
    audio
}

#[test]
fn fx25_round_trip() {
    let mut rng = Rng(25);
    let rate = 12_000;
    for check in [16, 32, 64] {
        for len in [20, 90, 150] {
            let f = frame(&mut rng, len);
            if uafsk::ufx25::Encoder::new(&f, check).is_err() {
                continue;
            }
            for deferred in [false, true] {
                // Clean: the plain deframer gets it, the FX.25 copy is a duplicate.
                let a = fx25_audio(&mut rng, &f, check, 0, 300.0, rate);
                let (plain, fx, dups) = fx25_run(&a, rate, deferred);
                assert_eq!((plain, fx.len(), dups), (vec![f.clone()], 0, deferred as usize), "clean {check} {len}");
                // Wrong bytes up to the code's limit (some inside the AX.25 frame): only FX.25 gets it, once.
                let t = check / 2;
                let a = fx25_audio(&mut rng, &f, check, t, 300.0, rate);
                let (plain, fx, _) = fx25_run(&a, rate, deferred);
                assert!(plain.is_empty(), "{check} {len}: the plain copy is broken");
                assert_eq!(fx, vec![(f.clone(), t as u8)], "{check} {len}: repaired");
                // One more than it can fix: nothing.
                let a = fx25_audio(&mut rng, &f, check, t + 1, 300.0, rate);
                let (plain, fx, _) = fx25_run(&a, rate, deferred);
                assert!(plain.is_empty() && fx.is_empty());
            }
        }
    }
}

#[test]
fn fx25_in_heavy_noise_beats_plain() {
    // A frame 40 times in noise loud enough to break most plain copies (measured: plain 12, with FX.25 40).
    let mut rng = Rng(7);
    let rate = 12_000;
    let f = frame(&mut rng, 60);
    let (mut plain_n, mut fx_n) = (0, 0);
    for _ in 0..40 {
        let a = fx25_audio(&mut rng, &f, 32, 0, 5000.0, rate);
        let (plain, fx, dups) = fx25_run(&a, rate, true);
        assert!(plain.iter().chain(fx.iter().map(|(g, _)| g)).all(|g| *g == f));
        plain_n += plain.len();
        fx_n += plain.len().max(fx.len() + dups);
    }
    println!("40 frames in noise: plain {plain_n}, with FX.25 {fx_n}");
    assert!(plain_n < 30 && fx_n >= plain_n + 15, "plain {plain_n}, with FX.25 {fx_n}");
}

#[test]
fn fx25_off_finds_only_plain() {
    let mut rng = Rng(3);
    let rate = 9600;
    let f = frame(&mut rng, 40);
    let a = fx25_audio(&mut rng, &f, 16, 0, 200.0, rate);
    let mut d: Demodulator = Demodulator::new(Config { fx25: false, ..Config::new(rate) });
    let mut n = 0;
    for &x in &a {
        d.push(x, |_| n += 1);
        assert!(d.take_fx25().is_none());
    }
    assert_eq!(n, 1);
}

#[test]
fn fx25_block_held_until_released() {
    // Two damaged FX.25 transmissions back to back. Unreleased, the first block holds the one buffer and the second
    // tag is skipped (counted); released after each batch, both come through.
    let mut rng = Rng(11);
    let rate = 12_000;
    let (f, g) = (frame(&mut rng, 50), frame(&mut rng, 70));
    let mut a = fx25_audio(&mut rng, &f, 16, 8, 300.0, rate);
    a.extend(fx25_audio(&mut rng, &g, 16, 8, 300.0, rate));
    for release in [false, true] {
        let mut d: Box<Demodulator> = Box::new(Demodulator::new(Config::new(rate)));
        let mut got = Vec::new();
        for chunk in a.chunks(24) {
            for &x in chunk {
                d.push(x, |_| {});
            }
            if let Some(b) = d.pending_fx25() {
                if release || got.is_empty() {
                    let fr = b.decode().unwrap();
                    assert_eq!((fr.corrected, fr.check, fr.duplicate), (8, 16, false));
                    got.push(fr.frame.to_vec());
                }
                if release {
                    d.release_fx25();
                }
            }
        }
        if release {
            assert_eq!((got, d.fx25_skipped()), (vec![f.clone(), g.clone()], 0));
        } else {
            // Every slicer found the second tag: three skipped.
            assert_eq!((got, d.fx25_skipped()), (vec![f.clone()], 3));
        }
    }
}

/// Each slicer's tone decisions from `Fsk::push`, NRZI-decoded into its own deframer.
fn fsk_frames<const S: usize, const C: usize>(fsk: &mut uafsk::fsk::Fsk<S, C>, audio: &[i16]) -> Vec<(usize, Vec<u8>)> {
    use uafsk::hdlc::Deframer;
    let mut d: Vec<Deframer> = (0..S * C).map(|_| Deframer::default()).collect();
    let mut prev = vec![false; S * C];
    let mut out = Vec::new();
    for &x in audio {
        fsk.push(x, |i, mark| {
            let bit = (mark == prev[i]) as u8;
            prev[i] = mark;
            if let Some(f) = d[i].push(bit) {
                out.push((i, f.to_vec()));
            }
        });
    }
    out
}

#[test]
fn every_slicer_and_chain_decodes() {
    // Four slicers (the fourth on raw amplitudes) on two chains (the first high-passed, the second not): each of
    // the eight decodes a clean frame on its own.
    use uafsk::fsk::{Fsk, BELL202};
    let mut rng = Rng(5);
    let f = frame(&mut rng, 40);
    let rate = 9600;
    let mut audio: Vec<i16> = Modulator::new(&f, TxConfig { rate_hz: rate, ..TxConfig::default() }).collect();
    audio.extend([0i16; 960]);
    let mut fsk = Fsk::<4, 2>::new(uafsk::fsk::Config::new(rate), BELL202);
    let got = fsk_frames(&mut fsk, &audio);
    assert!(got.iter().all(|(_, g)| *g == f));
    let mut who: Vec<usize> = got.iter().map(|(i, _)| *i).collect();
    who.sort();
    assert_eq!(who, (0..8).collect::<Vec<_>>());
}

#[test]
fn loud_audio_at_48000() {
    // Near full scale at 48000 samples/s: the tone levels outgrow the 32-bit fast path of the normaliser.
    let mut rng = Rng(9);
    let f = frame(&mut rng, 30);
    let audio: Vec<i16> = Modulator::new(&f, TxConfig { rate_hz: 48_000, preamble: 20, postamble: 2, amp: 32000 }).chain([0; 4800]).collect();
    assert_eq!(decode(&audio, 48_000, 3), vec![f]);
}

#[test]
fn defaults() {
    let t = TxConfig::default();
    assert_eq!((t.rate_hz, t.preamble, t.postamble, t.amp), (9600, 30, 3, 16000));
    // A demodulator with no block in hand: releasing is a no-op, nothing to take.
    let mut d: Demodulator = Demodulator::new(Config::new(9600));
    d.release_fx25();
    assert!(d.pending_fx25().is_none() && d.take_fx25().is_none());
}

#[test]
fn framer_ends_and_stays_ended() {
    use uafsk::hdlc::Framer;
    let mut fr = Framer::new(&[0x55; 20], 1, 1);
    let n = fr.by_ref().count();
    // 20 bytes plus FCS (no stuffing in 0x55; the FCS may stuff a bit or two) between two flags.
    assert!((8 * 24..8 * 24 + 3).contains(&n), "{n} bits");
    assert_eq!(fr.next(), None);
    assert_eq!(fr.next(), None);
}

#[test]
fn ax25_address_field_checks() {
    use uafsk::hdlc::ax25_addresses_ok;
    let addr = |call: &[u8; 6], last: bool| -> Vec<u8> {
        let mut a: Vec<u8> = call.iter().map(|c| c << 1).collect();
        a.push(0x60 | last as u8);
        a
    };
    let two = [addr(b"APRS  ", false), addr(b"N0CALL", true)].concat();
    assert!(ax25_addresses_ok(&two));
    assert!(ax25_addresses_ok(&[two.clone(), vec![0x03, 0xF0]].concat()));
    // One address only, cut short, a lower-case callsign, or an extension bit inside a callsign: not AX.25.
    assert!(!ax25_addresses_ok(&addr(b"N0CALL", true)));
    assert!(!ax25_addresses_ok(&two[..10]));
    assert!(!ax25_addresses_ok(&[addr(b"aprs  ", false), addr(b"N0CALL", true)].concat()));
    let mut odd = two.clone();
    odd[2] |= 1;
    assert!(!ax25_addresses_ok(&odd));
    // Ten addresses (destination, source, eight digipeaters) is the most; an eleventh is too many.
    let path = |n: usize| -> Vec<u8> { (0..n).flat_map(|i| addr(b"WIDE1 ", i == n - 1)).collect() };
    assert!(ax25_addresses_ok(&path(10)));
    assert!(!ax25_addresses_ok(&path(11)));
}

#[test]
fn fx25_block_accessors() {
    // The block handed out: its code, tag errors and bytes as received; decoding corrects it in place.
    let mut rng = Rng(13);
    let rate = 12_000;
    let f = frame(&mut rng, 50);
    let a = fx25_audio(&mut rng, &f, 32, 5, 300.0, rate);
    let mut d: Box<Demodulator> = Box::new(Demodulator::new(Config::new(rate)));
    let mut block = None;
    for &x in &a {
        d.push(x, |_| {});
        if let Some(b) = d.take_fx25() {
            block = Some(b);
        }
    }
    let mut b = block.expect("an FX.25 block");
    let enc = uafsk::ufx25::Encoder::new(&f, 32).unwrap();
    assert_eq!(b.mode(), Some(enc.mode()));
    assert_eq!(b.tag_errors(), 0);
    assert_eq!(b.block().len(), enc.block().len());
    let wrong = b.block().iter().zip(enc.block()).filter(|(x, y)| x != y).count();
    assert_eq!(wrong, 5);
    let fr = b.decode().unwrap();
    assert_eq!((fr.frame, fr.corrected, fr.check), (&f[..], 5, 32));
    // Corrected in place: the check bytes now match the encoder's.
    let k = enc.mode().k as usize;
    assert_eq!(&b.block()[k..], &enc.block()[k..]);
}
