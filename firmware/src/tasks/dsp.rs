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
//! - **Transmit test (TERM-33, `tx` builds):** the Radio task, after a PTT key-up with `txtest` armed, signals
//!   [`TXTEST`] with the signal; it plays (generated a sample at a time, [`ferrite_core::txtest::Gen`]) until the
//!   Radio task signals None at the release. Receiving pauses while it plays, as for AFSK.

use crate::chan::DSP;
use crate::hal::{adc, audio};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use ferrite_app::aprs_live::FRAME_MAX;
use ferrite_core::audio::{afsk_tx, to_dac, uafsk, Decim, BLOCK_DAC, BLOCK_IN, BLOCK_OUT};
use ferrite_core::decoders::{Decoder, Found, Page};
use ferrite_core::msg::{DecoderKind, DspCmd};

/// An AX.25 frame without its FCS, as the demodulator gives it and the modulator takes it.
#[derive(Clone, Copy)]
pub struct Frame {
    pub len: u16,
    /// Transmit: the flags ahead of it, in 10 ms (KISS's TX delay; 30 = 300 ms, KISS-2's default).
    pub txdelay_10ms: u8,
    pub b: [u8; FRAME_MAX],
}

impl Frame {
    pub fn bytes(&self) -> &[u8] {
        &self.b[..self.len as usize]
    }
}

/// Frames heard, to the UI task's APRS task: two deep (a second frame waits while the first is read; 660 B).
pub static APRS_RX: Channel<M, Frame, 2> = Channel::new();
/// A frame to send on the transmission the UI task just keyed, and its "sent" (true) or "not sent" (false).
pub static AFSK_TX: Signal<M, Frame> = Signal::new();
pub static AFSK_DONE: Signal<M, bool> = Signal::new();
/// TERM-33: the transmit test to play now (Some, from the key-up) or to stop (None, at the release).
#[cfg(feature = "tx")]
pub static TXTEST: Signal<M, Option<ferrite_core::txtest::Signal>> = Signal::new();
/// Pages heard, to the UI task (DEC-7): two deep, 144 B.
pub static PAGES: Channel<M, Page, 2> = Channel::new();
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
    loop {
        // TERM-33's signal only in a `tx` build: the receive build never waits on it, nor carries its generators.
        #[cfg(feature = "tx")]
        let test = TXTEST.wait();
        #[cfg(not(feature = "tx"))]
        let test = core::future::pending::<Option<ferrite_core::txtest::Signal>>();
        match select4(DSP.receive(), audio::ADC_HALF.wait(), AFSK_TX.wait(), test).await {
            Either4::First(cmd @ (DspCmd::Decoder(_) | DspCmd::Pager(_))) => {
                match cmd {
                    DspCmd::Decoder(k) => aprs = k,
                    DspCmd::Pager(on) => pager = on,
                    _ => {}
                }
                let kind = match (aprs, pager) {
                    (DecoderKind::Off, true) => DecoderKind::Pocsag,
                    (k, _) => k,
                };
                if kind == running {
                    continue;
                }
                running = kind;
                slot = match kind {
                    DecoderKind::Off => Decoder::Off,
                    DecoderKind::Afsk => Decoder::afsk(),
                    DecoderKind::Pocsag => Decoder::pocsag(),
                    DecoderKind::Dsc => Decoder::dsc(),
                };
                let want = kind != DecoderKind::Off;
                if want && !capturing {
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
                slot.push(&out[..n], |f| match f {
                    Found::Frame(b) => {
                        FRAMES.fetch_add(1, Relaxed);
                        let mut fr = Frame { len: b.len().min(FRAME_MAX) as u16, txdelay_10ms: 0, b: [0; FRAME_MAX] };
                        fr.b[..fr.len as usize].copy_from_slice(&b[..fr.len as usize]);
                        if APRS_RX.try_send(fr).is_err() {
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
            Either4::Fourth(Some(sig)) => {
                if capturing {
                    audio::capture_stop();
                }
                play_test(sig).await;
                if capturing {
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
    // The TX delay's flags first: the transmitter's key-up fits in them.
    let mut m = uafsk::Modulator::new(f.bytes(), afsk_tx(f.txdelay_10ms as u16 * 10));
    let fill = |half: usize, m: &mut uafsk::Modulator| -> bool {
        let mut more = true;
        for d in &audio::DAC_BUF[half * BLOCK_DAC..(half + 1) * BLOCK_DAC] {
            let s = if more { m.next() } else { None };
            more &= s.is_some();
            d.store(to_dac(s.unwrap_or(0)), Relaxed);
        }
        more
    };
    let mut more = fill(0, &mut m) && fill(1, &mut m);
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

/// TERM-33: the test signal out of DAC1 until [`TXTEST`] says stop (None; a new Some stops it too), looped by the
/// generator itself; then a half of silence, and stopped once that has played.
#[cfg(feature = "tx")]
async fn play_test(sig: ferrite_core::txtest::Signal) {
    let mut g = ferrite_core::txtest::Gen::new(sig);
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
    audio::DAC_HALF.reset();
    audio::play_start();
    loop {
        use embassy_futures::select::{select, Either};
        match select(audio::DAC_HALF.wait(), TXTEST.wait()).await {
            Either::First(half) => fill(half as usize, Some(&mut g)),
            Either::Second(_) => break,
        }
    }
    // Both halves silent, so whichever plays next is mid-scale; then off.
    for _ in 0..2 {
        let half = audio::DAC_HALF.wait().await as usize;
        fill(half, None);
    }
    audio::play_stop();
}

#[cfg(not(feature = "tx"))]
async fn play_test(_: ferrite_core::txtest::Signal) {}
