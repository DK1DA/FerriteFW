//! A settings group (MENU 0 n) and a third-level list (MENU 0 n m): one list widget for both (manual chapter
//! 11, MENU-3 to MENU-24; MenuGroup mock-up). The lists are data: [`GROUP_ITEMS`] and [`SUBS`] hold every item
//! of the manual's Appendix A with its value and help line; [`wired`] gives the live value of the settings that exist
//! so far, and [`change`] changes them. The rest show their default, read only, until their feature lands.
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

/// One item: name, value shown until it is wired (a list shows a summary), help line.
pub(crate) type Entry = (&'static str, &'static str, &'static str);

/// SND-3: the beep volume (0 7 5 2), three DAC levels (decision 100).
const BEEP_VOLS: [&str; 3] = ["Low", "Mid", "High"];
const G1: [Entry; 8] = [
    ("Watch", "A+B", "Which slots are heard besides the active one."),
    ("Watch behaviour", "Heard", "Watched audio and pause time."),
    ("Background job", "Off", "A zone scan or decoder on a watched slot."),
    ("Scan target", "Current zone", "What SIDE2 scans."),
    ("Resume", "5 s", "When a stopped scan moves on."),
    ("Scan options", "Up, 1 s", "Direction, dwell, priority, inputs."),
    ("Range presets", "4 presets", "Named ranges to scan."),
    ("Skip list", "Empty", "Frequencies * skipped in this scan."),
];
const G2: [Entry; 5] = [
    ("Browse", "", "Zones, all channels, nearby, recent."),
    ("Add repeater", "", "Frequency, shift, tone, name."),
    ("Zones", "", "Name, scan options, priority channels."),
    ("Nearby", "Off", "Sort channels by distance."),
    ("Channel display", "Name", "Name, number or frequency on home."),
];
const G3: [Entry; 6] = [
    ("FM broadcast", "", "Listen now; hold MENU there for its settings."),
    ("Band scope", "60 bars", "Bars, step, speed, trigger, waterfall."),
    ("Frequency counter", "", "Find a nearby transmitter's frequency."),
    ("HF mode", "", "Below 64 MHz. Everything else pauses."),
    ("Crossband repeater", "", "Repeat one slot's band on another's."),
    ("Tone search", "", "Find the CTCSS or DCS in use."),
];
const G4: [Entry; 9] = [
    ("Event log", "", "Every decode and alert: the inbox."),
    ("POCSAG", "439.9875", "Pager frequency, baud, capcodes."),
    ("DSC", "Off", "Marine distress calls, receive only."),
    ("AIS", "Off", "Ships near you."),
    // 5 was "Sensor reader" (with the jamming alarm), dropped (decision 75); the number stays empty so the
    // rest of the group keeps its paths.
    ("-", "", "Not used."),
    ("Selcall, MDC1200", "Off", "Decode selcall and MDC1200 IDs."),
    ("DTMF decode", "Off", "Show and log DTMF sequences."),
    ("Popups", "On", "A 3 s toast for each decode."),
    ("Weather alerts", "On", "SAME, location codes, tone, channel."),
];
const G5: [Entry; 9] = [
    ("Heard", "", "Stations heard, nearest first."),
    ("Messages", "", "Messages sent to you and by you."),
    ("APRS mode", "Off", "Slot C becomes the APRS slot."),
    ("My station", "SSID 9", "SSID, symbol, comment, position."),
    ("Beacon", "Off", "When to beacon: manual, timed, smart."),
    ("Path", "WIDE1-1,WIDE2-1", "The digipeater path."),
    ("Radio", "144.390", "APRS frequency, delays, levels."),
    ("Digipeat", "Off", "Relay others' packets."),
    // 9 was KISS TNC alone; now the packet group (decision 77): KISS TNC moved to 0 5 9 1, the mailbox beside it.
    ("Packet", "", "KISS TNC and the mailbox."),
];
const G6: [Entry; 9] = [
    ("GPS", "On", "The GPS receiver."),
    ("Position", "", "Fix, satellites, position, grid."),
    ("Coordinate format", "Deg, min", "How positions are written."),
    ("Units", "Metric", "Distance, altitude and speed units."),
    ("Time zone", "UTC", "The clock's offset from UTC."),
    ("Clock", "24 h", "12 or 24 h, from GPS; OK sets it by hand."),
    ("Waypoints", "", "Marked places; go to one."),
    ("GPS during RX/TX", "Always on", "Turn the GPS off around signals."),
    ("Fixed position", "None", "Used when there is no fix."),
];
const G7: [Entry; 9] = [
    ("Theme", "Dark", "Dark, daylight or night red."),
    ("Home layout", "Watch", "What home shows."),
    ("Display", "Bars", "S-meter, power-on message, boot screen."),
    ("Backlight", "15 s", "Timeout, brightness, wake."),
    ("Sounds", "Beep on", "Key beep, volume, alert tones."),
    ("LEDs", "RX, TX and heartbeat", "And a flash while the screen is dark."),
    ("Battery Save", "Level 1", "Receiver sleep: longer life, slower."),
    ("Auto power off", "Off", "Off after a time without key or signal."),
    ("Menu timeout", "30 s", "An idle menu returns home."),
];
const G8: [Entry; 9] = [
    ("Timeout", "120 s", "Transmit timeout and its pre-alert."),
    ("VOX", "Off", "Voice-operated transmit."),
    ("Mic gain", "5", "Microphone level, 1-9."),
    ("Tones sent", "1750 Hz", "Tone burst, subtone level."),
    ("Tail elimination", "Off", "Squelch tail, end tone (roger beep)."),
    ("DTMF", "100 ms", "Dialer, memories, timing, PTT ID."),
    ("Callsign and ID", "Empty", "Your callsign, and CW ID."),
    ("Busy lockout default", "Off", "For new channels and the VFO."),
    ("Extend TX", "Off", "Transmit beyond the amateur bands."),
];
const G9: [Entry; 9] = [
    ("Keys and lock", "No auto lock", "Auto lock, lock covers, key test, shortcuts."),
    ("Connections", "BT off", "Bluetooth, approved devices, link."),
    ("PIN and lock", "Off", "PIN lock, channel-only mode, PIN."),
    ("Region", "Not set", "Your amateur bands and defaults."),
    ("Receiver", "Tail 150 ms", "Advanced squelch, tail, discriminator."),
    ("SSB patch", "", "USB, LSB and CW in HF mode."),
    ("Calibration and trims", "", "View calibration, trim crystal, battery."),
    ("Setup and reset", "", "Run setup again, reset settings."),
    ("About", "", "Version, identity, checks, battery."),
];

/// The nine groups' items (MENU 0 1 to MENU 0 9).
pub(crate) const GROUP_ITEMS: [&[Entry]; 9] = [&G1, &G2, &G3, &G4, &G5, &G6, &G7, &G8, &G9];
/// Short names for the status bar (up to 10 characters).
const SHORT: [&str; 9] = ["Scan", "Channels", "Tools", "Decoders", "APRS", "GPS", "Display", "Transmit", "System"];

/// The third-level lists, by group and item.
pub(crate) const SUBS: [((u8, u8), &[Entry]); 29] = [
    ((1, 2), &[
        ("Watched audio", "Heard", "Heard, muted, per slot."),
        ("Pause time", "1 min", "# on home pauses watch this long."),
    ]),
    ((1, 6), &[
        ("Direction", "Up", "Scan up or down."),
        ("Dwell", "1 s", "Wait before resuming."),
        ("Scan inputs", "Off", "Also a channel's TX frequency."),
        ("Return to", "Start", "Where a stopped scan returns."),
        ("Priority check", "Every 2 s", "Look at the priority channels."),
    ]),
    // The FM screen's options (BCR-3): on the radio 0 3 1 opens the screen and its digits 1-5 reach these; the
    // terminal lists them here (TERM-12).
    ((3, 1), &[
        ("Presets", "Empty", "20 stations."),
        ("Step", "100 kHz", "Tuning step."),
        ("AGC", "Auto", "Gain control."),
        ("Radio monitor", "On", "A slot opening interrupts the broadcast."),
        ("Sleep timer", "Off", "End the broadcast after a time."),
    ]),
    ((3, 2), &[
        ("Bars", "60", "Bars across; span is bars x step."),
        ("Step", "25 kHz", "Distance between bars."),
        ("Sweep speed", "1 ms", "Time on each point."),
        ("Trigger", "Off", "Over the line: monitor or log."),
        ("Waterfall row", "Off", "History under the scope."),
        ("Open band scope", "", "Sweep around the active slot. Home: hold 1."),
    ]),
    ((4, 2), &[
        ("Decode", "Off", "Pagers on a slot tuned to the frequency."),
        ("Frequency", "439.9875", "The pager channel; 512, 1200 and 2400 baud."),
        ("Capcode filter", "None", "Only this capcode: OK types it, # clears it."),
    ]),
    ((4, 3), &[("DSC", "Off", "Decode DSC on channel 70."), ("Own MMSI", "None", "Your MMSI, receive only.")]),
    ((4, 4), &[("AIS", "Off", "161.975 and 162.025, in turn."), ("Range filter", "25 nm", "Off, 5, 10, 25, 50 nm; needs a fix.")]),
    ((4, 9), &[
        ("SAME", "On", "Decode weather alerts."),
        ("Location codes", "None", "Up to 8 FIPS or CLC codes."),
        ("Channel", "Auto", "The weather channel to watch."),
    ]),
    ((5, 4), &[
        ("SSID", "9", "0-15 after your callsign."),
        ("Symbol", "Car", "Your map symbol."),
        ("Comment", "Empty", "Up to 40 characters."),
        ("Position source", "GPS", "GPS, or the fixed position."),
        ("Fixed position", "None", "Used without GPS."),
        ("Ambiguity", "Exact", "Digits dropped from your position."),
        ("Mic-E status", "En route", "Status in Mic-E beacons."),
    ]),
    ((5, 5), &[
        ("Mode", "Off", "Manual, timed, distance or smart."),
        ("Smart settings", "Standard", "Rates, speeds, turn angle."),
        ("After voice TX", "Off", "Beacon after you talk."),
        ("Power", "Low", "Beacon and message power."),
    ]),
    ((5, 6), &[("Path", "WIDE1-1,WIDE2-1", "None, WIDE1-1, WIDE1-1,WIDE2-1, or custom."), ("Custom path", "None", "Your own path, like WIDE2-2: OK types it.")]),
    ((5, 7), &[
        ("Frequency", "144.390", "The APRS channel."),
        ("TX delay", "300 ms", "Carrier before data."),
        ("TX level", "Mid", "Modulation level."),
        ("RX level", "Auto", "Decoder input level."),
        ("Mute speaker", "On", "Mute packets on the speaker."),
        ("Ringer on message", "On", "Ring for a new message."),
        ("Heard popup", "Off", "A toast per station heard."),
    ]),
    ((5, 8), &[
        ("Digipeat", "Off", "Relay others' packets."),
        ("Aliases", "WIDE1-1", "WIDE1-1, and your call if chosen."),
        ("Wait", "0 s", "Delay before relaying, 0-5 s."),
    ]),
    ((5, 9), &[
        ("KISS TNC", "Off", "Raw KISS on the cable or Bluetooth."),
        ("Mailbox", "Off", "Answer packet connects to your SSID."),
        ("Mailbox SSID", "1", "The SSID callers connect to."),
        ("Mailbox frequency", "None", "Where the mailbox listens."),
        ("Bulletins", "On", "Callers may post bulletins."),
    ]),
    ((7, 3), &[
        // 1 was "Battery as": the bar shows an icon only, % and volts on Status (SBAR-19); S-meter took its row
        ("S-meter", "Bars", "Bars or dBm."),
        ("Power-on message", "Callsign", "Off, callsign or text."),
        ("Boot screen", "Logo", "Logo, or the owner card."),
    ]),
    ((7, 4), &[
        ("Timeout", "1 min", "Backlight off after this."),
        ("Brightness", "7", "1-10; night red keeps its own."),
        ("On transmit", "On", "Light up while transmitting."),
        ("Wake screen on", "Keys and RX", "What wakes the screen."),
        ("Keypad light", "With screen", "Off, with the screen, on key press, always."),
    ]),
    ((7, 5), &[
        ("Key beep", "On", "Beep on key presses."),
        ("Beep volume", "3", "1-5."),
        ("Alert tones", "On", "Tones for alerts."),
        ("Arrival tone", "On", "A tone on reaching a waypoint."),
    ]),
    ((8, 1), &[("TX timeout", "120 s", "Transmit stops after this."), ("Pre-alert", "10 s", "Warning before the timeout.")]),
    ((8, 2), &[("VOX", "Off", "Transmit when you talk."), ("Level", "5", "1-9; keys the active slot."), ("Tail", "2 s", "Hang time after speech.")]),
    ((8, 4), &[
        ("Tone burst", "1750 Hz", "PTT + SIDE2 sends it."),
        ("Subtone level", "Normal", "CTCSS/DCS strength: raise for a deaf repeater."),
    ]),
    ((8, 5), &[
        ("Mode", "Off", "Receive, transmit or both."),
        ("Phase", "55 Hz", "55 Hz or a phase shift."),
        // 3 was "Repeater tail delay", dropped (decision 63); the number stays empty so End tone keeps 0 8 5 4.
        ("-", "", "Not used."),
        ("End tone", "Off", "A short tone as you let go of PTT."),
    ]),
    ((8, 6), &[
        ("Dialer", "", "Type and send DTMF."),
        ("Memories", "", "16 stored sequences."),
        ("Speed", "100 ms", "Each tone and each gap."),
        ("PTT ID on VFOs", "Off", "Send the ID code at PTT."),
        ("ID code", "Empty", "The radio's DTMF ID."),
        ("Sidetone", "On", "Hear the digits you send."),
        ("First digit delay", "300 ms", "Wait after key-up so a repeater opens."),
    ]),
    ((8, 7), &[("Callsign", "Empty", "For APRS, CW ID, power-on message."), ("CW ID", "Off", "Send your callsign in CW.")]),
    ((9, 1), &[
        ("Auto lock", "Off", "Lock the keys after a time."),
        ("Lock covers", "Keys", "Keys, or keys and PTT."),
        ("Key test", "", "Each key's code, name and press."),
        ("Shortcuts", "", "What holding each digit on home does."),
    ]),
    ((9, 2), &[
        ("Bluetooth", "Off", "The Bluetooth link to the web app."),
        ("Link status", "", "Cable and Bluetooth now."),
        ("Debug console", "Off", "Off, cable or Bluetooth."),
        ("Cable port", "Auto", "Link, terminal or KISS."),
        ("Terminal colour", "Off", "ANSI colour on the cable terminal."),
        ("Cable log", "Off", "Plain log on the cable at power on."),
    ]),
    ((9, 3), &[("PIN lock", "Off", "Ask for the PIN at power on, or unlock too."), ("Channel-only mode", "Off", "Hide VFO and editing."), ("Change PIN", "None", "Type a new PIN; the old one first.")]),
    ((9, 5), &[
        ("Advanced squelch", "", "Thresholds per band."),
        ("Squelch tail", "150 ms", "Audio after the signal drops."),
        ("Discriminator mode", "Off", "Flat audio for decoders."),
    ]),
    ((9, 7), &[
        ("Calibration", "", "The factory calibration, read only."),
        ("Crystal trim", "0 ppm", "Centre a known signal."),
        ("Battery reading trim", "+0.00 V", "Match the reading to a meter."),
    ]),
    ((9, 8), &[
        ("Run setup again", "", "The first-boot questions."),
        ("Reset", "", "Everything but calibration."),
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

/// The live value of a wired setting, and whether it differs from its default (MENU-12).
pub(crate) fn wired(app: &App, dev: &Device, p: &[u8], b: &mut Buf<24>) -> Option<bool> {
    let s = &app.set;
    let pick = |b: &mut Buf<24>, l: &[&str], i: u8| b.set(l[i as usize % l.len()].as_bytes());
    let d = match p {
        [1, 1] => {
            pick(b, &WATCH_TEXT, app.watch);
            app.watch != 1
        }
        [6, 1] => {
            pick(b, &ON_OFF, s.gps_on as u8);
            !s.gps_on
        }
        [6, 8] => {
            pick(b, &GPS_RXTX, s.gps_rxtx);
            s.gps_rxtx != 0
        }
        [6, 3] => {
            pick(b, &COORD_FORMATS, s.coord_fmt);
            s.coord_fmt != Settings::DEFAULT.coord_fmt
        }
        [6, 4] => {
            pick(b, &UNITS, s.units);
            s.units != 0
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
        [6, 5] => {
            let mut t = Buf::<16>::new();
            s.tz_text(&mut t);
            b.set(t.as_bytes());
            s.tz != TZ_DEFAULT
        }
        [6, 6] => {
            pick(b, &CLOCKS, s.clock);
            if s.clock_gps {
                b.push(b", GPS");
            }
            s.clock != 0 || !s.clock_gps
        }
        [7, 1] => {
            pick(b, &THEMES, s.theme);
            s.theme != 0
        }
        [7, 2] => {
            pick(b, &LAYOUTS, s.layout);
            s.layout != 0
        }
        [7, 3, 2] => {
            pick(b, &POWER_ON, s.pon);
            if s.pon == 2 && !s.pon_text.is_empty() {
                b.set(s.pon_text.as_bytes());
            }
            s.pon != 1
        }
        [7, 3, 3] => {
            pick(b, &BOOT_SCREENS, s.boot_card as u8);
            s.boot_card
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
        [3, 1, 2] => {
            b.set(FM_STEPS[s.fm_step as usize % 3].1.as_bytes());
            s.fm_step != 1
        }
        [3, 1, 3] => {
            agc_text(s.fm_agc, b);
            s.fm_agc != 0
        }
        [3, 1, 4] => {
            pick(b, &ON_OFF, s.fm_monitor as u8);
            !s.fm_monitor
        }
        [3, 1, 5] => {
            match SLEEP_TIMES[s.fm_sleep as usize % SLEEP_TIMES.len()] {
                0 => b.set(b"Off"),
                m => {
                    let _ = uwrite!(b, "{} min", m);
                }
            }
            s.fm_sleep != 0
        }
        [3, 2, 1] => {
            let _ = uwrite!(b, "{}", SCOPE_BAR_COUNTS[s.scope_bars as usize % 4]);
            s.scope_bars != 1
        }
        [3, 2, 2] => {
            b.set(crate::slot::STEPS[s.scope_step as usize % crate::slot::STEPS.len()].1.as_bytes());
            s.scope_step != 11
        }
        [3, 2, 3] => {
            let d = s.scope_dwell.clamp(1, 100);
            let _ = uwrite!(b, "{}.{} ms", d / 10, d % 10);
            s.scope_dwell != 100
        }
        [3, 2, 4] => {
            pick(b, &SCOPE_TRIGGERS, s.scope_trig);
            s.scope_trig != 0
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
        [4, 8] => {
            pick(b, &ON_OFF, s.popups as u8);
            !s.popups
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
        [3, 2, 5] => {
            pick(b, &ON_OFF, s.scope_wf as u8);
            s.scope_wf
        }
        [9, 2, 5] => {
            pick(b, &ON_OFF, s.term_colour as u8);
            s.term_colour
        }
        [9, 2, 6] => {
            pick(b, &ON_OFF, s.cable_log as u8);
            s.cable_log
        }
        [7, 4] | [7, 4, 1] => {
            secs(b, BACKLIGHT_TIMES[s.backlight as usize % BACKLIGHT_TIMES.len()] as u32, "Never");
            s.backlight != Settings::DEFAULT.backlight
        }
        [7, 4, 2] => {
            let _ = uwrite!(b, "{}", s.brightness());
            let mut d = Settings::DEFAULT;
            d.theme = s.theme;
            s.brightness() != d.brightness()
        }
        [7, 4, 3] => {
            pick(b, &ON_OFF, s.tx_light as u8);
            !s.tx_light
        }
        [7, 4, 4] => {
            pick(b, &WAKE_ON, s.wake_on);
            s.wake_on != Settings::DEFAULT.wake_on
        }
        [7, 4, 5] => {
            pick(b, &KEYPAD_LIGHTS, s.keypad_light);
            s.keypad_light != Settings::DEFAULT.keypad_light
        }
        [7, 5, 1] => {
            pick(b, &ON_OFF, s.key_beep as u8);
            !s.key_beep
        }
        [7, 5, 2] => {
            pick(b, &BEEP_VOLS, s.beep_vol.clamp(1, 3) - 1);
            s.beep_vol != Settings::DEFAULT.beep_vol
        }
        [7, 5, 3] => {
            pick(b, &ON_OFF, s.alert_tones as u8);
            !s.alert_tones
        }
        [7, 5, 4] => {
            pick(b, &ON_OFF, s.arrival_tone as u8);
            !s.arrival_tone
        }
        [8, 2] | [8, 2, 1] => {
            pick(b, &ON_OFF, s.vox as u8);
            s.vox
        }
        [8, 2, 2] => {
            let _ = uwrite!(b, "{}", s.vox_level.clamp(1, 9));
            s.vox_level != Settings::DEFAULT.vox_level
        }
        [8, 2, 3] => {
            let t = VOX_TAILS[s.vox_tail as usize % VOX_TAILS.len()];
            if t.is_multiple_of(10) {
                let _ = uwrite!(b, "{} s", t / 10);
            } else {
                let _ = uwrite!(b, "{}.{} s", t / 10, t % 10);
            }
            s.vox_tail != Settings::DEFAULT.vox_tail
        }
        [7, 6] => {
            pick(b, &LED_MODES, s.leds);
            s.leds != Settings::DEFAULT.leds
        }
        [7, 8] => {
            secs(b, AUTO_OFF_TIMES[s.auto_off as usize % AUTO_OFF_TIMES.len()] as u32 * 60, "Off");
            s.auto_off != 0
        }
        [9, 1] | [9, 1, 1] => {
            let t = AUTO_LOCK_TIMES[s.auto_lock as usize % AUTO_LOCK_TIMES.len()] as u32;
            if p.len() == 2 && t == 0 {
                b.set(b"No auto lock");
            } else {
                secs(b, t, "Off");
            }
            s.auto_lock != 0
        }
        [8, 6, 6] => {
            pick(b, &ON_OFF, s.dtmf_sidetone as u8);
            !s.dtmf_sidetone
        }
        [8, 5, 1] => {
            pick(b, &TAIL_MODES, s.tail_mode);
            s.tail_mode != 0
        }
        [8, 5, 2] => {
            pick(b, &TAIL_PHASES, s.tail_phase);
            s.tail_phase != 0
        }
        [8, 5, 4] => {
            pick(b, &END_TONE_NAMES, s.end_tone);
            s.end_tone != 0
        }
        [8, 6, 3] => {
            let _ = uwrite!(b, "{} ms", s.dtmf_ms.clamp(50, 200));
            s.dtmf_ms != Settings::DEFAULT.dtmf_ms
        }
        [8, 6, 7] => {
            let _ = uwrite!(b, "{} ms", s.dtmf_delay.min(100) as u32 * 10);
            s.dtmf_delay != Settings::DEFAULT.dtmf_delay
        }
        [8, 7, 2] => {
            pick(b, &CW_ID_MODES, s.cw_id);
            s.cw_id != 0
        }
        [4, 9, 1] => {
            pick(b, &ON_OFF, s.same as u8);
            !s.same
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
        [4, 2, 1] => {
            pick(b, &ON_OFF, s.pocsag_on as u8);
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
        [9, 2, 1] => {
            pick(b, &ON_OFF, s.bt as u8);
            s.bt
        }
        [9, 2, 4] => {
            pick(b, &CABLE_PORTS, s.cable_port);
            s.cable_port != 0
        }
        [5, 9, 1] => {
            pick(b, &crate::settings::KISS_PORTS, s.kiss_port);
            s.kiss_port != 0
        }
        [9, 2] => {
            b.set(if dev.bt { b"BT connected".as_slice() } else if s.bt { b"BT on" } else { b"BT off" });
            s.bt
        }
        [9, 7] => {
            b.set(cal_text(dev.selftest.cal).as_bytes());
            false
        }
        [5, 4] if !s.callsign.is_empty() => {
            b.set(s.callsign.as_bytes());
            b.push(b"-9");
            false
        }
        [1, 6, 3] => {
            pick(b, &ON_OFF, s.scan_tx as u8);
            s.scan_tx
        }
        [1, 5] => {
            resume_text(b, s.scan_resume);
            s.scan_resume != Settings::DEFAULT.scan_resume
        }
        [1, 6, 5] => {
            if s.scan_prio == 0 {
                b.set(b"Off");
            } else {
                let _ = uwrite!(b, "Every {} s", s.scan_prio);
            }
            s.scan_prio != Settings::DEFAULT.scan_prio
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
        [9, 7, 1] => {
            b.set(cal_text(dev.selftest.cal).as_bytes());
            false
        }
        [9, 7, 3] => {
            let t = dev.selftest.battery_trim as i32;
            let _ = uwrite!(b, "{}{}.{} V", if t < 0 { "-" } else { "+" }, t.abs() / 100, ferrite_ui::text::Zero(t.unsigned_abs() % 100, 2));
            t != 0
        }
        _ => return None,
    };
    Some(d)
}

/// "Auto", "12 dB".
pub(crate) fn agc_text(v: u8, b: &mut Buf<24>) {
    if v == 0 {
        b.set(b"Auto");
    } else {
        b.clear();
        let _ = uwrite!(b, "{} dB", (v - 1) * 4);
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

/// The settings [`change`] steps, in menu order, less those that change only on the radio (PIN, region,
/// Extend TX): what the terminal's `set` takes and `dump` writes (TERM-12, TERM-13).
pub(crate) const SETTABLE: [&[u8]; 65] = [
    &[1, 1],
    &[1, 5],
    &[1, 6, 3],
    &[1, 6, 5],
    &[3, 1, 2],
    &[3, 1, 3],
    &[3, 1, 4],
    &[3, 1, 5],
    &[3, 2, 1],
    &[3, 2, 2],
    &[3, 2, 3],
    &[3, 2, 4],
    &[3, 2, 5],
    &[4, 2, 1],
    &[4, 2, 2],
    &[4, 2, 3],
    &[4, 8],
    &[4, 9, 1],
    &[5, 3],
    &[5, 5, 1],
    &[5, 5, 3],
    &[5, 5, 4],
    &[5, 6, 1],
    &[5, 6, 2],
    &[5, 8, 1],
    &[5, 8, 2],
    &[5, 8, 3],
    &[5, 9, 1],
    &[6, 1],
    &[6, 4],
    &[6, 5],
    &[6, 6],
    &[6, 8],
    &[7, 1],
    &[7, 2],
    &[7, 3, 2],
    &[7, 3, 3],
    &[7, 4, 1],
    &[7, 4, 2],
    &[7, 4, 3],
    &[7, 4, 4],
    &[7, 4, 5],
    &[7, 5, 1],
    &[7, 5, 2],
    &[7, 5, 3],
    &[7, 5, 4],
    &[7, 6],
    &[7, 7],
    &[7, 8],
    &[8, 2, 1],
    &[8, 2, 2],
    &[8, 2, 3],
    &[8, 4, 2],
    &[8, 5, 1],
    &[8, 5, 2],
    &[8, 5, 4],
    &[8, 6, 3],
    &[8, 6, 6],
    &[8, 6, 7],
    &[8, 7, 2],
    &[9, 1, 1],
    &[9, 2, 1],
    &[9, 2, 4],
    &[9, 2, 5],
    &[9, 2, 6],
];

/// The knob or ◀▶ on a wired setting (MENU-16); false if it isn't changeable here.
pub(crate) fn change(app: &mut App, p: &[u8], d: i32) -> bool {
    let s = &mut app.set;
    match p {
        [1, 1] => app.change(Item::Watch, d),
        [1, 6, 3] => {
            s.scan_tx = !s.scan_tx;
            app.out.push(RadioCmd::ScanTx(app.set.scan_tx));
        }
        // 1-10 s, carrier drop, stop on hit (SCAN-5); the next scan takes it
        [1, 5] => s.scan_resume = 1 + cycle(s.scan_resume.clamp(1, 12) - 1, d, 12),
        [1, 6, 5] => s.scan_prio = cycle(s.scan_prio, d, 6),
        [8, 4, 2] => {
            s.tone_level = (s.tone_level as i32 + d).clamp(0, 2) as u8;
            app.out.push(RadioCmd::ToneLevel(app.set.tone_level));
        }
        [7, 7] => app.change(Item::BatterySave, d),
        [6, 1] => s.gps_on = !s.gps_on,
        [6, 8] => s.gps_rxtx = cycle(s.gps_rxtx, d, GPS_RXTX.len()),
        [6, 3] => s.coord_fmt = cycle(s.coord_fmt, d, COORD_FORMATS.len()),
        [6, 4] => s.units = cycle(s.units, d, UNITS.len()),
        [6, 5] => s.tz = cycle(s.tz, d, TIME_ZONES.len()),
        [6, 6] => {
            // 24 h + GPS, 24 h, 12 h + GPS, 12 h
            let v = cycle(s.clock * 2 + !s.clock_gps as u8, d, 4);
            (s.clock, s.clock_gps) = (v / 2, v.is_multiple_of(2));
        }
        [7, 1] => {
            if s.theme != crate::settings::NIGHT_RED {
                app.day_theme = s.theme; // HOME-17: the quick menu's PTT B comes back to it from night red
            }
            s.theme = cycle(s.theme, d, THEMES.len());
        }
        [7, 2] => s.layout = cycle(s.layout, d, LAYOUTS.len()),
        [7, 3, 2] => s.pon = cycle(s.pon, d, POWER_ON.len()),
        [7, 3, 3] => s.boot_card = !s.boot_card,
        [9, 4] => {
            s.region = cycle(s.region.max(1) - 1, d, REGION_LIST.len()) + 1;
            app.region_steps(); // VFO-18
        }
        [3, 1, 2] => s.fm_step = cycle(s.fm_step, d, FM_STEPS.len()),
        [3, 1, 3] => s.fm_agc = cycle(s.fm_agc, d, 11),
        [3, 1, 4] => s.fm_monitor = !s.fm_monitor,
        [3, 2, 1] => s.scope_bars = cycle(s.scope_bars, d, SCOPE_BAR_COUNTS.len()),
        [3, 2, 2] => s.scope_step = cycle(s.scope_step, d, crate::slot::STEPS.len()),
        // 0.1 ms steps up to 1 ms, then 1 ms steps to 10 ms (stock's dwell, the default).
        [3, 2, 3] => {
            let v = s.scope_dwell.clamp(1, 100) as i32;
            let step = if v + d > 10 && v >= 10 { 10 } else { 1 };
            s.scope_dwell = (v + d * step).clamp(1, 100) as u8;
        }
        [3, 2, 4] => s.scope_trig = cycle(s.scope_trig, d, SCOPE_TRIGGERS.len()),
        [3, 2, 5] => s.scope_wf = !s.scope_wf,
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
        [4, 8] => s.popups = !s.popups,
        [5, 8, 1] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi ^ crate::aprs::DIGI_ON, ..app.aprs_cfg() }),
        [5, 8, 2] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi ^ crate::aprs::DIGI_MYCALL, ..app.aprs_cfg() }),
        [5, 8, 3] => {
            let c = app.aprs_cfg();
            let w = cycle(c.digi_wait_s(), d, crate::aprs::DIGI_WAIT_MAX as usize + 1);
            app.set_aprs(crate::aprs::AprsCfg { digi: c.digi & 0x0F | w << 4, ..c });
        }
        [5, 7, 1] => app.step_aprs_freq(d),
        [4, 9, 1] => s.same = !s.same,
        [4, 2, 1] => s.pocsag_on = !s.pocsag_on,
        // 6.25 kHz steps across the VHF and UHF pager bands.
        [4, 2, 2] => s.pocsag_freq = (s.pocsag_freq as i64 + d as i64 * 625).clamp(13_600_000, 52_000_000) as ferrite_core::freq::Freq10,
        [9, 2, 1] => s.bt = !s.bt,
        [9, 2, 4] => s.cable_port = cycle(s.cable_port, d, CABLE_PORTS.len()),
        [5, 9, 1] => s.kiss_port = cycle(s.kiss_port, d, crate::settings::KISS_PORTS.len()),
        [9, 2, 5] => s.term_colour = !s.term_colour,
        [9, 2, 6] => s.cable_log = !s.cable_log,
        [7, 4, 1] => s.backlight = cycle(s.backlight, d, BACKLIGHT_TIMES.len()),
        [7, 4, 2] => *s.brightness_mut() = (s.brightness() as i32 + d).clamp(1, 10) as u8, // DISP-16, DISP-23
        [7, 4, 3] => s.tx_light = !s.tx_light,
        [7, 4, 4] => s.wake_on = cycle(s.wake_on, d, WAKE_ON.len()),
        [7, 4, 5] => s.keypad_light = cycle(s.keypad_light, d, KEYPAD_LIGHTS.len()),
        [7, 5, 4] => s.arrival_tone = !s.arrival_tone,
        [7, 5, 1] => s.key_beep = !s.key_beep,
        [7, 5, 3] => s.alert_tones = !s.alert_tones,
        [7, 5, 2] => {
            s.beep_vol = (s.beep_vol.clamp(1, 3) as i32 + d).clamp(1, 3) as u8;
            app.out.push(RadioCmd::BeepVol(app.set.beep_vol));
        }
        [3, 1, 5] => s.fm_sleep = cycle(s.fm_sleep, d, SLEEP_TIMES.len()),
        [8, 2] | [8, 2, 1] => {
            s.vox = !s.vox;
            app.send_vox();
        }
        [8, 2, 2] => {
            s.vox_level = (s.vox_level.clamp(1, 9) as i32 + d).clamp(1, 9) as u8;
            app.send_vox();
        }
        [8, 2, 3] => {
            s.vox_tail = cycle(s.vox_tail, d, VOX_TAILS.len());
            app.send_vox();
        }
        [7, 6] => s.leds = cycle(s.leds, d, LED_MODES.len()),
        [7, 8] => s.auto_off = cycle(s.auto_off, d, AUTO_OFF_TIMES.len()),
        [9, 1, 1] => s.auto_lock = cycle(s.auto_lock, d, AUTO_LOCK_TIMES.len()), // KEY-20
        [8, 6, 6] => {
            s.dtmf_sidetone = !s.dtmf_sidetone;
            app.send_tx_tones();
        }
        [8, 5, 1] => {
            s.tail_mode = cycle(s.tail_mode, d, TAIL_MODES.len());
            app.send_slot_tones();
        }
        [8, 5, 2] => {
            s.tail_phase = cycle(s.tail_phase, d, TAIL_PHASES.len());
            app.send_tx_tones();
        }
        [8, 5, 4] => {
            s.end_tone = cycle(s.end_tone, d, END_TONE_NAMES.len());
            app.send_slot_tones();
        }
        // 50-200 ms in 10 ms steps (DTMF-3)
        [8, 6, 3] => {
            s.dtmf_ms = (s.dtmf_ms.clamp(50, 200) as i32 + d * 10).clamp(50, 200) as u8;
            app.send_tx_tones();
        }
        // 0-1000 ms in 50 ms steps (DTMF-3)
        [8, 6, 7] => {
            s.dtmf_delay = (s.dtmf_delay.min(100) as i32 + d * 5).clamp(0, 100) as u8;
            app.send_tx_tones();
        }
        // VOX-10: CW ID needs a callsign
        [8, 7, 2] if s.callsign.as_bytes().is_empty() => app.show_toast(b"CW ID", b"set a callsign first", false),
        [8, 7, 2] => {
            s.cw_id = cycle(s.cw_id, d, CW_ID_MODES.len());
            app.send_tx_tones();
        }
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
        _ => return false,
    }
    true
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

/// # on a wired setting: its default (MENU-18).
pub(crate) fn reset(app: &mut App, p: &[u8]) {
    let d = Settings::DEFAULT;
    let s = &mut app.set;
    match p {
        [1, 1] => app.change(Item::Watch, 1 - app.watch as i32),
        [7, 7] => app.change(Item::BatterySave, 1 - app.bsave as i32),
        [1, 6, 3] => {
            s.scan_tx = d.scan_tx;
            app.out.push(RadioCmd::ScanTx(d.scan_tx));
        }
        [1, 5] => s.scan_resume = d.scan_resume,
        [1, 6, 5] => s.scan_prio = d.scan_prio,
        [8, 4, 2] => {
            s.tone_level = d.tone_level;
            app.out.push(RadioCmd::ToneLevel(d.tone_level));
        }
        [6, 1] => s.gps_on = d.gps_on,
        [6, 8] => s.gps_rxtx = d.gps_rxtx,
        [6, 3] => s.coord_fmt = d.coord_fmt,
        [6, 9] => s.fixed_on = false,
        [6, 4] => s.units = d.units,
        [6, 5] => s.tz = d.tz,
        [6, 6] => (s.clock, s.clock_gps) = (d.clock, d.clock_gps),
        [7, 1] => s.theme = d.theme,
        [7, 2] => s.layout = d.layout,
        [7, 3, 2] => s.pon = d.pon,
        [7, 3, 3] => s.boot_card = d.boot_card,
        [8, 9] => s.extend_tx = false,
        [3, 1, 2] => s.fm_step = d.fm_step,
        [3, 1, 3] => s.fm_agc = d.fm_agc,
        [3, 1, 4] => s.fm_monitor = d.fm_monitor,
        [3, 2, 1] => s.scope_bars = d.scope_bars,
        [3, 2, 2] => s.scope_step = d.scope_step,
        [3, 2, 3] => s.scope_dwell = d.scope_dwell,
        [3, 2, 4] => s.scope_trig = d.scope_trig,
        [3, 2, 5] => s.scope_wf = d.scope_wf,
        [5, 3] if s.aprs_mode != d.aprs_mode => app.set_aprs_mode(d.aprs_mode),
        [5, 5, 1] => app.set_aprs(crate::aprs::AprsCfg { beacon: 0, ..app.aprs_cfg() }),
        [5, 5, 4] => app.set_aprs(crate::aprs::AprsCfg { power: 0, ..app.aprs_cfg() }),
        [5, 5, 3] => app.set_aprs(crate::aprs::AprsCfg { after_voice: false, ..app.aprs_cfg() }),
        [5, 6, 1] => app.set_aprs(crate::aprs::AprsCfg { path: crate::aprs::PATH_DEFAULT, ..app.aprs_cfg() }),
        [5, 8, 1] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi & !crate::aprs::DIGI_ON, ..app.aprs_cfg() }),
        [5, 8, 2] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi & !crate::aprs::DIGI_MYCALL, ..app.aprs_cfg() }),
        [5, 8, 3] => app.set_aprs(crate::aprs::AprsCfg { digi: app.aprs_cfg().digi & 0x0F, ..app.aprs_cfg() }),
        [5, 7, 1] => app.set_aprs(crate::aprs::AprsCfg { f5k: 0, ..app.aprs_cfg() }),
        [4, 9, 1] => s.same = d.same,
        [4, 2, 1] => s.pocsag_on = d.pocsag_on,
        [4, 2, 2] => s.pocsag_freq = d.pocsag_freq,
        [4, 2, 3] => s.pocsag_cap = d.pocsag_cap,
        [4, 8] => s.popups = d.popups,
        // # forgets the custom path; a path set to it goes back to the default (APRS-26)
        [5, 6, 2] => {
            let c = app.aprs_cfg();
            let path = if c.path == crate::aprs::PATH_CUSTOM { crate::aprs::PATH_DEFAULT } else { c.path };
            app.set_aprs(crate::aprs::AprsCfg { custom: [0; crate::aprs::CUSTOM_LEN], path, ..c });
        }
        [9, 2, 1] => s.bt = d.bt,
        [9, 2, 4] => s.cable_port = d.cable_port,
        [5, 9, 1] => s.kiss_port = d.kiss_port,
        [9, 2, 5] => s.term_colour = d.term_colour,
        [9, 2, 6] => s.cable_log = d.cable_log,
        [7, 4, 1] => s.backlight = d.backlight,
        [7, 4, 2] => {
            let night = s.theme == NIGHT_RED;
            *s.brightness_mut() = if night { d.brightness_night } else { d.brightness };
        }
        [7, 4, 3] => s.tx_light = d.tx_light,
        [7, 4, 4] => s.wake_on = d.wake_on,
        [7, 4, 5] => s.keypad_light = d.keypad_light,
        [7, 5, 4] => s.arrival_tone = d.arrival_tone,
        [7, 5, 1] => s.key_beep = d.key_beep,
        [7, 5, 3] => s.alert_tones = d.alert_tones,
        [7, 5, 2] => {
            s.beep_vol = d.beep_vol;
            app.out.push(RadioCmd::BeepVol(app.set.beep_vol));
        }
        [3, 1, 5] => s.fm_sleep = d.fm_sleep,
        [8, 2] | [8, 2, 1] | [8, 2, 2] | [8, 2, 3] => {
            match p {
                [8, 2, 2] => s.vox_level = d.vox_level,
                [8, 2, 3] => s.vox_tail = d.vox_tail,
                _ => s.vox = d.vox,
            }
            app.send_vox();
        }
        [7, 6] => s.leds = d.leds,
        [7, 8] => s.auto_off = d.auto_off,
        [9, 1, 1] => s.auto_lock = d.auto_lock,
        [8, 6, 6] | [8, 5, 2] | [8, 6, 3] | [8, 6, 7] | [8, 7, 2] => {
            match p {
                [8, 6, 6] => s.dtmf_sidetone = d.dtmf_sidetone,
                [8, 5, 2] => s.tail_phase = d.tail_phase,
                [8, 6, 3] => s.dtmf_ms = d.dtmf_ms,
                [8, 6, 7] => s.dtmf_delay = d.dtmf_delay,
                _ => s.cw_id = d.cw_id,
            }
            app.send_tx_tones();
        }
        [8, 5, 1] | [8, 5, 4] => {
            if p == [8, 5, 1] {
                s.tail_mode = d.tail_mode;
            } else {
                s.end_tone = d.end_tone;
            }
            app.send_slot_tones();
        }
        _ => {}
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
            let n = items(self.g, 0).get(self.sub as usize - 1).map_or("", |e| e.0).as_bytes();
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
            app.show_toast(self.items()[self.row as usize].0.as_bytes(), why.as_bytes(), false);
        } else {
            let p = &p[..pl];
            // MENU-22: a radio-wide setting changed here offers "* Undo" (the slot's own items keep theirs).
            let was = shown(app, p);
            if change(app, p, d) && !matches!(p, [1, 1] | [7, 7]) && shown(app, p) != was {
                app.keep_set_undo(p, d, was);
            }
        }
    }

    fn draw(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let list = self.items();
        app.new_page(lcd, hash(&(self.g, self.sub)));
        let mut count = Buf::<24>::new();
        for (i, (name, val, _)) in list.iter().enumerate() {
            let (p, pl) = self.path(i as u8);
            let mut v = Buf::<24>::new();
            let dot = match wired(app, cx.dev, &p[..pl], &mut v) {
                Some(d) => d,
                None => {
                    match &p[..pl] {
                        [2, 1] => {
                            count.clear();
                            let _ = uwrite!(count, "{} channels", cx.store.count());
                            v.set(count.as_bytes());
                        }
                        [4, 1] if app.inbox > 0 => {
                            let _ = uwrite!(v, "{} new", app.inbox);
                        }
                        [5, 4] => v.set(b"no callsign"),
                        _ => v.set(val.as_bytes()),
                    }
                    false
                }
            };
            let grey = greyed(app, &p[..pl]);
            if let (Some(why), true) = (grey, i as u8 != self.row) {
                v.set(why.as_bytes()); // MENU-14: greyed items say why
            }
            let r = Row { num: Some(i as u8 + 1), name: name.as_bytes(), value: v.as_bytes(), dot, selected: i as u8 == self.row, grey: grey.is_some() };
            app.list_row(lcd, i, 23 + 22 * i as i32, 22, &r);
        }
        let (p, pl) = self.path(self.row);
        let mut path = Buf::<20>::new();
        path.push(b"MENU 0");
        for x in &p[..pl] {
            let _ = uwrite!(path, " {}", x);
        }
        let e = list.get(self.row as usize).copied().unwrap_or(("", "", ""));
        let help = greyed(app, &p[..pl]).unwrap_or(e.2);
        app.help_box(lcd, 226, 46, help.as_bytes(), path.as_bytes());
    }

    /// OK (or its digit) on the highlighted row: its screen, its list, or (OK only) a toggle.
    fn open(self, app: &mut App, ok: bool) {
        let (p, pl) = self.path(self.row);
        let p = &p[..pl];
        if let Some(why) = greyed(app, p) {
            return app.show_toast(self.items()[self.row as usize].0.as_bytes(), why.as_bytes(), false);
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
    }
}
