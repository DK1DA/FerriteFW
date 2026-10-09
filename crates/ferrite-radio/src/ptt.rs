//! SAFE-14: a PTT held at power-on is ignored until released: no transmission until every PTT input (PTT A, PTT B,
//! the connector's) has read released once after boot.

/// The guard; starts closed at boot.
#[derive(Clone, Copy, Debug, Default)]
pub struct PttGuard {
    ok: bool,
}

impl PttGuard {
    /// One look at the PTT inputs (`all_released`: every one reads released now). True from the first look with
    /// all of them released, for the rest of the power-on.
    pub fn seen(&mut self, all_released: bool) -> bool {
        self.ok |= all_released;
        self.ok
    }
}
