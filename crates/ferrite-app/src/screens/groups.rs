//! A settings group (MENU 0 n) and a third-level list (MENU 0 n m): one list widget for both (manual chapter
//! 11, MENU-3 to MENU-24; MenuGroup mock-up). The lists are data: [`GROUP_ITEMS`] and [`SUBS`] hold every item
//! of the manual's Appendix A with its value and help line. [`SETTINGS`] has each wired setting once: its path,
//! its field, how it shows and steps, and what the Radio task is told; [`wired`] (the live value), [`change`] (the
//! knob) and [`reset`] (#) work from it, with a short match for the settings that need more. The rest show their
//! default, read only, until their feature lands.
//!
//! Keys: ▲▼ move, the knob or ◀▶ change the highlighted value (MENU-16), OK opens a screen or list, a digit
//! picks its row and opens it (MENU-5: a path typed straight through lands where it would typed slowly), #
//! resets (MENU-18), BACK goes up keeping the place (MENU-24).

use super::nav::{self, Dest};
use super::setup::ExtendTx;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::*;
use crate::slot::Item;
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::{CalState, Device, RadioCmd};
use ferrite_core::VERSION;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::Row;
use ufmt::uwrite;

/// One item: name, value shown until it is wired (a list shows a summary) and help line, in one string as
/// "name|value|help". One string per item, not three: 8 bytes of table per item instead of 24 (about 3 KB).
#[derive(Clone, Copy)]
pub struct Entry(&'static str);

impl Entry {
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        let (name, rest) = self.0.split_once('|').unwrap_or((self.0, ""));
        let (value, help) = rest.split_once('|').unwrap_or((rest, ""));
        (name, value, help)
    }

    pub fn name(self) -> &'static str {
        self.parts().0
    }

    /// The value shown until the setting is wired (a list shows a summary).
    pub fn value(self) -> &'static str {
        self.parts().1
    }

    pub fn help(self) -> &'static str {
        self.parts().2
    }
}

/// SND-3: the beep volume (0 7 5 2), three DAC levels (decision 100).
const BEEP_VOLS: [&str; 3] = ["Low", "Mid", "High"];
const G1: [Entry; 8] = [
    Entry("Watch|A+B|Which slots are heard besides the active one."),
    Entry("Watch behaviour|Heard|Watched audio and pause time."),
    Entry("Background job|Off|A zone scan or decoder on a watched slot."),
    Entry("Scan target|Current zone|What SIDE2 scans."),
    Entry("Resume|5 s|When a stopped scan moves on."),
    Entry("Scan options|Up, 1 s|Direction, dwell, priority, inputs."),
    Entry("Range presets|4 presets|Named ranges to scan."),
    Entry("Skip list|Empty|Frequencies * skipped in this scan."),
];
const G2: [Entry; 5] = [
    Entry("Browse||Zones, all channels, nearby, recent."),
    Entry("Add repeater||Frequency, shift, tone, name."),
    Entry("Zones||Name, scan options, priority channels."),
    Entry("Nearby|Off|Sort channels by distance."),
    Entry("Channel display|Name|Name, number or frequency on home."),
];
const G3: [Entry; 6] = [
    Entry("FM broadcast||Listen now; hold MENU there for its settings."),
    Entry("Band scope|60 bars|Bars, step, speed, trigger, waterfall."),
    Entry("Frequency counter||Find a nearby transmitter's frequency."),
    Entry("HF mode||Below 64 MHz. Everything else pauses."),
    Entry("Crossband repeater||Repeat one slot's band on another's."),
    Entry("Tone search||Find the CTCSS or DCS in use."),
];
const G4: [Entry; 9] = [
    Entry("Event log||Every decode and alert: the inbox."),
    Entry("POCSAG|439.9875|Pager frequency, baud, capcodes."),
    Entry("DSC|Off|Marine distress calls, receive only."),
    Entry("AIS|Off|Ships near you."),
    // 5 was "Sensor reader" (with the jamming alarm), dropped (decision 75); the number stays empty so the
    // rest of the group keeps its paths.
    Entry("-||Not used."),
    Entry("Selcall, MDC1200|Off|Decode selcall and MDC1200 IDs."),
    Entry("DTMF decode|Off|Show and log DTMF sequences."),
    Entry("Popups|On|A 3 s toast for each decode."),
    Entry("Weather alerts|On|SAME, location codes, tone, channel."),
];
const G5: [Entry; 9] = [
    Entry("Heard||Stations heard, nearest first."),
    Entry("Messages||Messages sent to you and by you."),
    Entry("APRS mode|Off|Slot C becomes the APRS slot."),
    Entry("My station|SSID 9|SSID, symbol, comment, position."),
    Entry("Beacon|Off|When to beacon: manual, timed, smart."),
    Entry("Path|WIDE1-1,WIDE2-1|The digipeater path."),
    Entry("Radio|144.390|APRS frequency, delays, levels."),
    Entry("Digipeat|Off|Relay others' packets."),
    // 9 was KISS TNC alone; now the packet group (decision 77): KISS TNC moved to 0 5 9 1, the mailbox beside it.
    Entry("Packet||KISS TNC and the mailbox."),
];
const G6: [Entry; 9] = [
    Entry("GPS|On|The GPS receiver."),
    Entry("Position||Fix, satellites, position, grid."),
    Entry("Coordinate format|Deg, min|How positions are written."),
    Entry("Units|Metric|Distance, altitude and speed units."),
    Entry("Time zone|UTC|The clock's offset from UTC."),
    Entry("Clock|24 h|12 or 24 h, from GPS; OK sets it by hand."),
    Entry("Waypoints||Marked places; go to one."),
    Entry("GPS during RX/TX|Always on|Turn the GPS off around signals."),
    Entry("Fixed position|None|Used when there is no fix."),
];
const G7: [Entry; 9] = [
    Entry("Theme|Dark|Dark, daylight or night red."),
    Entry("Home layout|Watch|What home shows."),
    Entry("Display|Bars|S-meter, power-on message, boot screen."),
    Entry("Backlight|15 s|Timeout, brightness, wake."),
    Entry("Sounds|Beep on|Key beep, volume, alert tones."),
    Entry("LEDs|RX, TX and heartbeat|And a flash while the screen is dark."),
    Entry("Battery Save|Level 1|Receiver sleep: longer life, slower."),
    Entry("Auto power off|Off|Off after a time without key or signal."),
    Entry("Menu timeout|30 s|An idle menu returns home."),
];
const G8: [Entry; 9] = [
    Entry("Timeout|120 s|Transmit timeout and its pre-alert."),
    Entry("VOX|Off|Voice-operated transmit."),
    Entry("Mic gain|5|Higher is louder on air: 1-9."),
    Entry("Tones sent|1750 Hz|Tone burst, subtone level."),
    Entry("Tail elimination|Off|Squelch tail, end tone (roger beep)."),
    Entry("DTMF|100 ms|Dialer, memories, timing, PTT ID."),
    Entry("Callsign and ID|Empty|Your callsign, and CW ID."),
    Entry("Busy lockout default|Off|For new channels and the VFO."),
    Entry("Extend TX|Off|Transmit beyond the amateur bands."),
];
const G9: [Entry; 9] = [
    Entry("Keys and lock|No auto lock|Auto lock, lock covers, key test, shortcuts."),
    Entry("Connections|BT off|Bluetooth, approved devices, link."),
    Entry("PIN and lock|Off|PIN lock, channel-only mode, PIN."),
    Entry("Region|Not set|Your amateur bands and defaults."),
    Entry("Receiver|Tail 150 ms|Advanced squelch, tail, discriminator."),
    Entry("SSB patch||USB, LSB and CW in HF mode."),
    Entry("Calibration and trims||View calibration, trim crystal, battery."),
    Entry("Setup and reset||Run setup again, reset settings."),
    Entry("About||Version, identity, checks, battery."),
];

