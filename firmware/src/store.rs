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

/// First boot: the layout header (0x02C000) is missing, so setup runs (BOOT-12, SETUP-1). The Storage task
/// doesn't read the header yet, so this says no; until it does, setup runs from MENU 0 9 8 1.
pub fn first_boot() -> bool {
    false
}

/// The radio-wide settings the UI borrows for good (`App::new`). Until the Storage task reads the settings
/// struct (0x02D000) and HF mode's sector (0x084000), every boot starts from the defaults (BOOT-12). Once only.
pub fn settings() -> &'static mut ferrite_app::settings::Settings {
    cortex_m::singleton!(: ferrite_app::settings::Settings = ferrite_app::settings::Settings::DEFAULT).unwrap()
}

/// The last alert, for the alert screen (WX-5): the App's too, kept outside it. Once only.
pub fn last_alert() -> &'static mut ferrite_app::screens::alert::AlertInfo {
    cortex_m::singleton!(: ferrite_app::screens::alert::AlertInfo = ferrite_app::screens::alert::AlertInfo::NONE).unwrap()
}

/// The store the UI task reads from.
pub fn channels() -> Table<'static> {
    Table { zones: &ZONES, channels: &CHANNELS }
}
