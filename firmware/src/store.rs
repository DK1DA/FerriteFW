//! The channels the UI reads ([`ChannelStore`]). Until the Storage task keeps channel records in SPI flash
//! (CHAN-7, CHAN-9), a fixed table in MCU flash: the two calling channels, so V/M and channel mode work on the
//! radio. Nothing here is ever written.

use ferrite_app::chan::{channel, Channel, Table};
use ferrite_app::slot::DEFAULT_CFG;
use ferrite_core::freq::mhz;

static ZONES: [&[u8]; 1] = [b"Calling"];
static CHANNELS: [Channel; 2] = [
    channel(0, 0, mhz(146) + 52_000, "Calling 2 m", DEFAULT_CFG),
    channel(1, 0, mhz(446), "Calling 70 cm", DEFAULT_CFG),
];

/// The store the UI task reads from.
pub fn channels() -> Table<'static> {
    Table { zones: &ZONES, channels: &CHANNELS }
}
