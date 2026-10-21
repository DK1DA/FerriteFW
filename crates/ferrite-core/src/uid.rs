//! The MCU's 96-bit factory unique ID as text (SELF-8, LINK-35.1): Crockford base32, 20 characters for the 12
//! bytes, nothing lost. The alphabet has no I, L, O or U, so it reads back without mix-ups; the same text on About,
//! from the link's `INFO` (tools/ferrite_link.py) and in Ferrite Console.

/// Crockford's base32 digits.
pub const DIGITS: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The 12 bytes as the MCU stores them (0x1FFFF7E8 up): each word little-endian, lowest word first.
pub fn bytes(uid: [u32; 3]) -> [u8; 12] {
    let mut b = [0u8; 12];
    for (i, w) in uid.iter().enumerate() {
        b[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    b
}

/// The ID as 20 base32 digits: [`bytes`] read as a bit stream, most significant bit first, five bits a digit (the
/// last digit's low four bits are zero padding).
pub fn text(uid: [u32; 3]) -> [u8; 20] {
    let b = bytes(uid);
    let mut out = [0u8; 20];
    for (i, o) in out.iter_mut().enumerate() {
        let bit = 5 * i;
        let mut v = 0u8;
        for k in 0..5 {
            let n = bit + k;
            let on = n < 96 && b[n / 8] & (0x80 >> (n % 8)) != 0;
            v = v << 1 | on as u8;
        }
        *o = DIGITS[v as usize];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SELF-8: 20 base32 digits for the 96 bits, the stored byte order, round trip to the same bits.
    #[test]
    fn base32_text() {
        assert_eq!(&text([0; 3]), b"00000000000000000000");
        assert_eq!(&text([u32::MAX; 3]), b"ZZZZZZZZZZZZZZZZZZZG", "the last digit: one bit of the ID, four of padding");
        let uid = [0x3F2A_91C0, 0x7D44_0123, 0x89AB_CDEF];
        assert_eq!(bytes(uid)[..4], [0xC0, 0x91, 0x2A, 0x3F], "little-endian words, lowest first");
        let t = text(uid);
        // decode back
        let mut bits = std::vec::Vec::new();
        for c in t {
            let v = DIGITS.iter().position(|&d| d == c).unwrap() as u8;
            for k in (0..5).rev() {
                bits.push(v >> k & 1);
            }
        }
        let mut back = [0u8; 12];
        for (n, &bit) in bits.iter().take(96).enumerate() {
            back[n / 8] |= bit << (7 - n % 8);
        }
        assert_eq!(back, bytes(uid));
        assert!(t.iter().all(|c| !b"ILOU".contains(c)));
    }
}
