//! The terminal's debug requests.

use super::*;

impl Radio {
    /// The terminal's debug requests (TERM-21 to TERM-24): registers read and written as asked, but never a
    /// transmit or PA bit, nor the chip's GPIO outputs (`bk_write_refused`, checked here as well as in the
    /// terminal). Memory requests are the UI task's.
    pub(super) fn debug(&mut self, d: DebugReq) -> DebugAns {
        match d {
            DebugReq::BkRead { chip, .. } | DebugReq::BkWrite { chip, .. } if chip > 1 || self.chips & 1 << chip == 0 => {
                DebugAns::Refused("that BK4829 didn't answer at start-up")
            }
            DebugReq::BkRead { chip, reg } if reg < 0x80 => DebugAns::Reg { chip, reg, val: self.chip(chip).read(reg) },
            DebugReq::BkWrite { chip, reg, val } => {
                if let Some(why) = bk_write_refused(reg, val) {
                    return DebugAns::Refused(why);
                }
                self.chip(chip).write(reg, val);
                DebugAns::Reg { chip, reg, val: self.chip(chip).read(reg) }
            }
            // TERM-31, PWR-21: the settle waits, set or asked; from the next wake or sleep on.
            DebugReq::Settle(set) => {
                if let Some(set) = set {
                    self.settle = set;
                }
                DebugAns::Settle(self.settle)
            }
            // TERM-32 (a bench test): PA0, the mic level tap VOX reads, at 8 kHz for 32 ms, to tell raw audio from an
            // envelope. Through the ADC's preempted group, as VOX; the Radio task waits through it.
            DebugReq::Mic => {
                use crate::hal::{adc, clock, time::cycles};
                use ferrite_core::vox::{MicCapture, MIC_PERIOD_US, MIC_SAMPLES};
                adc::init();
                let mut s = [0u16; MIC_SAMPLES];
                let per = MIC_PERIOD_US * clock::mhz();
                let start = cycles();
                for (i, v) in s.iter_mut().enumerate() {
                    while cycles().wrapping_sub(start) < i as u32 * per {}
                    *v = adc::read(adc::CH_PA0);
                }
                let us = cycles().wrapping_sub(start) / clock::mhz();
                DebugAns::Mic(MicCapture::from_samples(&s, us))
            }
            // TERM-33 (decision 132, a bench test): armed for the next PTT key-up, once; the key-up plays it.
            // TERM-34: the suite needs a TX timeout it fits in; asked while it runs, its progress.
            #[cfg(feature = "tx")]
            DebugReq::TxTest(ask) => {
                use ferrite_core::txtest::{Ask, Signal};
                match ask {
                    Ask::Set(Some(a)) if a.signal == Signal::Suite && self.tx.timeout_s < ferrite_core::txsuite::TIMEOUT_MIN_S => {
                        return DebugAns::Refused("the suite runs 60 s: set the TX timeout (0 8 1) to 90 s or more first");
                    }
                    Ask::Set(a) => {
                        self.tx.test = a;
                        log(b"txtest armed ", a.map_or(0, |a| a.signal as u32 + 1));
                    }
                    Ask::Show => {
                        if let Some((section, left_ms)) = self.suite_status() {
                            return DebugAns::TxSuite { section, left_ms };
                        }
                    }
                }
                DebugAns::TxTest(self.tx.test)
            }
            #[cfg(not(feature = "tx"))]
            DebugReq::TxTest(_) => DebugAns::Refused("the receive-only build has no transmit: txtest needs the tx-bench build"),
            DebugReq::Si { cmd, n, reply } => {
                let (n, rn) = ((n as usize).min(8), (reply as usize).clamp(1, 16));
                // TERM-24: powered down (and in reset) outside FM broadcast and HF mode: nothing is sent then.
                if self.si.mode().is_none() {
                    return DebugAns::Refused("the SI4732 is powered down: FM broadcast or HF mode powers it up");
                }
                let mut data = [0u8; 16];
                match self.si.command(&cmd[..n], &mut data[..rn], &mut BusyDelay) {
                    Ok(()) => DebugAns::Si { data, n: rn as u8 },
                    Err(_) => DebugAns::Refused("the SI4732 didn't answer the command"),
                }
            }
            _ => DebugAns::Refused("not a Radio task request"),
        }
    }

    /// TERM-29, SAFE-24 (decision 114, a bench test): hold both band receive selects on (VHF RX PB0 and UHF RX
    /// PE12), back to one, or ask. Refused while keyed, in HF mode (its path and the relay) and while the crossband
    /// repeater runs (it owns both chips' paths). RAM only: off at boot.
    pub(super) async fn rx_both(&mut self, on: Option<bool>) -> DebugAns {
        let Some(on) = on else { return DebugAns::RxBoth(self.path.both()) };
        if self.hf.is_some() {
            return DebugAns::Refused("HF mode is on: it has the HF path and the relay; leave HF mode first");
        }
        #[cfg(feature = "tx")]
        if self.xb.is_some() {
            return DebugAns::Refused("the crossband repeater is running: it owns both chips' paths");
        }
        let mut s = Steps::default();
        match self.path.set_both(on, &mut s) {
            Err(Blocked::Keyed) => DebugAns::Refused("transmitting: try again after the unkey"),
            Err(_) => DebugAns::Refused("an HF slot has the relay energised: tune VHF or UHF first"),
            Ok(()) => {
                self.run(&s).await;
                DebugAns::RxBoth(on)
            }
        }
    }
}
