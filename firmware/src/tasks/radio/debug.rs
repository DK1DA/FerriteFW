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
            DebugReq::Si { cmd, n, reply } => {
                let (n, rn) = ((n as usize).min(8), (reply as usize).clamp(1, 16));
                let mut data = [0u8; 16];
                match self.si.command(&cmd[..n], &mut data[..rn], &mut BusyDelay) {
                    Ok(()) => DebugAns::Si { data, n: rn as u8 },
                    Err(_) => DebugAns::Refused("the SI4732 didn't answer: it is held in reset until FM broadcast or HF mode starts it"),
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
