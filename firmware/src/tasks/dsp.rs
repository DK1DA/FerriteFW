//! DSP: audio from ADC1 (PA2, the BK4829s' EARO) through the 4:1 step into the one decoder slot, and AFSK out
//! of DAC1 (APRS-1, APRS-14; roadmap 2f, 5b, 5c). Runs on the second interrupt executor, below Radio.
//!
//! - **Receive:** while a decoder runs, the capture runs ([`hal::audio`]); each 2 ms half goes through
//!   [`Decim`] to the slot. An AFSK frame with a good CRC goes to the UI task's APRS task through [`APRS_RX`], a
//!   page to the UI task through [`PAGES`].
//! - **One slot, two wanting it (DEC-18):** the APRS task asks for AFSK, the UI task for the pager decoder; APRS
//!   comes first (APRS mode gives it slot C), the pagers get the slot whenever APRS doesn't want it. Background
//!   jobs on watched slots (roadmap 6f) will share it more finely.
//! - **Transmit:** the UI task keys slot C, then hands a frame to [`AFSK_TX`]; it is played from TX delay's
//!   flags to the end (the flags cover the key-up), PC12 high meanwhile, and [`AFSK_DONE`] says it's out so the
//!   UI task unkeys. Receiving pauses while it plays.
//! - **Transmit test (TERM-33, TERM-34, the tx-bench build; TERM-38):** the Radio task, after a PTT key-up with `txtest` armed,
//!   signals [`TXTEST`] with the generator to play (a test signal at the arming's level, made a sample at a time:
//!   [`ferrite_core::txtest::Gen`]); a new generator while one plays takes over at the next half (the suite's
//!   steps, DAC1 running throughout); None at the release stops it. Receiving pauses while it plays, as for AFSK.

use crate::chan::DSP;
use crate::hal::{adc, audio, clock};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use ferrite_app::aprs_live::FRAME_MAX;
use ferrite_core::audio::{afsk_tx_at, to_dac, tx_bits, uafsk, Decim, TxBits, BLOCK_DAC, BLOCK_IN, BLOCK_OUT};
use ferrite_core::decoders::{Decoder, Found, Page};
use ferrite_core::msg::{DecoderKind, DspCmd};

/// An AX.25 frame without its FCS, as the demodulator gives it and the modulator takes it.
#[derive(Clone, Copy)]
pub struct Frame {
    pub len: u16,
    /// Transmit: the flags ahead of it, in 10 ms (KISS's TX delay; 30 = 300 ms, KISS-2's default).
    pub txdelay_10ms: u8,
    /// Transmit: FX.25 check bytes (16, 32, 64), or 0 for plain AX.25 (APRS-45).
    pub fx25: u8,
    /// Transmit: the AFSK level (APRS-49: 0 Low, 1 Mid, 2 High).
    pub level: u8,
    pub b: [u8; FRAME_MAX],
}

impl Frame {
    pub fn bytes(&self) -> &[u8] {
        &self.b[..self.len as usize]
    }
}

/// What the decoder heard, for the APRS task: a frame, or an FX.25 block for it to correct (APRS-44: 0.27 to 1.03
/// M instructions, 3 to 13 ms at 120 MHz, too long for this task's 2 ms halves).
pub enum Heard {
    Frame(Frame),
    Fx25(uafsk::Fx25Block),
}

/// Frames heard, to the UI task's APRS task: two deep (a second waits while the first is read; 672 B).
pub static APRS_RX: Channel<M, Heard, 2> = Channel::new();
/// A frame to send on the transmission the UI task just keyed, and its "sent" (true) or "not sent" (false).
pub static AFSK_TX: Signal<M, Frame> = Signal::new();
pub static AFSK_DONE: Signal<M, bool> = Signal::new();
/// TERM-33, TERM-34: the transmit test's generator to play now (Some, from the key-up; another Some takes over
/// seamlessly) or to stop (None, at the release).
#[cfg(all(feature = "tx", feature = "bench"))]
pub static TXTEST: Signal<M, Option<ferrite_core::txtest::Gen>> = Signal::new();
/// PWR-26.4: how often (in 2 ms capture halves) a capture running at 24 MHz asks again for the build's speed.
const RECHECK_HALVES: u32 = 5_000;
/// Pages heard, to the UI task (DEC-7): two deep, 144 B.
pub static PAGES: Channel<M, Page, 2> = Channel::new();
/// LINK-13.4: receive audio for the PC, a block every 32 ms while the cable's session listens; six deep (192 ms:
/// the UI task takes them each pass, at most [`AUDIO_TICK_MS`] apart meanwhile, plus a pass's own drawing time).
pub static AUDIO_BLOCKS: Channel<M, ferrite_core::rxaudio::Block, 6> = Channel::new();
/// LINK-13.4: the UI task's longest wait between passes while listening.
pub const AUDIO_TICK_MS: u64 = 60;
/// LINK-13.4: blocks made, and dropped because the UI task hadn't taken the last six (the readback's `audio` line).
pub static AUDIO_MADE: AtomicU32 = AtomicU32::new(0);
pub static AUDIO_DROPS: AtomicU32 = AtomicU32::new(0);
/// Frames decoded and dropped (the UI hadn't taken the last two), for the bench.
pub static FRAMES: AtomicU32 = AtomicU32::new(0);
pub static DROPPED: AtomicU32 = AtomicU32::new(0);


