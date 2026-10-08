//! The APRS task (manual chapter 25, roadmap 5b-5d), run by the UI task beside the App and the store it feeds,
//! as the simulators run theirs: frames from the DSP task's decoder go to [`AprsLive`] (the heard list, messages
//! to us into the inbox and the log, acks), and once a second it may give a frame to send (an ack, a digipeat,
//! one of ours, a beacon), which goes out as a data transmission on slot C:
//!
//! 1. [`transmit::data_key`]: the automatic transmission's checks (TX-35, APRS-8, APRS-10), then the data key-up
//!    at the APRS power (0 5 5 4, default Low);
//! 2. once the Radio task says slot C is keyed, the frame to the DSP task ([`AFSK_TX`]), which plays it from the TX
//!    delay's flags (the key-up's ramp fits in them);
//! 3. when it's out ([`AFSK_DONE`]), the unkey. A key-up the Radio task refuses, or a frame that never finishes,
//!    unkeys after a timeout.
//!
//! The decoder runs while APRS mode is on (APRS-16), and without it while a slot on a channel flagged APRS is
//! the active one (APRS-22: decode only, nothing sent). Bluetooth KISS waits for roadmap 3d. The digipeater
//! follows 0 5 8 (off by default, APRS-12, APRS-26).

use super::dsp::{Frame, AFSK_DONE, AFSK_TX, APRS_RX};
use crate::chan::DSP;
use crate::store::Store;
use ferrite_app::aprs_live::uaprs::kiss;
use ferrite_app::aprs_live::FRAME_MAX;
use ferrite_app::data::{Data, Event, Src};
use ferrite_app::screens::transmit;
use ferrite_app::App;
use ferrite_core::msg::{DecoderKind, DspCmd, LiveAll};
use ferrite_core::txplan;
use ferrite_ui::text::Buf;

/// APRS mode's slot (APRS-15).
const SLOT_C: u8 = 2;
/// The Radio task keys within this long, or it refused.
const KEY_WAIT_MS: u32 = 1_500;
/// The longest frame (330 bytes at 1200 Bd, about 2.2 s) after up to 2.55 s of flags, with margin.
const PLAY_WAIT_MS: u32 = 6_000;
/// The cable port setting's KISS value (0 9 2 5: auto, link, terminal, KISS).
const PORT_KISS: u8 = 3;
/// KISS-2's TX delay default, 300 ms (the app may change it); the APRS task's own frames use it too.
const TXDELAY_10MS: u8 = 30;

enum Tx {
    Idle,
    /// Keyed for [`Aprs::frame`], waiting for the Radio task.
    Keying { at_ms: u32 },
    Playing { at_ms: u32 },
}

pub struct Aprs {
    on: bool,
    last_s: u32,
    tx: Tx,
    /// The frame being sent.
    frame: Frame,
    /// The KISS TNC (chapter 26): the cable's bytes in, the app's settings, a frame waiting to go, and its
    /// channel access.
    kiss: kiss::Decoder,
    params: kiss::Params,
    kiss_q: Option<Frame>,
    access: kiss::Access,
    seed: u32,
}

impl Aprs {
    pub fn new() -> Self {
        let params = kiss::Params { tx_delay: TXDELAY_10MS, ..kiss::Params::default() };
        Aprs { on: false, last_s: u32::MAX, tx: Tx::Idle, frame: Frame { len: 0, txdelay_10ms: 0, b: [0; FRAME_MAX] }, kiss: kiss::Decoder::new(), params, kiss_q: None, access: kiss::Access::default(), seed: 0x2545_F491 }
    }

    /// A byte from the cable while KISS has it (TERM-2's route).
    pub fn kiss_in(&mut self, b: u8) {
        let Some(r) = self.kiss.push(b) else { return };
        if let kiss::Command::Data(d) = self.params.apply(&r) {
            if self.kiss_q.is_none() && (17..=FRAME_MAX).contains(&d.len()) {
                let mut f = Frame { len: d.len() as u16, txdelay_10ms: self.params.tx_delay, b: [0; FRAME_MAX] };
                f.b[..d.len()].copy_from_slice(d);
                self.kiss_q = Some(f);
            }
        }
    }

