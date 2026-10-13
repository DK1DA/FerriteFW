//! A path plan carried out on the hardware (SAFE-11): every path select, the PA drive and the band relay change
//! only here, in the plan's order, waits included, before anything else reaches the pins.

use ferrite_core::path::{Act, Sel, Steps};

/// The pins a plan moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin {
    /// One of the six RF path selects (the board's `RF_PINS`, in [`Sel`]'s order).
    Sel(Sel),
    /// The PA drive pin (PA5) as a plain output: the receive-only build holds it low (no DAC there).
    PaDrive,
    /// The band relay (PC4): high energises it (chip 1's HF path).
    Relay,
    /// The SI4732's input path pins (PA7, PC5).
    SiIn(u8),
    /// Stock's crossband audio select (PA15): low routes the receiving chip's audio into the TX chip's mic input.
    RepeatAudio,
    /// The speaker amplifier and the shared audio stage (PB8, PE4).
    Amp(u8),
}

/// What [`run`] needs from the radio: its pins, a wait, and the chip-level steps the Radio task owns. The
/// firmware implements it on its Radio task; the tests on a recording mock.
#[allow(async_fn_in_trait)]
pub trait Hw {
    /// Drive a pin.
    fn pin(&mut self, p: Pin, high: bool);
    /// The band relay's pin as it is now (for the log line on a move).
    fn relay_is(&mut self) -> bool;
    /// Wait this long before the next step.
    async fn wait_ms(&mut self, ms: u32);
    /// The receiver's audio muted (the speaker is switched off by [`run`]).
    fn muted(&mut self, m: bool);
    /// Whether the SI4732 has been started since boot (BCR-20: only then does leaving it set 1/0).
    fn si_used(&mut self) -> bool;
    fn set_si_used(&mut self);
    /// A line on the bring-up build's cable log.
    fn log(&mut self, _tag: &[u8], _v: u32) {}
    /// The PA drive (DAC2) to 0.
    #[cfg(feature = "tx")]
    fn pa_zero(&mut self);
    /// The PA drive to this DAC value.
    #[cfg(feature = "tx")]
    fn pa_set(&mut self, v: u8);
    /// The calibrated PA level for this transmission (`ferrite_core::txcal`).
    #[cfg(feature = "tx")]
    fn pa_level(&mut self) -> u8;
    /// Key chip `n` with its read-back (TX-7): false stops the plan (no PA ramp), the caller goes safe next.
    #[cfg(feature = "tx")]
    fn key_chip(&mut self, n: u8) -> bool;
    /// The read-back failed: the caller sees it after [`run`].
    #[cfg(feature = "tx")]
    fn chip_fault(&mut self, n: u8);
    /// Chip `n` back to receive.
    #[cfg(feature = "tx")]
    fn unkey_chip(&mut self, n: u8);
    /// While keyed, at the unkey (TONE-8): the end tone, the CW ID, tail elimination.
    #[cfg(feature = "tx")]
    async fn end_tone(&mut self);
    #[cfg(feature = "tx")]
    async fn cw_id(&mut self);
    #[cfg(feature = "tx")]
    async fn tail_elim(&mut self);
}

/// TX-23's ramp: the calibrated level in this many equal steps, 1 ms apart.
#[cfg(feature = "tx")]
pub const PA_RAMP_STEPS: u32 = 4;

/// Carry out a path plan, waits included, step by step in its order.
pub async fn run<H: Hw>(hw: &mut H, steps: &Steps) {
    for &a in steps.as_slice() {
        match a {
            Act::Sel(s, on) => {
                hw.pin(Pin::Sel(s), on);
                // Transmit selects only: the receive select hops with dual watch many times a second and
                // flooded the bring-up log on the radio (Oct 12, 2026).
                if on && s.is_tx() {
                    hw.log(b"path select ", s as u32);
                }
            }
            Act::PaZero => {
                #[cfg(feature = "tx")]
                hw.pa_zero();
                #[cfg(not(feature = "tx"))]
                hw.pin(Pin::PaDrive, false);
            }
            Act::Relay(on) => {
                if on != hw.relay_is() {
                    hw.log(b"relay ", on as u32);
                }
                hw.pin(Pin::Relay, on);
            }
            // BCR-20, stock's states: the mode's own state is set when the SI4732 is started (`chip::si_input`);
            // away from it, 1/0 once it has been used, as stock leaves broadcast. Boot and the safe state before
            // any broadcast keep stock's 0/0.
            Act::SiInput(true) => hw.set_si_used(),
            Act::SiInput(false) => {
                if hw.si_used() {
                    hw.pin(Pin::SiIn(0), true);
                    hw.pin(Pin::SiIn(1), false);
                }
            }
            Act::Mute(m) => {
                hw.muted(m);
                if m {
                    hw.pin(Pin::Amp(0), false);
                    hw.pin(Pin::Amp(1), false);
                }
            }
            Act::Wait(ms) => hw.wait_ms(ms as u32).await,
            // XB-6, XB-8, decision 95: stock's crossband route at its PTT state 1, PA15 low with PB8 set
            // (0x080248C2-0x080248D8); PA15 high again at the receive drop, still keyed (0x0801F534).
            Act::RepeatAudio(on) => {
                hw.pin(Pin::RepeatAudio, !on);
                if on {
                    hw.pin(Pin::Amp(0), true);
                }
                hw.log(b"repeat audio ", on as u32);
            }
            #[cfg(feature = "tx")]
            Act::ChipTx(n) => {
                if !hw.key_chip(n) {
                    hw.chip_fault(n);
                    hw.log(b"tx read-back failed, chip ", n as u32);
                    break; // no PA ramp: the caller puts the safe state next
                }
            }
            #[cfg(feature = "tx")]
            Act::ChipRx(n) => hw.unkey_chip(n),
            #[cfg(feature = "tx")]
            Act::PaRamp => {
                let v = hw.pa_level() as u32;
                for k in 1..=PA_RAMP_STEPS {
                    hw.pa_set((v * k / PA_RAMP_STEPS) as u8);
                    hw.wait_ms(1).await;
                }
                hw.log(b"tx pa ", v);
            }
            // TONE-8's order at the unkey, still keyed (the sequencer plans each only when its setting is on).
            #[cfg(feature = "tx")]
            Act::EndTone => hw.end_tone().await,
            #[cfg(feature = "tx")]
            Act::CwId => hw.cw_id().await,
            #[cfg(feature = "tx")]
            Act::TailElim => hw.tail_elim().await,
            // Transmit steps: only a key-up plans them, and a receive-only build never asks for one.
            #[cfg(not(feature = "tx"))]
            Act::ChipTx(_) | Act::ChipRx(_) | Act::PaRamp | Act::EndTone | Act::CwId | Act::TailElim => {}
        }
    }
}