#[embassy_executor::task]
pub async fn task() {
    // The slot lives in this task's future, which is a static: one decoder's memory, whichever is running.
    let mut slot = Decoder::Off;
    let mut dec = Decim::new();
    let mut capturing = false;
    // What each side asks for, and what runs (DEC-18).
    let (mut aprs, mut pager, mut running) = (DecoderKind::Off, false, DecoderKind::Off);
    // LINK-13.4: receive audio for the PC, coded from the same samples the decoder gets.
    let mut listen = false;
    let mut pack = ferrite_core::rxaudio::Packer::new();
    let mut halves = 0u32;
    loop {
        // TERM-33's signal only in a `tx` bench build: the others never wait on it, nor carry its generators.
        #[cfg(all(feature = "tx", feature = "bench"))]
        let test = TXTEST.wait();
        #[cfg(not(all(feature = "tx", feature = "bench")))]
        let test = core::future::pending::<Option<core::convert::Infallible>>();
        match select4(DSP.receive(), audio::ADC_HALF.wait(), AFSK_TX.wait(), test).await {
            Either4::First(cmd @ (DspCmd::Decoder(_) | DspCmd::Pager(_) | DspCmd::Audio(_))) => {
                match cmd {
                    DspCmd::Decoder(k) => aprs = k,
                    DspCmd::Pager(on) => pager = on,
                    DspCmd::Audio(on) => {
                        if on && !listen {
                            pack = ferrite_core::rxaudio::Packer::new();
                        }
                        listen = on;
                    }
                    _ => {}
                }
                let kind = match (aprs, pager) {
                    (DecoderKind::Off, true) => DecoderKind::Pocsag,
                    (k, _) => k,
                };
                if kind != running {
                    running = kind;
                    slot = match kind {
                        DecoderKind::Off => Decoder::Off,
                        DecoderKind::Afsk => Decoder::afsk(),
                        DecoderKind::Pocsag => Decoder::pocsag(),
                        DecoderKind::Dsc => Decoder::dsc(),
                    };
                }
                let want = kind != DecoderKind::Off || listen;
                if want && !capturing {
                    clock::for_audio().await; // PWR-26.4: the speed first, never under a running capture
                    adc::init();
                    dec = Decim::new();
                    audio::capture_start();
                } else if !want && capturing {
                    audio::capture_stop();
                }
                capturing = want;
            }
            Either4::First(DspCmd::Beep { .. }) => {} // beeps are the BK4829's (SND-11)
            Either4::Second(half) => {
                // PWR-26.4: a capture that started at 24 MHz (the governor didn't answer in time) gets the build's
                // speed: stopped, the speed asked for, started again; looked at every 10 s.
                halves = halves.wrapping_add(1);
                if halves.is_multiple_of(RECHECK_HALVES) && clock::is_low() && clock::AUDIO_RAISES.load(Relaxed) {
                    audio::capture_stop();
                    clock::for_audio().await;
                    dec = Decim::new();
                    audio::capture_start();
                    continue;
                }
                let base = half as usize * BLOCK_IN;
                let mut out = [0i16; BLOCK_OUT];
                let mut n = 0;
                for s in &audio::ADC_BUF[base..base + BLOCK_IN] {
                    dec.push(s.load(Relaxed), |x| {
                        if n < BLOCK_OUT {
                            out[n] = x;
                            n += 1;
                        }
                    });
                }
                if listen {
                    for &x in &out[..n] {
                        pack.push(x, |b| {
                            AUDIO_MADE.fetch_add(1, Relaxed);
                            if AUDIO_BLOCKS.try_send(*b).is_err() {
                                AUDIO_DROPS.fetch_add(1, Relaxed); // a full queue drops the newest: the PC hears a gap
                            }
                        });
                    }
                }
                slot.push(&out[..n], |f| match f {
                    Found::Frame(b) => {
                        FRAMES.fetch_add(1, Relaxed);
                        let mut fr = Frame { len: b.len().min(FRAME_MAX) as u16, txdelay_10ms: 0, fx25: 0, level: 1, b: [0; FRAME_MAX] };
                        fr.b[..fr.len as usize].copy_from_slice(&b[..fr.len as usize]);
                        if APRS_RX.try_send(Heard::Frame(fr)).is_err() {
                            DROPPED.fetch_add(1, Relaxed);
                        }
                    }
                    Found::Fx25(b) => {
                        if APRS_RX.try_send(Heard::Fx25(*b)).is_err() {
                            DROPPED.fetch_add(1, Relaxed);
                        }
                    }
                    Found::Page(m) => {
                        if PAGES.try_send(Page::from(m)).is_err() {
                            DROPPED.fetch_add(1, Relaxed);
                        }
                    }
                    Found::Call(_) => {} // DSC: roadmap 6c
                });
            }
            Either4::Fourth(None) => {} // a stop with nothing playing
            #[cfg(not(all(feature = "tx", feature = "bench")))]
            Either4::Fourth(Some(never)) => match never {},
            #[cfg(all(feature = "tx", feature = "bench"))]
            Either4::Fourth(Some(g)) => {
                if capturing {
                    audio::capture_stop();
                }
                play_test(g).await;
                if capturing {
                    clock::for_audio().await;
                    dec = Decim::new();
                    audio::capture_start();
                }
            }
            Either4::Third(frame) => {
                if capturing {
                    audio::capture_stop();
                }
                play(&frame).await;
                if capturing {
                    clock::for_audio().await;
                    dec = Decim::new();
                    audio::capture_start();
                }
                AFSK_DONE.signal(true);
            }
        }
    }
}

