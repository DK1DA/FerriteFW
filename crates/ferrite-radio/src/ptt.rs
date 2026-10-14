//! SAFE-14: a PTT held at power-on is ignored until released: no transmission until every PTT input (PTT A, PTT B,
//! the connector's) has read released once after boot; one held longer than [`STUCK_MS`] from boot is reported
//! stuck, by name, until it is let go.

/// A PTT held this long from boot is stuck (SAFE-14).
pub const STUCK_MS: u32 = 1000;
pub use ferrite_core::stuck::{names, PTT_A, PTT_B, PTT_EXT};

/// The guard; starts closed at boot.
#[derive(Clone, Copy, Debug, Default)]
pub struct PttGuard {
    ok: bool,
    /// The first look happened; the inputs held since boot and never released.
    looked: bool,
    held: u8,
}

impl PttGuard {
    /// One look at the PTT inputs: `pressed` the [`PTT_A`], [`PTT_B`], [`PTT_EXT`] bits pressed now. True from the
    /// first look with all of them released, for the rest of the power-on.
    pub fn look(&mut self, pressed: u8) -> bool {
        self.held = if self.looked { self.held & pressed } else { pressed };
        self.looked = true;
        self.ok |= pressed == 0;
        self.ok
    }

    /// Transmit allowed (every input read released once since boot).
    pub fn open(&self) -> bool {
        self.ok
    }

    /// The inputs held since boot for [`STUCK_MS`] or more at `now` and not let go since: shown as "PTT stuck".
    pub fn stuck(&self, now: u32) -> u8 {
        if now >= STUCK_MS {
            self.held
        } else {
            0
        }
    }
}
