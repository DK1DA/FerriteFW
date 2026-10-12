//! Keys. The matrix and direct keys use stock's key codes 0-21 (`analysis/keypad.md`), scanned by uradio's
//! `keypad` crate with the tables below; the side inputs get codes 24 up.
//!
//! The 20 matrix codes are the digit pad (12), the navigation pad (UP, DOWN, LEFT, RIGHT, MENU/OK, BACK) and the
//! dedicated SLOT (A/B/C) and V/M keys, read off the radio's key test on the black radio (Oct 12, 2026). They
//! follow the matrix: the ground row is the 1/4/7/* column, and stock's notes agree (6 MENU, 3 A/B/C, 9 BACK).

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// PTT: [`PTT_A`] and [`PTT_ACTIVE`] (the side connector's) key the active slot, [`PTT_B`] keys slot B
    /// (KEY-7, decision 77: there is no PTT C). Transmit is the Radio task's call (and needs the tx build).
    Ptt(u8),
    /// The side keys: 1 = SIDE1 (PF1, PE5: monitor, squelch, clarifier), 2 = SIDE2 (PF2, PA12: scan, big steps,
    /// held alone the frequency counter).
    Side(u8),
}

/// `Key::Ptt` for PTT A (PE3): keys the active slot, as nicFW's PTT does (KEY-7).
pub const PTT_A: u8 = 0;
/// `Key::Ptt` for PTT B (PE2): keys slot B (KEY-7).
pub const PTT_B: u8 = 1;
/// `Key::Ptt` for the side connector's PTT: the active slot (KEY-7).
pub const PTT_ACTIVE: u8 = 0xFF;

impl Key {
    /// The side connector's PTT: transmits on the active slot.
    pub const PTT: Key = Key::Ptt(PTT_ACTIVE);
}

/// Key code to meaning. The front keys from the key test on the radio (Oct 12, 2026).
pub const MAP: [Option<Key>; CODES] = {
    let mut m = [None; CODES];
    // Matrix (ROWS): MENU, A/B/C, BACK, V/M; 3, 6, 9, #; 2, 5, 8, 0; UP, DOWN, LEFT, RIGHT. Ground row: 1, 4, 7, *.
    m[6] = Some(Key::Menu);
    m[3] = Some(Key::Slot);
    m[9] = Some(Key::Exit);
    m[2] = Some(Key::VfoMem);
    m[12] = Some(Key::Digit(3));
    m[16] = Some(Key::Digit(6));
    m[20] = Some(Key::Digit(9));
    m[21] = Some(Key::Hash);
    m[11] = Some(Key::Digit(2));
    m[15] = Some(Key::Digit(5));
    m[19] = Some(Key::Digit(8));
    m[17] = Some(Key::Digit(0));
    m[7] = Some(Key::Up);
    m[8] = Some(Key::Down);
    m[4] = Some(Key::Left);
    m[5] = Some(Key::Right);
    m[10] = Some(Key::Digit(1));
    m[14] = Some(Key::Digit(4));
    m[18] = Some(Key::Digit(7));
    m[13] = Some(Key::Star);
    // PF1 and PF2, stock's codes 0 and 1 (`analysis/keypad.md`): SIDE1 and SIDE2 (decision 77; stock's PF1 is
    // PTT C, nicFW's S1-S3 are PE2, PE5 and PA12).
    m[0] = Some(Key::Side(1));
    m[1] = Some(Key::Side(2));
    m[CODE_PTT_A as usize] = Some(Key::Ptt(PTT_A));
    m[CODE_PTT_B as usize] = Some(Key::Ptt(PTT_B));
    m[CODE_PTT_VFO as usize] = Some(Key::PTT);
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
        assert_eq!(key(CODE_PTT_A), Some(Key::Ptt(0)));
        assert_eq!(key(CODE_PTT_B), Some(Key::Ptt(1)), "PTT B keys slot B");
        assert_eq!((key(0), key(1)), (Some(Key::Side(1)), Some(Key::Side(2))), "PF1 = SIDE1, PF2 = SIDE2");
        assert_eq!(key(99), None);
    }

    /// The codes read off the black radio's key test (Oct 12, 2026).
    #[test]
    fn front_keys_as_measured() {
        let want = [
            (6, Key::Menu), (7, Key::Up), (8, Key::Down), (4, Key::Left), (5, Key::Right), (3, Key::Slot),
            (9, Key::Exit), (2, Key::VfoMem), (13, Key::Star), (21, Key::Hash), (17, Key::Digit(0)),
            (10, Key::Digit(1)), (11, Key::Digit(2)), (12, Key::Digit(3)), (14, Key::Digit(4)), (15, Key::Digit(5)),
            (16, Key::Digit(6)), (18, Key::Digit(7)), (19, Key::Digit(8)), (20, Key::Digit(9)),
        ];
        for (c, k) in want {
            assert_eq!(key(c), Some(k), "code {c}");
        }
    }

    /// Every code 0-21 means one key, and every front key has exactly one code (the radio's key test).
    #[test]
    fn front_keys_mapped_once() {
        let mut keys: [Option<Key>; 22] = [None; 22];
        for (c, k) in keys.iter_mut().enumerate() {
            *k = key(c as u8);
            assert!(k.is_some(), "code {c} unmapped");
        }
        let front = [Key::Menu, Key::Exit, Key::Up, Key::Down, Key::Left, Key::Right, Key::Slot, Key::VfoMem, Key::Star, Key::Hash];
        for f in front.iter().copied().chain((0..10).map(Key::Digit)) {
            assert_eq!(keys.iter().filter(|k| **k == Some(f)).count(), 1, "{f:?}");
        }
    }
}