/// The nine groups' items (MENU 0 1 to MENU 0 9).
pub(crate) const GROUP_ITEMS: [&[Entry]; 9] = [&G1, &G2, &G3, &G4, &G5, &G6, &G7, &G8, &G9];
/// Short names for the status bar (up to 10 characters).
const SHORT: [&str; 9] = ["Scan", "Channels", "Tools", "Decoders", "APRS", "GPS", "Display", "Transmit", "System"];

/// The third-level lists, by group and item.
pub(crate) const SUBS: [((u8, u8), &[Entry]); 29] = [
    ((1, 2), &[
        Entry("Watched audio|Heard|Heard, muted, per slot."),
        Entry("Pause time|1 min|# on home pauses watch this long."),
    ]),
    ((1, 6), &[
        Entry("Direction|Up|Scan up or down."),
        Entry("Dwell|1 s|Wait before resuming."),
        Entry("Scan inputs|Off|Also a channel's TX frequency."),
        Entry("Return to|Start|Where a stopped scan returns."),
        Entry("Priority check|Every 2 s|Look at the priority channels."),
    ]),
    // The FM screen's options (BCR-3): on the radio 0 3 1 opens the screen and its digits 1-5 reach these; the
    // terminal lists them here (TERM-12).
    ((3, 1), &[
        Entry("Presets|Empty|20 stations."),
        Entry("Step|100 kHz|Tuning step."),
        Entry("AGC|Auto|Gain control."),
        Entry("Radio monitor|On|A slot opening interrupts the broadcast."),
        Entry("Sleep timer|Off|End the broadcast after a time."),
    ]),
    ((3, 2), &[
        Entry("Bars|60|Bars across; span is bars x step."),
        Entry("Step|25 kHz|Distance between bars."),
        Entry("Sweep speed|1 ms|Time on each point."),
        Entry("Trigger|Off|Over the line: monitor or log."),
        Entry("Waterfall row|Off|History under the scope."),
        Entry("Open band scope||Sweep around the active slot. Home: hold 1."),
    ]),
    ((4, 2), &[
        Entry("Decode|Off|Pagers on a slot tuned to the frequency."),
        Entry("Frequency|439.9875|The pager channel; 512, 1200 and 2400 baud."),
        Entry("Capcode filter|None|Only this capcode: OK types it, # clears it."),
    ]),
    ((4, 3), &[Entry("DSC|Off|Decode DSC on channel 70."), Entry("Own MMSI|None|Your MMSI, receive only.")]),
    ((4, 4), &[Entry("AIS|Off|161.975 and 162.025, in turn."), Entry("Range filter|25 nm|Off, 5, 10, 25, 50 nm; needs a fix.")]),
    ((4, 9), &[
        Entry("SAME|On|Decode weather alerts."),
        Entry("Location codes|None|Up to 8 FIPS or CLC codes."),
        Entry("Channel|Auto|The weather channel to watch."),
    ]),
    ((5, 4), &[
        Entry("SSID|9|0-15 after your callsign."),
        Entry("Symbol|Car|Your map symbol."),
        Entry("Comment|Empty|Up to 40 characters."),
        Entry("Position source|GPS|GPS, or the fixed position."),
        Entry("Fixed position|None|Used without GPS."),
        Entry("Ambiguity|Exact|Digits dropped from your position."),
        Entry("Mic-E status|En route|Status in Mic-E beacons."),
    ]),
    ((5, 5), &[
        Entry("Mode|Off|Manual, timed, distance or smart."),
        Entry("Smart settings|Standard|Rates, speeds, turn angle."),
        Entry("After voice TX|Off|Beacon after you talk."),
        Entry("Power|Low|Beacon and message power."),
    ]),
    ((5, 6), &[Entry("Path|WIDE1-1,WIDE2-1|None, WIDE1-1, WIDE1-1,WIDE2-1, or custom."), Entry("Custom path|None|Your own path, like WIDE2-2: OK types it.")]),
    ((5, 7), &[
        Entry("Frequency|144.390|The APRS channel."),
        Entry("TX delay|300 ms|Carrier before data."),
        Entry("TX level|Mid|Modulation level."),
        Entry("RX level|Auto|Decoder input level."),
        Entry("Mute speaker|On|Mute packets on the speaker."),
        Entry("Ringer on message|On|Ring for a new message."),
        Entry("Heard popup|Off|A toast per station heard."),
    ]),
    ((5, 8), &[
        Entry("Digipeat|Off|Relay others' packets."),
        Entry("Aliases|WIDE1-1|WIDE1-1, and your call if chosen."),
        Entry("Wait|0 s|Delay before relaying, 0-5 s."),
    ]),
    ((5, 9), &[
        Entry("KISS TNC|Off|Raw KISS on the cable or Bluetooth."),
        Entry("Mailbox|Off|Answer packet connects to your SSID."),
        Entry("Mailbox SSID|1|The SSID callers connect to."),
        Entry("Mailbox frequency|None|Where the mailbox listens."),
        Entry("Bulletins|On|Callers may post bulletins."),
    ]),
    ((7, 3), &[
        // 1 was "Battery as": the bar shows an icon only, % and volts on Status (SBAR-19); S-meter took its row
        Entry("S-meter|Bars|Bars or dBm."),
        Entry("Power-on message|Callsign|Off, callsign or text."),
        Entry("Boot screen|Logo|Logo, or the owner card."),
    ]),
    ((7, 4), &[
        Entry("Timeout|1 min|Backlight off after this."),
        Entry("Brightness|7|1-10; night red keeps its own."),
        Entry("On transmit|On|Light up while transmitting."),
        Entry("Wake screen on|Keys and RX|What wakes the screen."),
        Entry("Keypad light|With screen|Off, with the screen, on key press, always."),
    ]),
    ((7, 5), &[
        Entry("Key beep|On|Beep on key presses."),
        Entry("Beep volume|3|1-5."),
        Entry("Alert tones|On|Tones for alerts."),
        Entry("Arrival tone|On|A tone on reaching a waypoint."),
    ]),
    ((8, 1), &[Entry("TX timeout|120 s|Transmit stops after this."), Entry("Pre-alert|10 s|Warning before the timeout.")]),
    ((8, 2), &[Entry("VOX|Off|Transmit when you talk."), Entry("Level|5|Higher needs a louder voice: 1 keys easiest."), Entry("Tail|2 s|Hang time after speech.")]),
    ((8, 4), &[
        Entry("Tone burst|1750 Hz|PTT + SIDE2 sends it."),
        Entry("Subtone level|Normal|CTCSS/DCS strength: raise for a deaf repeater."),
    ]),
    ((8, 5), &[
        Entry("Mode|Off|Receive, transmit or both."),
        Entry("Phase|55 Hz|55 Hz or a phase shift."),
        // 3 was "Repeater tail delay", dropped (decision 63); the number stays empty so End tone keeps 0 8 5 4.
        Entry("-||Not used."),
        Entry("End tone|Off|A short tone as you let go of PTT."),
    ]),
    ((8, 6), &[
        Entry("Dialer||Type and send DTMF."),
        Entry("Memories||16 stored sequences."),
        Entry("Speed|100 ms|Each tone and each gap."),
        Entry("PTT ID on VFOs|Off|Send the ID code at PTT."),
        Entry("ID code|Empty|The radio's DTMF ID."),
        Entry("Sidetone|On|Hear the digits you send."),
        Entry("First digit delay|300 ms|Wait after key-up so a repeater opens."),
    ]),
    ((8, 7), &[Entry("Callsign|Empty|For APRS, CW ID, power-on message."), Entry("CW ID|Off|Send your callsign in CW.")]),
    ((9, 1), &[
        Entry("Auto lock|Off|Lock the keys after a time."),
        Entry("Lock covers|Keys|Keys, or keys and PTT."),
        Entry("Key test||Each key's code, name and press."),
        Entry("Shortcuts||What holding each digit on home does."),
    ]),
    ((9, 2), &[
        Entry("Bluetooth|Off|The Bluetooth link to the web app."),
        Entry("Link status||Cable and Bluetooth now."),
        Entry("Debug console|Off|Off, cable or Bluetooth."),
        Entry("Cable port|Auto|Link, terminal or KISS."),
        Entry("Terminal colour|Off|ANSI colour on the cable terminal."),
        Entry("Cable log|Off|Plain log on the cable at power on."),
    ]),
    ((9, 3), &[Entry("PIN lock|Off|Ask for the PIN at power on, or unlock too."), Entry("Channel-only mode|Off|Hide VFO and editing."), Entry("Change PIN|None|Type a new PIN; the old one first.")]),
    ((9, 5), &[
        Entry("Advanced squelch||Thresholds per band."),
        Entry("Squelch tail|150 ms|Audio after the signal drops."),
        Entry("Discriminator mode|Off|Flat audio for decoders."),
    ]),
    ((9, 7), &[
        Entry("Calibration||The factory calibration, read only."),
        Entry("Crystal trim|0 ppm|Centre a known signal."),
        Entry("Battery reading trim|+0.00 V|Match the reading to a meter."),
    ]),
    ((9, 8), &[
        Entry("Run setup again||The first-boot questions."),
        Entry("Reset||Everything but calibration."),
    ]),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Group {
    /// MENU 0 g, and m for a third-level list (0: the group itself).
    pub g: u8,
    pub sub: u8,
    pub row: u8,
}

impl Group {
    pub fn items(self) -> &'static [Entry] {
        items(self.g, self.sub)
    }

    /// The highlighted item's path after MENU 0: g, m and (in a list) the item.
    fn path(self, row: u8) -> ([u8; 3], usize) {
        if self.sub == 0 { ([self.g, row + 1, 0], 2) } else { ([self.g, self.sub, row + 1], 3) }
    }

    fn set_row(self, app: &mut App, row: u8) {
        app.screen = Screen::Group(Group { row, ..self });
    }
}