/// One frame out of DAC1: both halves filled, then each half refilled as it goes out; mid-scale after the last
/// sample, and stopped once that has played too.
async fn play(f: &Frame) {
    // The TX delay's flags first: the transmitter's key-up fits in them. FX.25 (APRS-45) when the frame asks for it.
    let cfg = afsk_tx_at(f.txdelay_10ms as u16 * 10, f.level);
    let mut enc = None;
    let mut m = uafsk::Modulator::from_bits(tx_bits(f.bytes(), f.fx25, &cfg, &mut enc), cfg);
    let fill = |half: usize, m: &mut uafsk::Modulator<TxBits>| -> bool {
        let mut more = true;
        for d in &audio::DAC_BUF[half * BLOCK_DAC..(half + 1) * BLOCK_DAC] {
            let s = if more { m.next() } else { None };
            more &= s.is_some();
            d.store(to_dac(s.unwrap_or(0)), Relaxed);
        }
        more
    };
    let mut more = fill(0, &mut m) && fill(1, &mut m);
    clock::for_audio().await; // PWR-26.4
    audio::DAC_HALF.reset();
    audio::play_start();
    // After the modulator ends: one half of silence written, then wait for it to play.
    let mut tail = 2;
    while tail > 0 {
        let half = audio::DAC_HALF.wait().await as usize;
        if more {
            more = fill(half, &mut m);
        } else {
            fill(half, &mut m);
            tail -= 1;
        }
    }
    audio::play_stop();
}

/// TERM-33, TERM-34: the test signal out of DAC1 until [`TXTEST`] says stop (None); a new generator (Some) takes
/// over from the next half filled, DAC1 still running (the suite's steps). Each generator loops or ends in silence
/// by itself. Then a half of silence, and stopped once that has played.
#[cfg(all(feature = "tx", feature = "bench"))]
async fn play_test(mut g: ferrite_core::txtest::Gen) {
    let fill = |half: usize, g: Option<&mut ferrite_core::txtest::Gen>| match g {
        Some(g) => {
            for d in &audio::DAC_BUF[half * BLOCK_DAC..(half + 1) * BLOCK_DAC] {
                d.store(to_dac(g.sample()), Relaxed);
            }
        }
        None => {
            for d in &audio::DAC_BUF[half * BLOCK_DAC..(half + 1) * BLOCK_DAC] {
                d.store(to_dac(0), Relaxed);
            }
        }
    };
    fill(0, Some(&mut g));
    fill(1, Some(&mut g));
    clock::for_audio().await; // PWR-26.4
    audio::DAC_HALF.reset();
    audio::play_start();
    loop {
        use embassy_futures::select::{select, Either};
        match select(audio::DAC_HALF.wait(), TXTEST.wait()).await {
            Either::First(half) => fill(half as usize, Some(&mut g)),
            Either::Second(Some(next)) => g = next,
            Either::Second(None) => break,
        }
    }
    // Both halves silent, so whichever plays next is mid-scale; then off.
    for _ in 0..2 {
        let half = audio::DAC_HALF.wait().await as usize;
        fill(half, None);
    }
    audio::play_stop();
}
