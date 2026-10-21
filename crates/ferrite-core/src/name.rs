//! The characters a channel name may hold (CHAN-31, decision 221): letters, digits, space and a few symbols. The
//! two name fonts (`NAME_26`, `NAME_22`) are cut to this set, so every way a name gets in maps to it.

/// The symbols a channel name may hold, besides letters, digits and space (CHAN-31).
pub const NAME_SYMBOLS: &[u8] = b"-./_#+&()'!";

/// What a character outside the set becomes (CHAN-31).
pub const NAME_STAND_IN: u8 = b'-';

/// A byte a channel name may hold (CHAN-31): A-Z, a-z, 0-9, space and [`NAME_SYMBOLS`].
pub fn name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b' ' || NAME_SYMBOLS.contains(&c)
}

/// One byte of a name into the set: itself, or [`NAME_STAND_IN`] (CHAN-31; a 2-byte UTF-8 letter gives two).
pub fn map_name(c: u8) -> u8 {
    if name_char(c) { c } else { NAME_STAND_IN }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CHAN-31: the set is A-Z, a-z, 0-9, space and - . / _ # + & ( ) ' !; anything else maps to '-'.
    #[test]
    fn chan_31_name_set() {
        let set: std::vec::Vec<u8> = (0..=255u8).filter(|&c| name_char(c)).collect();
        assert_eq!(set.len(), 26 * 2 + 10 + 1 + 11);
        for c in b"AZaz09 -./_#+&()'!" {
            assert!(name_char(*c) && map_name(*c) == *c);
        }
        for c in b"?*@=:;%\"<>[]$^~|{}\\`,\x00\x7f\xc3\xa9" {
            assert_eq!(map_name(*c), b'-', "{:?}", *c as char);
        }
    }
}