pub(crate) fn items(g: u8, sub: u8) -> &'static [Entry] {
    if sub == 0 {
        return GROUP_ITEMS[(g as usize).clamp(1, 9) - 1];
    }
    SUBS.iter().find(|(p, _)| *p == (g, sub)).map_or(&[], |(_, l)| l)
}

/// Why an item can't be used here, if it can't (MENU-14).
pub(crate) fn greyed(app: &App, p: &[u8]) -> Option<&'static str> {
    match p {
        [4, 9, ..] if !app.set.wx_region() => Some("US and Canada only"), // WX-3
        [4, 5] | [8, 5, 3] => Some("not used"),
        _ => None,
    }
}

/// What the Radio task is told after a setting changes (the knob, #, the terminal).
#[derive(Clone, Copy)]
enum Tell {
    Nothing,
    ScanTx,
    BeepVol,
    Vox,
    MicGain,
    TxTimeout,
    TxTones,
    SlotTones,
}

/// How a setting in [`SETTINGS`] shows, steps and resets.
#[derive(Clone, Copy)]
enum Kind {
    /// Two values (Off/On, or the two names given): any knob step flips it.
    Flip(&'static [&'static str; 2]),
    /// One of a list's values, round and round.
    Pick(&'static [&'static str]),
    /// 0 to n - 1, round and round, shown by the function.
    Round(u8, fn(u8, &mut Buf<24>)),
    /// lo to hi in steps, stopping at the ends, shown by the function.
    Level(u8, u8, u8, fn(u8, &mut Buf<24>)),
    /// Shown, stepped and reset by name in [`wired`], [`change`] and [`reset`] (more than one field, a typed
    /// value, a store of its own, or a step of its own).
    Special,
}

/// One setting: its menu path (a 0 ends a two-number path), whether the terminal's `set` and `dump` take it
/// (TERM-12, TERM-13), how it behaves, the `Settings` field it reads and writes as a number (a bool is 0 or 1),
/// and what the Radio task is told after a change. Its default is the field's value in `Settings::DEFAULT`.
#[derive(Clone, Copy)]
pub(crate) struct Setting {
    path: [u8; 3],
    term: bool,
    kind: Kind,
    get: fn(&Settings) -> u8,
    set: fn(&mut Settings, u8),
    tell: Tell,
}

impl Setting {
    fn path(&self) -> &[u8] {
        &self.path[..if self.path[2] == 0 { 2 } else { 3 }]
    }
}

/// A `Setting` for a field: `field!(gps_on: bool, ...)` or `field!(theme: u8, ...)`.
macro_rules! field {
    ($path:expr, $f:ident: bool, $kind:expr, $tell:ident) => {
        Setting { path: $path, term: true, kind: $kind, get: |s| s.$f as u8, set: |s, v| s.$f = v != 0, tell: Tell::$tell }
    };
    ($path:expr, $f:ident: u8, $kind:expr, $tell:ident) => {
        Setting { path: $path, term: true, kind: $kind, get: |s| s.$f, set: |s, v| s.$f = v, tell: Tell::$tell }
    };
    ($path:expr, special) => {
        Setting { path: $path, term: true, kind: Kind::Special, get: |_| 0, set: |_, _| {}, tell: Tell::Nothing }
    };
}

