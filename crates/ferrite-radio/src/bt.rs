//! The Bluetooth module's power, PB9 (LINK-14, LINK-25; the link task owns it).

/// What the link task does to PB9 on one look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BtOut {
    /// 0 9 2 1 changed: `Some(true)` powers the module (then waits for it before USART1 starts), `Some(false)`
    /// switches it and USART1 off.
    pub power: Option<bool>,
    /// Decision 76: PB9 set again, as stock does after each transmission while Bluetooth is on.
    pub reassert: bool,
}

/// The link task's Bluetooth state between looks.
#[derive(Clone, Copy, Debug, Default)]
pub struct BtPower {
    pub on: bool,
    keyed: bool,
}

impl BtPower {
    /// One look: `want` is 0 9 2 1, `keyed` while any chip transmits.
    pub fn step(&mut self, want: bool, keyed: bool) -> BtOut {
        let mut out = BtOut::default();
        if want != self.on {
            self.on = want;
            out.power = Some(want);
        }
        out.reassert = self.keyed && !keyed && self.on;
        self.keyed = keyed;
        out
    }
}
