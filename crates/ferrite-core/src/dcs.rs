//! DCS codewords (CHAN-3's tone words `0x8000 | inverted 0x4000 | code`): the Golay (23,12) word a code sends, and
//! the code of the other polarity that sends the same bits (TONE-20.5): an inverted code on air is some normal
//! code's word turned, so a receiver that reads the bits can't tell D023I from D047N.

/// The 23-bit word a code sends, as the BK4829 driver's `dcs_codeword` (the data bits, the fixed 100, the Golay
/// check bits); `inverted` complements it.
pub const fn codeword(code: u16, inverted: bool) -> u32 {
    let data = 0x800 | (code as u32 & 0x1FF);
    let mut r = data << 11;
    let mut i = 22;
    while i >= 11 {
        if r >> i & 1 != 0 {
            r ^= 0xC75 << (i - 11);
        }
        i -= 1;
    }
    let w = data | r << 12;
    if inverted { !w & 0x7F_FFFF } else { w }
}

/// The code in `table` with polarity `inverted` whose word is `w` turned by some rotation, if any.
fn find(w: u32, inverted: bool, table: &[u16]) -> Option<u16> {
    let mut x = if inverted { !w & 0x7F_FFFF } else { w & 0x7F_FFFF };
    for _ in 0..23 {
        let c = (x & 0x1FF) as u16;
        if x & 0xE00 == 0x800 && codeword(c, false) == x && table.contains(&c) {
            return Some(0x8000 | if inverted { 0x4000 } else { 0 } | c);
        }
        x = (x >> 1) | (x & 1) << 22;
    }
    None
}

/// TONE-20.5: the DCS tone word of the other polarity that sends the same bits as `t` (a code in `table`), e.g.
/// D047N for D023I; None for CTCSS, or when no code in `table` has it.
pub fn twin(t: u16, table: &[u16]) -> Option<u16> {
    if t & 0x8000 == 0 {
        return None;
    }
    let inv = t & 0x4000 != 0;
    find(codeword(t & 0x1FF, inv), !inv, table)
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: u16 = 0x8000;
    const I: u16 = 0xC000;
    /// A few of the standard codes (the table the UI passes has all 104).
    const TABLE: [u16; 6] = [0o023, 0o047, 0o025, 0o244, 0o754, 0o131];

    /// TONE-20.5: D023I and D047N send the same bits, both ways round; a CTCSS tone has none.
    #[test]
    fn d023i_is_d047n() {
        assert_eq!(twin(I | 0o023, &TABLE), Some(N | 0o047));
        assert_eq!(twin(N | 0o047, &TABLE), Some(I | 0o023));
        assert_eq!(twin(N | 0o023, &TABLE), Some(I | 0o047));
        assert_eq!(twin(885, &TABLE), None);
        assert_eq!(twin(I | 0o023, &[0o023]), None, "the twin not in the table");
    }

    /// Every twin is a twin back.
    #[test]
    fn twins_pair_up() {
        for &c in &TABLE {
            for p in [N, I] {
                if let Some(t) = twin(p | c, &TABLE) {
                    assert_ne!(t & 0x4000, p & 0x4000, "the other polarity");
                    assert_eq!(twin(t, &TABLE), Some(p | c), "{c:o}");
                }
            }
        }
    }
}