/// Every setting the menu changes, in menu order: what the terminal's `set` takes and `dump` writes (less those
/// that change only on the radio: PIN, region, Extend TX), and for most of them all the menu needs. A new
/// setting is one row here (and its field, default and place in `persist.rs`).
#[cfg_attr(target_os = "none", link_section = ".slow_rodata")]
static SETTINGS: [Setting; 68] = [
    field!([1, 1, 0], special),
    field!([1, 5, 0], special),
    field!([1, 6, 3], scan_tx: bool, Kind::Flip(&ON_OFF), ScanTx),
    field!([1, 6, 5], scan_prio: u8, Kind::Round(6, prio_text), Nothing),
    field!([3, 1, 2], fm_step: u8, Kind::Round(FM_STEPS.len() as u8, fm_step_text), Nothing),
    field!([3, 1, 3], fm_agc: u8, Kind::Round(crate::settings::AGC_MAX + 1, agc_text), Nothing),
    field!([3, 1, 4], fm_monitor: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([3, 1, 5], fm_sleep: u8, Kind::Round(SLEEP_TIMES.len() as u8, sleep_text), Nothing),
    field!([3, 2, 1], scope_bars: u8, Kind::Round(SCOPE_BAR_COUNTS.len() as u8, bars_text), Nothing),
    field!([3, 2, 2], scope_step: u8, Kind::Round(crate::slot::STEPS.len() as u8, scope_step_text), Nothing),
    field!([3, 2, 3], special),
    field!([3, 2, 4], scope_trig: u8, Kind::Pick(&SCOPE_TRIGGERS), Nothing),
    field!([3, 2, 5], scope_wf: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([4, 2, 1], pocsag_on: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([4, 2, 2], special),
    field!([4, 2, 3], special),
    field!([4, 8, 0], popups: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([4, 9, 1], same: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([5, 3, 0], special),
    field!([5, 5, 1], special),
    field!([5, 5, 3], special),
    field!([5, 5, 4], special),
    field!([5, 6, 1], special),
    field!([5, 6, 2], special),
    field!([5, 8, 1], special),
    field!([5, 8, 2], special),
    field!([5, 8, 3], special),
    field!([5, 9, 1], kiss_port: u8, Kind::Pick(&crate::settings::KISS_PORTS), Nothing),
    field!([6, 1, 0], gps_on: bool, Kind::Flip(&ON_OFF), Nothing),
    // The knob steps it on the radio; the terminal doesn't take it.
    Setting { term: false, ..field!([6, 3, 0], coord_fmt: u8, Kind::Pick(&COORD_FORMATS), Nothing) },
    field!([6, 4, 0], units: u8, Kind::Pick(&UNITS), Nothing),
    field!([6, 5, 0], tz: u8, Kind::Round(TIME_ZONES.len() as u8, zone_text), Nothing),
    field!([6, 6, 0], special),
    field!([6, 8, 0], gps_rxtx: u8, Kind::Pick(&GPS_RXTX), Nothing),
    field!([7, 1, 0], theme: u8, Kind::Pick(&THEMES), Nothing),
    field!([7, 2, 0], layout: u8, Kind::Pick(&LAYOUTS), Nothing),
    field!([7, 3, 2], pon: u8, Kind::Pick(&POWER_ON), Nothing),
    field!([7, 3, 3], boot_card: bool, Kind::Flip(&BOOT_SCREENS), Nothing),
    field!([7, 4, 1], backlight: u8, Kind::Round(BACKLIGHT_TIMES.len() as u8, backlight_text), Nothing),
    field!([7, 4, 2], special),
    field!([7, 4, 3], tx_light: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([7, 4, 4], wake_on: u8, Kind::Pick(&WAKE_ON), Nothing),
    field!([7, 4, 5], keypad_light: u8, Kind::Pick(&KEYPAD_LIGHTS), Nothing),
    field!([7, 5, 1], key_beep: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([7, 5, 2], beep_vol: u8, Kind::Level(1, 3, 1, beep_vol_text), BeepVol),
    field!([7, 5, 3], alert_tones: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([7, 5, 4], arrival_tone: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([7, 6, 0], leds: u8, Kind::Pick(&LED_MODES), Nothing),
    field!([7, 7, 0], special),
    field!([7, 8, 0], auto_off: u8, Kind::Round(AUTO_OFF_TIMES.len() as u8, auto_off_text), Nothing),
    field!([8, 1, 1], tx_timeout: u8, Kind::Level(1, 20, 1, tx_timeout_text), TxTimeout),
    field!([8, 2, 1], vox: bool, Kind::Flip(&ON_OFF), Vox),
    field!([8, 2, 2], vox_level: u8, Kind::Level(1, 9, 1, vox_level_text), Vox),
    field!([8, 2, 3], vox_tail: u8, Kind::Round(VOX_TAILS.len() as u8, vox_tail_text), Vox),
    field!([8, 3, 0], mic_gain: u8, Kind::Level(1, 9, 1, vox_level_text), MicGain),
    field!([8, 4, 2], special),
    field!([8, 5, 1], tail_mode: u8, Kind::Pick(&TAIL_MODES), SlotTones),
    field!([8, 5, 2], tail_phase: u8, Kind::Pick(&TAIL_PHASES), TxTones),
    field!([8, 5, 4], end_tone: u8, Kind::Pick(&END_TONE_NAMES), SlotTones),
    // 50-200 ms in 10 ms steps (DTMF-3)
    field!([8, 6, 3], dtmf_ms: u8, Kind::Level(50, 200, 10, dtmf_ms_text), TxTones),
    field!([8, 6, 6], dtmf_sidetone: bool, Kind::Flip(&ON_OFF), TxTones),
    // 0-1000 ms in 50 ms steps (DTMF-3)
    field!([8, 6, 7], dtmf_delay: u8, Kind::Level(0, 100, 5, dtmf_delay_text), TxTones),
    field!([8, 7, 2], cw_id: u8, Kind::Pick(&CW_ID_MODES), TxTones),
    field!([9, 1, 1], auto_lock: u8, Kind::Round(AUTO_LOCK_TIMES.len() as u8, auto_lock_text), Nothing), // KEY-20
    field!([9, 2, 1], bt: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([9, 2, 4], cable_port: u8, Kind::Pick(&CABLE_PORTS), Nothing),
    field!([9, 2, 5], term_colour: bool, Kind::Flip(&ON_OFF), Nothing),
    field!([9, 2, 6], cable_log: bool, Kind::Flip(&ON_OFF), Nothing),
];

fn row(p: &[u8]) -> Option<&'static Setting> {
    SETTINGS.iter().find(|r| r.path() == p)
}

/// The settings the terminal's `set` takes and `dump` writes, in menu order (TERM-12, TERM-13).
pub(crate) fn settable() -> impl Iterator<Item = &'static [u8]> + Clone {
    SETTINGS.iter().filter(|r| r.term).map(|r| r.path())
}

/// Whether the terminal's `set` takes this path.
pub(crate) fn is_settable(p: &[u8]) -> bool {
    row(p).is_some_and(|r| r.term)
}

fn tell(app: &mut App, t: Tell) {
    match t {
        Tell::Nothing => {}
        Tell::ScanTx => app.out.push(RadioCmd::ScanTx(app.set.scan_tx)),
        Tell::BeepVol => app.out.push(RadioCmd::BeepVol(app.set.beep_vol)),
        Tell::Vox => app.send_vox(),
        Tell::MicGain => app.out.push(RadioCmd::MicGain(app.set.mic_gain.clamp(1, 9))),
        Tell::TxTimeout => app.out.push(RadioCmd::TxTimeout(app.set.tx_timeout_s() as u16)),
        Tell::TxTones => app.send_tx_tones(),
        Tell::SlotTones => app.send_slot_tones(),
    }
}

// The texts of the Round and Level settings.
fn prio_text(v: u8, b: &mut Buf<24>) {
    if v == 0 {
        b.set(b"Off");
    } else {
        let _ = uwrite!(b, "Every {} s", v);
    }
}

fn fm_step_text(v: u8, b: &mut Buf<24>) {
    b.set(FM_STEPS[v as usize % 3].1.as_bytes());
}

fn sleep_text(v: u8, b: &mut Buf<24>) {
    match SLEEP_TIMES[v as usize % SLEEP_TIMES.len()] {
        0 => b.set(b"Off"),
        m => {
            let _ = uwrite!(b, "{} min", m);
        }
    }
}

fn bars_text(v: u8, b: &mut Buf<24>) {
    let _ = uwrite!(b, "{}", SCOPE_BAR_COUNTS[v as usize % 4]);
}

fn scope_step_text(v: u8, b: &mut Buf<24>) {
    b.set(crate::slot::STEPS[v as usize % crate::slot::STEPS.len()].1.as_bytes());
}

fn zone_text(v: u8, b: &mut Buf<24>) {
    let mut t = Buf::<16>::new();
    tz_text(TIME_ZONES[v as usize % TIME_ZONES.len()], &mut t);
    b.set(t.as_bytes());
}

fn backlight_text(v: u8, b: &mut Buf<24>) {
    secs(b, BACKLIGHT_TIMES[v as usize % BACKLIGHT_TIMES.len()] as u32, "Never");
}

fn auto_off_text(v: u8, b: &mut Buf<24>) {
    secs(b, AUTO_OFF_TIMES[v as usize % AUTO_OFF_TIMES.len()] as u32 * 60, "Off");
}

fn auto_lock_text(v: u8, b: &mut Buf<24>) {
    secs(b, AUTO_LOCK_TIMES[v as usize % AUTO_LOCK_TIMES.len()] as u32, "Off");
}

fn vox_tail_text(v: u8, b: &mut Buf<24>) {
    let t = VOX_TAILS[v as usize % VOX_TAILS.len()];
    if t.is_multiple_of(10) {
        let _ = uwrite!(b, "{} s", t / 10);
    } else {
        let _ = uwrite!(b, "{}.{} s", t / 10, t % 10);
    }
}

fn beep_vol_text(v: u8, b: &mut Buf<24>) {
    b.set(BEEP_VOLS[(v.clamp(1, 3) - 1) as usize % BEEP_VOLS.len()].as_bytes());
}

fn vox_level_text(v: u8, b: &mut Buf<24>) {
    let _ = uwrite!(b, "{}", v.clamp(1, 9));
}

fn tx_timeout_text(v: u8, b: &mut Buf<24>) {
    let _ = uwrite!(b, "{} s", v.clamp(1, 20) as u16 * 30);
}

fn dtmf_ms_text(v: u8, b: &mut Buf<24>) {
    let _ = uwrite!(b, "{} ms", v.clamp(50, 200));
}

fn dtmf_delay_text(v: u8, b: &mut Buf<24>) {
    let _ = uwrite!(b, "{} ms", v.min(100) as u32 * 10);
}

/// The live value of a wired setting, and whether it differs from its default (MENU-12): the special cases by
/// name, the rest from [`SETTINGS`].
pub(crate) fn wired(app: &App, dev: &Device, p: &[u8], b: &mut Buf<24>) -> Option<bool> {
    let s = &app.set;
    let pick = |b: &mut Buf<24>, l: &[&str], i: u8| b.set(l[i as usize % l.len()].as_bytes());
    let d = match p {
        [1, 1] => {
            pick(b, &WATCH_TEXT, app.watch);
            app.watch != 1
        }
        [1, 5] => {
            resume_text(b, s.scan_resume);
            s.scan_resume != Settings::DEFAULT.scan_resume
        }
        [6, 9] => {
            if s.fixed_on {
                b.set(&super::lists::grid(s.fixed));
            } else {
                b.set(b"None");
            }
            s.fixed_on
        }
        [4, 9, 2] => {
            let n = s.same_codes.iter().filter(|&&c| c != 0).count();
            if n == 0 {
                b.set(b"None");
            } else {
                let _ = ufmt::uwrite!(b, "{} code{}", n, if n == 1 { "" } else { "s" });
            }
            n != 0
        }
        [6, 6] => {
            pick(b, &CLOCKS, s.clock);
            if s.clock_gps {
                b.push(b", GPS");
            }
            s.clock != 0 || !s.clock_gps
        }
        [7, 3, 2] => {
            pick(b, &POWER_ON, s.pon);
            if s.pon == 2 && !s.pon_text.is_empty() {
                b.set(s.pon_text.as_bytes());
            }
            s.pon != 1
        }
        [7, 7] => {
            let mut t = Buf::<16>::new();
            app.item_text(Item::BatterySave, true, &mut t);
            b.set(t.as_bytes());
            app.bsave != 1
        }
        [8, 7] | [8, 7, 1] => {
            b.set(if s.callsign.is_empty() { b"Empty".as_slice() } else { s.callsign.as_bytes() });
            !s.callsign.is_empty()
        }
        [8, 9] => {
            pick(b, &ON_OFF, s.extend_tx as u8);
            s.extend_tx
        }
        [9, 4] => {
            b.set(s.region_name().as_bytes());
            false
        }
        [9, 6] => {
            b.set(if dev.selftest.ssb_patch { b"Installed".as_slice() } else { b"Not installed" });
            false
        }
        [9, 9] => {
            b.set(VERSION.as_bytes());
            false
        }
        [3, 2, 3] => {
            let d = s.scope_dwell.clamp(1, 100);
            let _ = uwrite!(b, "{}.{} ms", d / 10, d % 10);
            s.scope_dwell != 100
        }
        [5, 3] => {
            pick(b, &ON_OFF, s.aprs_mode as u8);
            s.aprs_mode
        }
        [5, 5, 1] => {
            app.beacon_mode_text(b);
            app.aprs_cfg().beacon != 0
        }
        [5, 5, 4] => {
            pick(b, &crate::aprs::POWERS, app.aprs_cfg().power);
            app.aprs_cfg().power != 0
        }
        [5, 5, 3] => {
            pick(b, &ON_OFF, app.aprs_cfg().after_voice as u8);
            app.aprs_cfg().after_voice
        }
        // The group rows show their setting too (APRS-26).
        [5, 6] | [5, 6, 1] => {
            b.set(app.aprs_cfg().path_name());
            app.aprs_cfg().path != crate::aprs::PATH_DEFAULT
        }
        [5, 6, 2] => {
            let c = app.aprs_cfg();
            b.set(if c.custom_text().is_empty() { b"None".as_slice() } else { c.custom_text() });
            !c.custom_text().is_empty()
        }
        [5, 8] | [5, 8, 1] => {
            pick(b, &ON_OFF, app.aprs_cfg().digi_on() as u8);
            app.aprs_cfg().digi_on()
        }
        [5, 8, 2] => {
            pick(b, &crate::aprs::DIGI_ALIASES, app.aprs_cfg().digi_mycall() as u8);
            app.aprs_cfg().digi_mycall()
        }
        [5, 8, 3] => {
            let _ = uwrite!(b, "{} s", app.aprs_cfg().digi_wait_s());
            app.aprs_cfg().digi_wait_s() != 0
        }
        [5, 7, 1] => {
            b.set(crate::slot::mhz3(app.aprs_freq()).as_bytes());
            app.aprs_cfg().f5k != 0
        }
        // A group's row shows its list's first setting.
        [7, 4] => return wired(app, dev, &[7, 4, 1], b),
        [8, 2] => return wired(app, dev, &[8, 2, 1], b),
        [7, 4, 2] => {
            let _ = uwrite!(b, "{}", s.brightness());
            let mut d = Settings::DEFAULT;
            d.theme = s.theme;
            s.brightness() != d.brightness()
        }
        [9, 1] => {
            let t = AUTO_LOCK_TIMES[s.auto_lock as usize % AUTO_LOCK_TIMES.len()] as u32;
            if t == 0 {
                b.set(b"No auto lock");
            } else {
                secs(b, t, "Off");
            }
            s.auto_lock != 0
        }
        // 0 4 2 (DEC-4, DEC-18): the row in the group shows the frequency while decoding, else Off.
        [4, 2] => {
            if s.pocsag_on {
                b.set(crate::slot::mhz3(s.pocsag_freq).as_bytes());
            } else {
                b.set(b"Off");
            }
            s.pocsag_on
        }
        [4, 2, 2] => {
            b.set(crate::slot::mhz3(s.pocsag_freq).as_bytes());
            s.pocsag_freq != Settings::DEFAULT.pocsag_freq
        }
        [4, 2, 3] => {
            if s.pocsag_cap == 0 {
                b.set(b"None");
            } else {
                let _ = uwrite!(b, "{}", s.pocsag_cap);
            }
            s.pocsag_cap != 0
        }
        [9, 2] => {
            b.set(if dev.bt { b"BT connected".as_slice() } else if s.bt { b"BT on" } else { b"BT off" });
            s.bt
        }
        [9, 7] | [9, 7, 1] => {
            b.set(cal_text(dev.selftest.cal).as_bytes());
            false
        }
        [5, 4] if !s.callsign.is_empty() => {
            b.set(s.callsign.as_bytes());
            b.push(b"-9");
            false
        }
        [8, 4, 2] => {
            pick(b, &crate::settings::TONE_LEVELS, s.tone_level);
            s.tone_level != 1
        }
        [9, 3, 2] => {
            pick(b, &ON_OFF, s.chan_only as u8);
            s.chan_only
        }
        [9, 3] | [9, 3, 1] => {
            pick(b, &PIN_MODES, s.pin_mode);
            s.pin_mode != 0
        }
        [9, 3, 3] => {
            b.set(if s.pin_len > 0 { b"Set" } else { b"None" });
            s.pin_len > 0
        }
        [9, 7, 3] => {
            let t = dev.selftest.battery_trim as i32;
            let _ = uwrite!(b, "{}{}.{} V", if t < 0 { "-" } else { "+" }, t.abs() / 100, ferrite_ui::text::Zero(t.unsigned_abs() % 100, 2));
            t != 0
        }
        _ => {
            let r = row(p)?;
            let v = (r.get)(s);
            match r.kind {
                Kind::Flip(l) => pick(b, l, v),
                Kind::Pick(l) => pick(b, l, v),
                Kind::Round(_, text) | Kind::Level(_, _, _, text) => text(v, b),
                Kind::Special => return None,
            }
            v != (r.get)(&Settings::DEFAULT)
        }
    };
    Some(d)
}

/// The knob or ◀▶ on a wired setting (MENU-16); false if it isn't changeable here. The special cases by name,
/// the rest from [`SETTINGS`].
pub(crate) fn change(app: &mut App, p: &[u8], d: i32) -> bool {
    let s = &mut app.set;
    match p {
        [1, 1] => app.change(Item::Watch, d),
        // 1-10 s, carrier drop, stop on hit (SCAN-5); the next scan takes it
        [1, 5] => s.scan_resume = 1 + cycle(s.scan_resume.clamp(1, 12) - 1, d, 12),
        [8, 4, 2] => {
            s.tone_level = (s.tone_level as i32 + d).clamp(0, 2) as u8;
            app.out.push(RadioCmd::ToneLevel(app.set.tone_level));
        }
        [7, 7] => app.change(Item::BatterySave, d),
        [6, 6] => {
            // 24 h + GPS, 24 h, 12 h + GPS, 12 h
            let v = cycle(s.clock.min(1) * 2 + !s.clock_gps as u8, d, 4);
            (s.clock, s.clock_gps) = (v / 2, v.is_multiple_of(2));
        }
        [7, 1] => {
            if s.theme != crate::settings::NIGHT_RED {
                app.day_theme = s.theme; // HOME-17: the quick menu's PTT B comes back to it from night red
            }
            s.theme = cycle(s.theme, d, THEMES.len());
        }
        [9, 4] => {
            s.region = cycle(s.region.max(1) - 1, d, REGION_LIST.len()) + 1;
            app.region_steps(); // VFO-18
        }
        // 0.1 ms steps up to 1 ms, then 1 ms steps to 10 ms (stock's dwell, the default).
        [3, 2, 3] => {
            let v = s.scope_dwell.clamp(1, 100) as i32;
            let step = if v + d > 10 && v >= 10 { 10 } else { 1 };
            s.scope_dwell = (v + d * step).clamp(1, 100) as u8;
        }
        [5, 3] => {
            let on = !app.set.aprs_mode;
            app.set_aprs_mode(on); // APRS-15
        }
        [5, 5, 1] => app.step_beacon(d),
        [5, 5, 4] => app.set_aprs(crate::aprs::AprsCfg { power: cycle(app.aprs_cfg().power, d, 3), ..app.aprs_cfg() }),
        [5, 5, 3] => app.set_aprs(crate::aprs::AprsCfg { after_voice: !app.aprs_cfg().after_voice, ..app.aprs_cfg() }),
        [5, 6, 1] => {
            let c = app.aprs_cfg();
            app.set_aprs(crate::aprs::AprsCfg { path: cycle(c.path, d, c.path_choices() as usize), ..c });
        }
        [5, 8, 1] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi ^ crate::aprs::DIGI_ON, ..app.aprs_cfg() }),
        [5, 8, 2] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi ^ crate::aprs::DIGI_MYCALL, ..app.aprs_cfg() }),
        [5, 8, 3] => {
            let c = app.aprs_cfg();
            let w = cycle(c.digi_wait_s(), d, crate::aprs::DIGI_WAIT_MAX as usize + 1);
            app.set_aprs(crate::aprs::AprsCfg { digi: c.digi & 0x0F | w << 4, ..c });
        }
        [5, 7, 1] => app.step_aprs_freq(d),
        // 6.25 kHz steps across the VHF and UHF pager bands.
        [4, 2, 2] => s.pocsag_freq = (s.pocsag_freq as i64 + d as i64 * 625).clamp(13_600_000, 52_000_000) as ferrite_core::freq::Freq10,
        [7, 4, 2] => *s.brightness_mut() = (s.brightness() as i32 + d).clamp(1, 10) as u8, // DISP-16, DISP-23
        [8, 2] => return change(app, &[8, 2, 1], d),
        // VOX-10: CW ID needs a callsign
        [8, 7, 2] if s.callsign.as_bytes().is_empty() => app.show_toast(b"CW ID", b"set a callsign first", false),
        [8, 9] => set_extend(app, !app.set.extend_tx),
        [9, 3, 2] if !s.chan_only => app.set_chan_only(), // PIN-5: leaving is hold #
        [9, 3] | [9, 3, 1] => {
            let m = cycle(s.pin_mode, d, PIN_MODES.len());
            if m != 0 && s.pin_len == 0 {
                app.open(Dest::Pin); // a PIN first (PIN-1): the PIN screen sets it, then the lock
                app.screen = Screen::Pin(super::pin::Pin::set(m));
            } else if m == 0 && s.pin_mode != 0 {
                app.open(Dest::Pin); // turning the lock off: the PIN first (PIN-10)
                app.screen = Screen::Pin(super::pin::Pin::off());
            } else {
                s.pin_mode = m;
            }
        }
        _ => {
            let Some(r) = row(p) else { return false };
            let v = (r.get)(s);
            let v = match r.kind {
                Kind::Flip(_) => (v == 0) as u8,
                Kind::Pick(l) => cycle(v, d, l.len()),
                Kind::Round(n, _) => cycle(v, d, n as usize),
                Kind::Level(lo, hi, step, _) => (v.clamp(lo, hi) as i32 + d * step as i32).clamp(lo as i32, hi as i32) as u8,
                Kind::Special => return false,
            };
            (r.set)(s, v);
            tell(app, r.tell);
        }
    }
    true
}

/// # on a wired setting: its default (MENU-18). The special cases by name, the rest from [`SETTINGS`].
pub(crate) fn reset(app: &mut App, p: &[u8]) {
    let d = Settings::DEFAULT;
    let s = &mut app.set;
    match p {
        [1, 1] => app.change(Item::Watch, 1 - app.watch as i32),
        [7, 7] => app.change(Item::BatterySave, 1 - app.bsave as i32),
        [1, 5] => s.scan_resume = d.scan_resume,
        [8, 4, 2] => {
            s.tone_level = d.tone_level;
            app.out.push(RadioCmd::ToneLevel(d.tone_level));
        }
        [6, 9] => s.fixed_on = false,
        [6, 6] => (s.clock, s.clock_gps) = (d.clock, d.clock_gps),
        [8, 9] => s.extend_tx = false,
        [3, 2, 3] => s.scope_dwell = d.scope_dwell,
        [5, 3] if s.aprs_mode != d.aprs_mode => app.set_aprs_mode(d.aprs_mode),
        [5, 5, 1] => app.set_aprs(crate::aprs::AprsCfg { beacon: 0, ..app.aprs_cfg() }),
        [5, 5, 4] => app.set_aprs(crate::aprs::AprsCfg { power: 0, ..app.aprs_cfg() }),
        [5, 5, 3] => app.set_aprs(crate::aprs::AprsCfg { after_voice: false, ..app.aprs_cfg() }),
        [5, 6, 1] => app.set_aprs(crate::aprs::AprsCfg { path: crate::aprs::PATH_DEFAULT, ..app.aprs_cfg() }),
        [5, 8, 1] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi & !crate::aprs::DIGI_ON, ..app.aprs_cfg() }),
        [5, 8, 2] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi & !crate::aprs::DIGI_MYCALL, ..app.aprs_cfg() }),
        [5, 8, 3] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi & 0x0F, ..app.aprs_cfg() }),
        [5, 7, 1] => app.set_aprs(crate::aprs::AprsCfg { f5k: 0, ..app.aprs_cfg() }),
        [4, 2, 2] => s.pocsag_freq = d.pocsag_freq,
        [4, 2, 3] => s.pocsag_cap = d.pocsag_cap,
        // # forgets the custom path; a path set to it goes back to the default (APRS-26)
        [5, 6, 2] => {
            let c = app.aprs_cfg();
            let path = if c.path == crate::aprs::PATH_CUSTOM { crate::aprs::PATH_DEFAULT } else { c.path };
            app.set_aprs(crate::aprs::AprsCfg { custom: [0; crate::aprs::CUSTOM_LEN], path, ..c });
        }
        [7, 4, 2] => {
            let night = s.theme == NIGHT_RED;
            *s.brightness_mut() = if night { d.brightness_night } else { d.brightness };
        }
        [8, 2] => reset(app, &[8, 2, 1]),
        _ => {
            if let Some(r) = row(p).filter(|r| !matches!(r.kind, Kind::Special)) {
                (r.set)(s, (r.get)(&d));
                tell(app, r.tell);
            }
        }
    }
}

