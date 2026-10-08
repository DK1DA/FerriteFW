//! DSP: audio from ADC1 (PA2, the BK4829s' EARO) through the 4:1 step into the one decoder slot, and AFSK out
//! of DAC1 (APRS-1, APRS-14; roadmap 2f, 5b, 5c). Runs on the second interrupt executor, below Radio.
//!
//! - **Receive:** while a decoder runs, the capture runs ([`hal::audio`]); each 2 ms half goes through
//!   [`Decim`] to the slot. An AFSK frame with a good CRC goes to the UI task's APRS task through [`APRS_RX`].
//! - **Transmit:** the UI task keys slot C, then hands a frame to [`AFSK_TX`]; it is played from TX delay's
//!   flags to the end (the flags cover the key-up), PC12 high meanwhile, and [`AFSK_DONE`] says it's out so the
//!   UI task unkeys. Receiving pauses while it plays.

use crate::chan::DSP;
use crate::hal::{adc, audio};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use embassy_futures::select::{select3, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use ferrite_app::aprs_live::FRAME_MAX;
use ferrite_core::audio::{afsk_tx, to_dac, uafsk, Decim, BLOCK_DAC, BLOCK_IN, BLOCK_OUT};
use ferrite_core::decoders::{Decoder, Found};
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
/// Frames decoded and dropped (the UI hadn't taken the last two), for the bench.
pub static FRAMES: AtomicU32 = AtomicU32::new(0);
pub static DROPPED: AtomicU32 = AtomicU32::new(0);


#[embassy_executor::task]
pub async fn task() {
    // The slot lives in this task's future, which is a static: one decoder's memory, whichever is running.
    let mut slot = Decoder::Off;
    let mut dec = Decim::new();
    let mut capturing = false;
    loop {
        match select3(DSP.receive(), audio::ADC_HALF.wait(), AFSK_TX.wait()).await {
            Either3::First(DspCmd::Decoder(kind)) => {
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
            Either3::First(DspCmd::Beep { .. }) => {} // beeps are the BK4829's (SND-11)
            Either3::Second(half) => {
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
                slot.push(&out[..n], |f| {
                    if let Found::Frame(b) = f {
                        FRAMES.fetch_add(1, Relaxed);
                        let mut fr = Frame { len: b.len().min(FRAME_MAX) as u16, txdelay_10ms: 0, b: [0; FRAME_MAX] };
                        fr.b[..fr.len as usize].copy_from_slice(&b[..fr.len as usize]);
                        if APRS_RX.try_send(fr).is_err() {
                            DROPPED.fetch_add(1, Relaxed);
                        }
                    }
                });
            }
            Either3::Third(frame) => {
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
