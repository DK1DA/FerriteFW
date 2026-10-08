//! The radio's APRS receive path on recorded audio: the WA8LMF TNC Test CD (fetched into udigimodes with its
//! `tools/fetch_aprs_testdata.sh`; never redistributed, so this test is ignored when it isn't there). The audio
//! goes the way PA2's does on the radio: 48 kHz, 12-bit ADC counts around mid-scale (`audio::Decim`), the 4:1
//! step, the decoder slot's AFSK demodulator at 12 kHz, then the APRS task's parser and heard list. Run with
//! `cargo test --release -p ferrite-app --test aprs_corpus -- --ignored --nocapture`.

use ferrite_app::aprs_live::AprsLive;
use ferrite_core::audio::{Decim, ADC_HZ};
use ferrite_core::decoders::{Decoder, Found};

fn testdata(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../udigimodes/testdata").join(rel)
}

fn read_wav(path: &std::path::Path) -> Option<(Vec<i16>, u32)> {
    let d = std::fs::read(path).ok()?;
    let mut i = 12;
    let (mut rate, mut data) = (0, Vec::new());
    while i + 8 <= d.len() {
        let len = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = &d[i + 8..(i + 8 + len).min(d.len())];
        match &d[i..i + 4] {
            b"fmt " => rate = u32::from_le_bytes(body[4..8].try_into().unwrap()),
            b"data" => data = body.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect(),
            _ => {}
        }
        i += 8 + len + (len & 1);
    }
    Some((data, rate))
}

/// The recording as the ADC would sample it: resampled to 48 kHz (linear), a full-scale recording swinging
/// `peak` counts around 2048.
fn as_adc(s: &[i16], rate: u32, peak: i32) -> Vec<u16> {
    let n = (s.len() as u64 * ADC_HZ as u64 / rate as u64) as usize;
    (0..n)
        .map(|k| {
            let t = k as f64 * rate as f64 / ADC_HZ as f64;
            let (i, f) = (t as usize, t.fract());
            let a = s[i.min(s.len() - 1)] as f64;
            let b = s[(i + 1).min(s.len() - 1)] as f64;
            let x = a + (b - a) * f;
            (2048.0 + x * peak as f64 / 32768.0).round().clamp(0.0, 4095.0) as u16
        })
        .collect()
}

#[test]
#[ignore]
fn wa8lmf_through_the_radio_path() {
    println!("\nWA8LMF TNC Test CD through the radio's path (48 kHz ADC, Decim, AFSK at 12 kHz, APRS task)");
    for (track, peak) in [("track01", 1500), ("track01", 400), ("track02", 1500), ("track02", 400)] {
        let Some((s, rate)) = read_wav(&testdata(&format!("wa8lmf/{track}.wav"))) else {
            println!("{track}: not fetched, skipped");
            continue;
        };
        let adc = as_adc(&s, rate, peak);
        let mut dec = Decim::new();
        let mut d = Box::new(Decoder::afsk());
        let aprs = AprsLive::new();
        let (mut frames, mut parsed) = (0, 0);
        let mut frame_bufs: Vec<Vec<u8>> = Vec::new();
        for block in adc.chunks(ferrite_core::audio::BLOCK_IN) {
            let mut out = [0i16; ferrite_core::audio::BLOCK_OUT];
            let mut n = 0;
            dec.block(block, |x| {
                out[n] = x;
                n += 1;
            });
            d.push(&out[..n], |f| {
                if let Found::Frame(b) = f {
                    frame_bufs.push(b.to_vec());
                }
            });
            for f in frame_bufs.drain(..) {
                frames += 1;
                let before = aprs.stations();
                let rx = aprs.receive(&f, frames, b"N0CALL", false);
                parsed += (rx.changed || aprs.stations() != before) as u32;
            }
        }
        println!("{track} at {:>4} counts peak: {frames:>5} frames with a good CRC, {parsed:>5} read as APRS, {} in the heard list", peak, aprs.stations());
        println!("RESULT ferrite.{track}.peak{peak}.frames {frames}");
    }
}