/// "Auto", "12 dB".
pub(crate) fn agc_text(v: u8, b: &mut Buf<24>) {
    if v == 0 {
        b.set(b"Auto");
    } else {
        b.clear();
        let _ = uwrite!(b, "{} dB", (v.min(crate::settings::AGC_MAX) - 1) * 4);
    }
}

pub(crate) fn cal_text(c: CalState) -> &'static str {
    match c {
        CalState::Unknown => "Not checked",
        CalState::Ok => "Valid",
        CalState::Blank => "Blank: TX locked",
        CalState::Invalid => "Invalid: TX locked",
        CalState::CrcMismatch => "Changed: TX locked",
    }
}

/// "15 s", "2 min", "1 h", or `zero` for 0.
fn secs(b: &mut Buf<24>, t: u32, zero: &str) {
    match t {
        0 => b.set(zero.as_bytes()),
        1..=59 => {
            let _ = uwrite!(b, "{} s", t);
        }
        60..=3599 => {
            let _ = uwrite!(b, "{} min", t / 60);
        }
        _ => {
            let _ = uwrite!(b, "{} h", t / 3600);
        }
    }
}

/// A resume rule's text (SCAN-5): "5 s", "Carrier drop", "Stop on hit".
pub(crate) fn resume_text(b: &mut Buf<24>, code: u8) {
    match code {
        11 => b.set(b"Carrier drop"),
        12 => b.set(b"Stop on hit"),
        c => {
            let _ = uwrite!(b, "{} s", c.clamp(1, 10));
        }
    }
}

