//! The GPS module's power (PA8) and PB10 (manual chapter 28; the GPS task owns both, decision 76).
//!
//! - **PB10 while keyed (GPS-5, GPS-22):** USART3's TX pin is driven low while any chip transmits and handed back
//!   to the UART after, as stock does (0x080252D8 / 0x0802530C).
//! - **Power (GPS-1, GPS-4):** on or off with 0 6 1, and off around signals as 0 6 8 asks; after a signal the
//!   module comes back [`QUIET_MS`] later, so a burst of short signals doesn't cycle it.

/// After the last signal that wanted the module off, it powers again this long later (GPS-25).
pub const QUIET_MS: u32 = 2000;

/// What the GPS task does to the pins on one look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpsOut {
    /// PB10: `Some(true)` driven low (keyed), `Some(false)` back to the UART.
    pub pb10_low: Option<bool>,
    /// The module's power changes: `Some(true)` on (PA8 low), `Some(false)` off.
    pub power: Option<bool>,
}

/// The GPS task's power state between looks.
#[derive(Clone, Copy, Debug, Default)]
pub struct GpsPower {
    pub powered: bool,
    tx_low: bool,
    busy_at: u32,
}

impl GpsPower {
    /// The Radio task drove PB10 low itself before a key-up (GPS-22, stock's PTT press 0x0801C6D4; TX audit F15):
    /// the next look hands it back to the UART unless the radio is keyed by then (a refused key-up).
    pub fn forced_low(&mut self) {
        self.tx_low = true;
    }

    /// One look at `now`: `on` is 0 6 1; `allowed` is 0 6 8's answer for what the radio is doing now (false
    /// while a signal it covers is on); `keyed` while any chip transmits.
    pub fn step(&mut self, now: u32, on: bool, allowed: bool, keyed: bool) -> GpsOut {
        let mut out = GpsOut::default();
        if keyed != self.tx_low {
            out.pb10_low = Some(keyed);
            self.tx_low = keyed;
        }
        if !allowed {
            self.busy_at = now;
        }
        let want = on && (!self.powered && now.wrapping_sub(self.busy_at) >= QUIET_MS || self.powered && allowed);
        if want != self.powered {
            out.power = Some(want);
            self.powered = want;
        }
        out
    }
}
