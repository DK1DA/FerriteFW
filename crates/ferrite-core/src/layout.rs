//! The SPI flash map (`docs/firmware-design.md`, "SPI flash layout"). Ferrite writes only inside [`WRITABLE`];
//! the flash driver (uradio `w25q`) refuses everything else, so factory calibration, stock's data and the
//! bootloader's update area cannot be written by any bug above it.

use core::ops::Range;

pub const CALIBRATION: Range<u32> = 0x00_F000..0x01_0000;
pub const LAYOUT_HEADER: Range<u32> = 0x02_C000..0x02_D000;
pub const SETTINGS: Range<u32> = 0x02_D000..0x02_F000;
pub const CHANNELS: Range<u32> = 0x02_F000..0x03_F000;
pub const ZONES: Range<u32> = 0x03_F000..0x04_3000;
pub const APRS: Range<u32> = 0x04_3000..0x04_B000;
pub const EVENT_LOG: Range<u32> = 0x04_B000..0x06_B000;
/// Imported from the owner's stock firmware, never shipped.
pub const SSB_PATCH: Range<u32> = 0x06_B000..0x06_F000;
pub const TRIMS: Range<u32> = 0x06_F000..0x07_0000;
pub const WAYPOINTS: Range<u32> = 0x07_0000..0x07_4000;
/// Two sectors for rewriting a sector without losing it to a power cut: a copy of the new content, then a log
/// of rewrites in progress (ferrite-store `swap`). Taken from the start of the free 64 KB held for the mailbox.
pub const SWAP: Range<u32> = 0x07_4000..0x07_6000;
/// Stock V0.29's record: left alone.
pub const STOCK_RECORD: Range<u32> = 0x08_2000..0x08_3000;
/// Stock's voice prompts: writable only after the owner has backed them up (a later, explicit unlock).
pub const PROMPTS: Range<u32> = 0x20_0000..0x30_0000;
/// The bootloader reflashes the MCU from here if it finds its header: never written.
pub const UPDATE_AREA: Range<u32> = 0x30_0000..0x40_0000;

/// Everything Ferrite may erase or program, from the layout header to the end of the swap sectors. Prompts are
/// not in it until they have been backed up.
#[allow(clippy::single_range_in_vec_init)] // a list: the prompts region joins it once backed up
pub static WRITABLE: [Range<u32>; 1] = [LAYOUT_HEADER.start..SWAP.end];

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &Range<u32>, b: &Range<u32>) -> bool {
        a.start < b.end && b.start < a.end
    }

    #[test]
    fn protected_ranges_are_not_writable() {
        for w in &WRITABLE {
            for p in [CALIBRATION, STOCK_RECORD, PROMPTS, UPDATE_AREA, 0..0x02_C000] {
                assert!(!overlaps(w, &p), "{w:x?} overlaps {p:x?}");
            }
        }
    }

    #[test]
    fn regions_tile_the_writable_range() {
        let r = [LAYOUT_HEADER, SETTINGS, CHANNELS, ZONES, APRS, EVENT_LOG, SSB_PATCH, TRIMS, WAYPOINTS, SWAP];
        for p in r.windows(2) {
            assert_eq!(p[0].end, p[1].start);
        }
        for x in &r {
            assert_eq!(x.start % 4096, 0, "sector aligned");
            assert_eq!(x.end % 4096, 0);
        }
        assert_eq!(r[0].start..r[9].end, WRITABLE[0]);
        assert_eq!(CHANNELS.len(), 1000 * 64 + 1536, "1,000 channels of 64 B, rounded to sectors");
    }
}