fn cycle(v: u8, d: i32, n: usize) -> u8 {
    (v as i32 + d).rem_euclid(n as i32) as u8
}

/// Extend TX: turning it on shows the warning first, once (SETUP-8, TX-2).
pub(crate) fn set_extend(app: &mut App, on: bool) {
    if on && !app.set.extend_warned {
        app.open(Dest::ExtendTx);
        app.screen = Screen::ExtendTx(ExtendTx);
    } else {
        app.set.extend_tx = on;
    }
}

impl View for Group {
    fn name(self) -> &'static str {
        "Settings group"
    }

    fn mockup(self) -> Option<&'static str> {
        (self.sub == 0).then_some("MenuGroup.png")
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        if self.sub == 0 {
            b.set(SHORT[(self.g as usize).clamp(1, 9) - 1].as_bytes());
        } else {
            // The list's own name, whole words that fit.
            let n = items(self.g, 0).get(self.sub as usize - 1).map_or("", |e| e.name()).as_bytes();
            let cut = if n.len() <= 10 { n.len() } else { n[..11].iter().rposition(|&c| c == b' ').unwrap_or(10) };
            b.set(&n[..cut]);
        }
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let n = self.items().len() as u8;
        let (p, pl) = self.path(self.row);
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => self.set_row(app, (self.row + n - 1) % n),
            (Key::Down, Short | Repeat) => self.set_row(app, (self.row + 1) % n),
            (Key::Digit(d), Short) if d >= 1 && d <= n => {
                self.set_row(app, d - 1);
                let g = Group { row: d - 1, ..self };
                g.open(app, false);
            }
            (Key::Digit(_), Short) => app.show_toast(b"No item", b"with that number", false),
            (Key::Menu, Short) => self.open(app, true),
            (Key::Hash, Short) => reset(app, &p[..pl]),
            (Key::Star, Short) => app.do_undo(),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        self.turn(app, d);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        self.draw(app, lcd, cx);
    }

    fn hints(self, app: &App) -> Hints {
        if app.set_undo.is_some() || app.undo.is_some() {
            Hints::new(&[("Knob", "Change"), ("*", "Undo"), ("BACK", "Back")]) // MENU-22
        } else {
            Hints::new(&[("Knob", "Change"), ("OK", "Open"), ("BACK", "Back")])
        }
    }
}