    /// A random byte for p-persistence (xorshift).
    fn random(&mut self) -> u8 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as u8
    }

    /// A frame is being keyed or played: the UI task turns faster meanwhile.
    pub fn busy(&self) -> bool {
        !matches!(self.tx, Tx::Idle)
    }

    /// One turn of the UI task's loop.
    pub fn poll(&mut self, app: &mut App, store: &Store, live: &LiveAll, now_ms: u32) {
        let extra = &store.extra;
        extra.set_callsign(app.callsign());
        let kiss_on = app.settings().cable_port == PORT_KISS;
        let on = app.aprs_on();
        let decode = on || kiss_on || app.aprs_flag_decode(); // APRS-22
        if decode != self.on && DSP.try_send(DspCmd::Decoder(if decode { DecoderKind::Afsk } else { DecoderKind::Off })).is_ok() {
            self.on = decode;
        }
        let now_s = now_ms / 1000;
        let me: Buf<10> = Buf::from(app.callsign());
        // Frames heard (APRS-3, APRS-6).
        while let Ok(f) = APRS_RX.try_receive() {
            if kiss_on {
                kiss::encode(0, f.bytes(), |b| {
                    crate::chan::TERM_OUT.put(b); // KISS-4; a full ring drops the rest of the frame
                });
                crate::chan::TERM_WAKE.signal(());
            }
            let rx = extra.aprs.receive(f.bytes(), now_s, me.as_bytes(), &extra.cfg());
            if let Some((from, text)) = rx.inbound {
                app.notify(from.as_bytes(), text.as_bytes(), false);
                store.message_in(true, from.as_bytes(), text.as_bytes(), 0);
                store.log(&log_event(app.aprs_freq(), from.as_bytes(), text.as_bytes())); // INBOX-6
            }
            if let Some((peer, text, st)) = rx.state {
                store.message_state(peer.as_bytes(), text.as_bytes(), st);
            }
            if rx.changed || rx.state.is_some() {
                extra.changed();
            }
        }
        match &self.tx {
            Tx::Keying { at_ms } => {
                if live.tx.keyed && live.tx.slot == SLOT_C {
                    AFSK_TX.signal(self.frame);
                    self.tx = Tx::Playing { at_ms: now_ms };
                } else if now_ms.wrapping_sub(*at_ms) > KEY_WAIT_MS {
                    transmit::data_unkey(app, SLOT_C); // refused by the Radio task (its toast says why)
                    self.tx = Tx::Idle;
                }
            }
            Tx::Playing { at_ms } => {
                if AFSK_DONE.try_take().is_some() || now_ms.wrapping_sub(*at_ms) > PLAY_WAIT_MS {
                    transmit::data_unkey(app, SLOT_C);
                    self.tx = Tx::Idle;
                }
            }
            // KISS-2: the app's frame, exempt from the 30 s spacing, once the channel is clear (p-persistence).
            Tx::Idle if kiss_on && self.kiss_q.is_some() => {
                let busy = live.slots[SLOT_C as usize].squelch_open;
                let r = self.random();
                if self.access.may_send(&self.params, now_ms, busy, r) {
                    self.frame = self.kiss_q.take().unwrap();
                    if transmit::data_key(app, SLOT_C, extra.cfg().power, b"KISS") {
                        self.tx = Tx::Keying { at_ms: now_ms };
                    }
                }
            }
            Tx::Idle if now_s != self.last_s => {
                self.last_s = now_s;
                let relay_ok = txplan::relay_ok(app.tuned_freq(app.active()), app.aprs_freq());
                let cfg = extra.cfg();
                let mut out = [0u8; FRAME_MAX];
                let t = extra.aprs.tick(now_s, on, relay_ok, me.as_bytes(), extra.fix(), &cfg, &mut out);
                for (peer, text, st) in t.states.into_iter().flatten() {
                    store.message_state(peer.as_bytes(), text.as_bytes(), st);
                }
                if t.changed {
                    extra.changed();
                }
                if t.len > 0 {
                    if transmit::data_key(app, SLOT_C, cfg.power, t.what.as_bytes()) {
                        self.frame = Frame { len: t.len as u16, txdelay_10ms: self.params.tx_delay, b: [0; FRAME_MAX] };
                        self.frame.b[..t.len].copy_from_slice(&out[..t.len]);
                        self.tx = Tx::Keying { at_ms: now_ms };
                    }
                } else if t.what == "Beacon: no position" {
                    transmit::not_sent(app, b"Beacon", b"no position");
                }
            }
            Tx::Idle => {}
        }
    }
}

fn log_event(freq: ferrite_core::freq::Freq10, name: &[u8], text: &[u8]) -> Event {
    Event { src: Src::Aprs, ago_s: 0, freq, slot: SLOT_C, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(name), text: Buf::from(text) }
}
