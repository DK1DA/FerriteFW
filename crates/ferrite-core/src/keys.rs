//! Keys. The matrix and direct keys use stock's key codes 0-21 (`analysis/keypad.md`), scanned by uradio's
//! `keypad` crate with the tables below; the side inputs get codes 24 up.
//!
//! The 20 matrix codes are the digit pad (12), the navigation pad (UP, DOWN, LEFT, RIGHT, MENU/OK, BACK) and the
//! dedicated SLOT (A/B/C) and V/M keys. Which code is which still needs the radio (the key test); until then only
//! the inputs whose role is known map to a [`Key`].

/// Stock's code table (0x0802F706 in V0.27). The ground row: keys from the sense lines straight to ground.
pub const GROUND_ROW: [u8; 4] = [10, 14, 18, 13];
/// Rows with PC0..PC3 driven low; columns PD7, PD6, PD5, PD4.
pub const ROWS: [[u8; 4]; 4] = [[6, 3, 9, 2], [12, 16, 20, 21], [11, 15, 19, 17], [7, 8, 4, 5]];
/// PE5 and PA12, read before the matrix.
pub const DIRECT: [u8; 2] = [0, 1];

pub const CODE_PTT_A: u8 = 24; // PE3
pub const CODE_PTT_B: u8 = 25; // PE2
pub const CODE_PTT_VFO: u8 = 26; // PE6, likely the side connector's PTT
pub const CODES: usize = 27;

/// What a key means to the UI: the same key means the same thing on every screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// MENU/OK.
    Menu,
    /// BACK.
    Exit,
    Up,
    Down,
    Left,
    Right,
    /// The dedicated A/B/C key: next slot.
    Slot,
    /// The dedicated VFO/memory key.
    VfoMem,
    Digit(u8),
    Star,
    Hash,
    /// PTT A, B or the connector's: transmit is the Radio task's call (and needs the tx build).
    Ptt,
    Side(u8),
}

/// Key code to meaning. The matrix entries are filled in from the key test on the radio.
pub const MAP: [Option<Key>; CODES] = {
    let mut m = [None; CODES];
    m[CODE_PTT_A as usize] = Some(Key::Ptt);
    m[CODE_PTT_B as usize] = Some(Key::Ptt);
    m[CODE_PTT_VFO as usize] = Some(Key::Ptt);
    m
};

pub fn key(code: u8) -> Option<Key> {
    MAP.get(code as usize).copied().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_once() {
        let mut seen = 0u32;
        for c in GROUND_ROW.iter().chain(ROWS.iter().flatten()).chain(&DIRECT) {
            assert_eq!(seen >> c & 1, 0, "code {c} twice");
            seen |= 1 << c;
        }
        assert_eq!(seen, 0x003F_FFFF, "codes 0-21");
        assert_eq!(key(CODE_PTT_B), Some(Key::Ptt));
        assert_eq!(key(99), None);
    }
}