/// The value a setting shows, as a hash (MENU-22's undo compares it).
fn shown(app: &App, p: &[u8]) -> u32 {
    let mut b = Buf::<24>::new();
    let _ = wired(app, &Device::default(), p, &mut b);
    hash(&b.as_bytes())
}

/// MENU-22: steps the setting back the other way until it shows what it did before the run of changes (a
/// toggle flips back, a list walks back); bounded, so a value that can't come back stops trying.
pub(crate) fn undo_setting(app: &mut App, u: &crate::SetUndo) {
    let p = &u.path[..u.len as usize];
    for _ in 0..64 {
        if shown(app, p) == u.was || !change(app, p, -(u.dir as i32)) {
            break;
        }
    }
}

impl Group {
    /// The knob or ◀▶: change the highlighted value, or say why it can't.
    fn turn(self, app: &mut App, d: i32) {
        let (p, pl) = self.path(self.row);
        if let Some(why) = greyed(app, &p[..pl]) {
            app.show_toast(self.items()[self.row as usize].name().as_bytes(), why.as_bytes(), false);
        } else {
            let p = &p[..pl];
            // MENU-22: a radio-wide setting changed here offers "* Undo" (the slot's own items keep theirs).
            let was = shown(app, p);
            if change(app, p, d) && !matches!(p, [1, 1] | [7, 7]) && shown(app, p) != was {
                app.keep_set_undo(p, d, was);
            }
        }
    }

