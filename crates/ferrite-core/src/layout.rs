//! The SPI flash map (`docs/firmware-design.md`, "SPI flash layout"). Ferrite writes only inside [`WRITABLE`];
//! the flash driver (uradio `w25q`) refuses everything else, so factory calibration, stock's data and the
//! bootloader's update area cannot be written by any bug above it.

use core::ops::Range;

pub const CALIBRATION: Range<u32> = 0x00_F000..0x01_0000;
pub const LAYOUT_HEADER: Range<u32> = 0x02_C000..0x02_D000;
pub const SETTINGS: Range<u32> = 0x02_D000..0x02_F000;
/// 16 sectors of 63 channels × 64 B (1,008 slots, 1,000 used); each sector's last 64 bytes stay free.
pub const CHANNELS: Range<u32> = 0x02_F000..0x03_F000;
/// 2 sectors of 127 zones × 32 B (254 zones); each sector's last 32 bytes stay free.
pub const ZONES: Range<u32> = 0x03_F000..0x04_1000;
/// 16 range presets × 224 B in one sector.
pub const PRESETS: Range<u32> = 0x04_1000..0x04_2000;
/// One sector: a data sector's new content while it is rewritten, with a footer saying where it goes
/// (ferrite-store `scratch`). The free last bytes of every data sector are what make room for that footer.
pub const SCRATCH: Range<u32> = 0x04_2000..0x04_3000;
pub const APRS: Range<u32> = 0x04_3000..0x04_B000;
pub const EVENT_LOG: Range<u32> = 0x04_B000..0x06_B000;
/// Imported from the owner's stock firmware, never shipped.
pub const SSB_PATCH: Range<u32> = 0x06_B000..0x06_F000;
pub const TRIMS: Range<u32> = 0x06_F000..0x07_0000;
pub const WAYPOINTS: Range<u32> = 0x07_0000..0x07_4000;
/// APRS contacts (APRS-37): 128 in callsign order, 127 + 1 per sector.
pub const CONTACTS: Range<u32> = 0x07_4000..0x07_6000;
/// Free, held for the packet mailbox: 48 KB up to stock's record (and 4 KB after it, to 0x084000).
pub const MAILBOX: Range<u32> = 0x07_6000..0x08_2000;
/// Stock V0.29's record: left alone.
pub const STOCK_RECORD: Range<u32> = 0x08_2000..0x08_3000;
/// Stock's voice prompts: writable only after the owner has backed them up (a later, explicit unlock).
pub const PROMPTS: Range<u32> = 0x20_0000..0x30_0000;
/// The bootloader reflashes the MCU from here if it finds its header: never written.
pub const UPDATE_AREA: Range<u32> = 0x30_0000..0x40_0000;

/// Everything Ferrite may erase or program, from the layout header to the end of the contacts. Prompts are not
/// in it until they have been backed up.
#[allow(clippy::single_range_in_vec_init)] // a list: the prompts region joins it once backed up
pub static WRITABLE: [Range<u32>; 1] = [LAYOUT_HEADER.start..CONTACTS.end];

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
        let r = [LAYOUT_HEADER, SETTINGS, CHANNELS, ZONES, PRESETS, SCRATCH, APRS, EVENT_LOG, SSB_PATCH, TRIMS, WAYPOINTS, CONTACTS];
        for p in r.windows(2) {
            assert_eq!(p[0].end, p[1].start);
        }
        for x in &r {
            assert_eq!(x.start % 4096, 0, "sector aligned");
            assert_eq!(x.end % 4096, 0);
        }
        assert_eq!(r[0].start..r[r.len() - 1].end, WRITABLE[0]);
        assert_eq!(CONTACTS.end, MAILBOX.start);
        assert_eq!(MAILBOX.end, STOCK_RECORD.start);
        assert!(CHANNELS.len() / 4096 * 63 >= 1000, "1,000 channels at 63 per sector");
        assert_eq!(ZONES.len() / 4096 * 127, 254, "254 zones at 127 per sector");
    }
}
