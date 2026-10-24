//! # ufx25
//!
//! FX.25 forward error correction for AX.25 packet radio, for microcontrollers. FX.25 wraps an ordinary AX.25
//! frame (flags, bit stuffing and all) in a Reed-Solomon block behind a 64-bit correlation tag, so a plain AX.25
//! receiver still decodes it while an FX.25 receiver can repair damaged bytes.
//!
//! - [`Encoder`]: an AX.25 frame (without FCS, as `uafsk::hdlc::Framer` takes it) to the bits to transmit,
//!   before NRZI: flags, tag, block, flags.
//! - [`Receiver`]: data bits in (after NRZI decoding, as `uafsk::hdlc::Deframer` gets them), corrected and
//!   FCS-checked frames out, with the number of bytes the code repaired.
//! - [`rs`]: the Reed-Solomon codes on their own.
//!
//! Part of udigimodes: `no_std`, no allocation, integer only.
//!
//! Source: "FX.25 Forward Error Correction Extension to AX.25 Link Protocol for Amateur Packet Radio", Stensat
//! Group, draft 0.01.06, 1 September 2006 (tags, codes, layout, bit order). Details it leaves open (field
//! polynomial, generator roots, where a shortened code's zeros go, pad bits) were measured against Dire Wolf;
//! see the README.

#![no_std]
#![allow(clippy::needless_range_loop)]

pub mod gf;
pub mod rs;
mod rx;
mod tx;

pub use rx::{decode, Collector, Decoded, Hunter, Receiver, Tag, DEFAULT_TAG_ERRORS};
pub use tx::{Bits, Encoder, TooLong};

/// One FX.25 code: its correlation tag and block size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Tag number in the specification's Table 1 (0x01 to 0x0B).
    pub id: u8,
    /// The 64-bit correlation tag, sent least significant bit first.
    pub tag: u64,
    /// Block bytes on air (information plus check).
    pub n: u8,
    /// Information bytes (the bit-stuffed AX.25 frame with its flags, then pad).
    pub k: u8,
}

impl Mode {
    /// Check bytes: 16, 32 or 64 (corrects up to half as many wrong bytes).
    pub const fn check(&self) -> usize {
        (self.n - self.k) as usize
    }
}

/// The eleven codes of the specification's Table 1, in tag order. A `static`, not a `const`: read per bit while
/// hunting, and a named table stays with the hot code on firmware that places unnamed constants in slower flash.
pub static MODES: [Mode; 11] = [
    Mode { id: 0x01, tag: 0xB74D_B7DF_8A53_2F3E, n: 255, k: 239 },
    Mode { id: 0x02, tag: 0x26FF_60A6_00CC_8FDE, n: 144, k: 128 },
    Mode { id: 0x03, tag: 0xC7DC_0508_F3D9_B09E, n: 80, k: 64 },
    Mode { id: 0x04, tag: 0x8F05_6EB4_3696_60EE, n: 48, k: 32 },
    Mode { id: 0x05, tag: 0x6E26_0B1A_C583_5FAE, n: 255, k: 223 },
    Mode { id: 0x06, tag: 0xFF94_DC63_4F1C_FF4E, n: 160, k: 128 },
    Mode { id: 0x07, tag: 0x1EB7_B9CD_BC09_C00E, n: 96, k: 64 },
    Mode { id: 0x08, tag: 0xDBF8_69BD_2DBB_1776, n: 64, k: 32 },
    Mode { id: 0x09, tag: 0x3ADB_0C13_DEAE_2836, n: 255, k: 191 },
    Mode { id: 0x0A, tag: 0xAB69_DB6A_5431_88D6, n: 192, k: 128 },
    Mode { id: 0x0B, tag: 0x4A4A_BEC4_A724_B796, n: 128, k: 64 },
];

/// Longest block (bytes): what a [`Receiver`] or [`Encoder`] holds.
pub const MAX_BLOCK: usize = 255;

/// HDLC flag.
pub(crate) const FLAG: u8 = 0x7E;

#[cfg(test)]
mod tests {
    use super::*;

    /// The tags are the specification's Gold codes, regenerated from its description: two 6-bit LFSRs,
    /// x^6 + x^5 seeded 0x3F and x^6 + x^5 + x^3 + x^2 seeded with the tag number, shifting left with the top bit
    /// out; the XOR of the two is 63 chips after a leading 0, the first chip sent first (the tag's bit 1). Checks
    /// the table and the bit order, including the specification's reserved tags 0x00 and 0x40.
    #[test]
    fn tags_are_the_gold_codes() {
        fn lfsr(mut s: u8, taps: &[u8]) -> impl Iterator<Item = u64> + '_ {
            (0..63).map(move |_| {
                let out = (s >> 5) & 1;
                let fb = taps.iter().fold(0, |x, &t| x ^ (s >> (t - 1))) & 1;
                s = ((s << 1) | fb) & 0x3F;
                out as u64
            })
        }
        let gold = |seed1: u8, seed2: u8| {
            lfsr(seed1, &[6, 5]).zip(lfsr(seed2, &[6, 5, 3, 2])).enumerate().fold(0u64, |v, (i, (a, b))| v | ((a ^ b) << (i + 1)))
        };
        for m in &MODES {
            assert_eq!(gold(0x3F, m.id), m.tag, "tag {:#x}", m.id);
        }
        assert_eq!(gold(0x3F, 0x00), 0x566E_D271_7946_107E);
        assert_eq!(gold(0x3F, 0x3F), 0x17AC_94BA_24A0_3A00);
        assert_eq!(gold(0x00, 0x3F), 0x41C2_46CB_5DE6_2A7E);
        // The receiver tolerates bit errors in a tag, so the tags must be far apart.
        for a in &MODES {
            for b in &MODES {
                if a.id != b.id {
                    assert!((a.tag ^ b.tag).count_ones() >= 16);
                }
            }
        }
    }
}