    /// Row `i` as drawn: its name, the value it shows, the changed-from-default dot and whether it is greyed
    /// (MENU-14: a greyed row not selected shows why instead of its value). The UI readback reads it too (LINK-19).
    pub(crate) fn row(self, app: &App, dev: &Device, store: &impl ChannelStore, i: u8) -> (&'static str, Buf<24>, bool, bool) {
        let e = self.items()[i as usize];
        let (p, pl) = self.path(i);
        let mut v = Buf::<24>::new();
        let dot = match wired(app, dev, &p[..pl], &mut v) {
            Some(d) => d,
            None => {
                match &p[..pl] {
                    [2, 1] => {
                        let _ = uwrite!(v, "{} channels", store.count());
                    }
                    [4, 1] if app.inbox > 0 => {
                        let _ = uwrite!(v, "{} new", app.inbox);
                    }
                    [5, 4] => v.set(b"no callsign"),
                    _ => v.set(e.value().as_bytes()),
                }
                false
            }
        };
        let grey = greyed(app, &p[..pl]);
        if let (Some(why), true) = (grey, i != self.row) {
            v.set(why.as_bytes()); // MENU-14: greyed items say why
        }
        (e.name(), v, dot, grey.is_some())
    }

    /// The help box: the highlighted item's path ("MENU 0 7 1") and help line (or why it is greyed).
    pub(crate) fn help(self, app: &App) -> (Buf<20>, &'static str) {
        let (p, pl) = self.path(self.row);
        let mut path = Buf::<20>::new();
        path.push(b"MENU 0");
        for x in &p[..pl] {
            let _ = uwrite!(path, " {}", x);
        }
        (path, greyed(app, &p[..pl]).unwrap_or(self.items().get(self.row as usize).map_or("", |e| e.help())))
    }

    fn draw(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.new_page(lcd, hash(&(self.g, self.sub)));
        for i in 0..self.items().len() {
            let (name, v, dot, grey) = self.row(app, cx.dev, cx.store, i as u8);
            let r = Row { num: Some(i as u8 + 1), name: name.as_bytes(), value: v.as_bytes(), dot, selected: i as u8 == self.row, grey };
            app.list_row(lcd, i, 23 + 22 * i as i32, 22, &r);
        }
        let (path, help) = self.help(app);
        app.help_box(lcd, 226, 46, help.as_bytes(), path.as_bytes());
    }

    /// OK (or its digit) on the highlighted row: its screen, its list, or (OK only) a toggle.
    fn open(self, app: &mut App, ok: bool) {
        let (p, pl) = self.path(self.row);
        let p = &p[..pl];
        if let Some(why) = greyed(app, p) {
            return app.show_toast(self.items()[self.row as usize].name().as_bytes(), why.as_bytes(), false);
        }
        match p {
            [8, 7, 1] => {
                super::setup::callsign(app);
            }
            // OK on Text edits it (16 characters); the knob still picks off, callsign or text.
            [7, 3, 2] if ok && app.set.pon == 2 => {
                app.text = Buf::from(app.set.pon_text.as_bytes());
                app.open(Dest::Text(super::text::TextFor::PowerOn));
            }
            [8, 9] if ok => set_extend(app, !app.set.extend_tx),
            // OK types the capcode (DEC-4), the clock (GPS-23) or the custom path (APRS-26); the knob still
            // changes 0 6 6's format.
            [4, 2, 3] => {
                app.text = Buf::new();
                if app.set.pocsag_cap != 0 {
                    let _ = uwrite!(app.text, "{}", app.set.pocsag_cap);
                }
                app.open(Dest::Text(super::text::TextFor::Capcode));
            }
            [6, 6] if ok => {
                app.text = Buf::new();
                app.open(Dest::Text(super::text::TextFor::Clock));
            }
            [5, 6, 2] => {
                app.text = Buf::from(app.aprs_custom_path());
                app.open(Dest::Text(super::text::TextFor::AprsPath));
            }
            _ => {
                if let Some(d) = nav::menu0(p) {
                    app.open(d);
                } else if ok && !change(app, p, 1) {
                    let mut b = Buf::<20>::new();
                    b.push(b"MENU 0");
                    for x in p {
                        let _ = uwrite!(b, " {}", x);
                    }
                    app.show_toast(b.as_bytes(), b"setting comes next", false);
                }
            }
        }
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    /// MENU-3: at most 9 items per list, every list on one screen, and a group's third level is the deepest
    /// (MENU 0 g s i): sub-lists hang only off a group's own rows and have no sub-lists of their own.
    #[test]
    fn menu_shape() {
        for (g, items) in GROUP_ITEMS.iter().enumerate() {
            assert!(!items.is_empty() && items.len() <= 9, "group {} has {} items", g + 1, items.len());
        }
        for ((g, s), items) in SUBS {
            assert!((1..=9).contains(&g) && (1..=9).contains(&s), "sub-list ({g}, {s})");
            assert!(s as usize <= GROUP_ITEMS[g as usize - 1].len(), "({g}, {s}) hangs off a row its group has");
            assert!(!items.is_empty() && items.len() <= 9, "({g}, {s}) has {} items", items.len());
        }
        // SETTINGS: every path is a menu item, once.
        for (i, r) in SETTINGS.iter().enumerate() {
            let p = r.path();
            let list = items(p[0], if p.len() == 3 { p[1] } else { 0 });
            assert!(p.last().is_some_and(|&x| x >= 1 && x as usize <= list.len()), "{p:?} is not a menu item");
            assert!(SETTINGS[..i].iter().all(|q| q.path() != p), "{p:?} twice");
        }
        // Each item is "name|value|help": three parts, a name always.
        for e in GROUP_ITEMS.iter().copied().chain(SUBS.iter().map(|x| x.1)).flatten() {
            assert!(e.0.matches('|').count() == 2 && !e.name().is_empty(), "item {:?}", e.0);
        }
    }
}
