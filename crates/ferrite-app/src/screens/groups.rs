//! A settings group (MENU 0 n) and a third-level list (MENU 0 n m): one list widget for both (manual chapter
//! 11, MENU-3 to MENU-24; MenuGroup mock-up). The lists are data: [`GROUP_ITEMS`] and [`SUBS`] hold every item
//! of `docs/settings.md` with its value and help line; [`wired`] gives the live value of the settings that exist
//! so far, and [`change`] changes them. The rest show their default, read only, until their feature lands.
//!
//! Keys: ▲▼ move, the knob or ◀▶ change the highlighted value (MENU-16), OK opens a screen or list, a digit
//! picks its row and opens it (MENU-5: a path typed straight through lands where it would typed slowly), #
//! resets (MENU-18), BACK goes up keeping the place (MENU-24).

use super::nav::{self, Dest};
use super::setup::{ExtendTx, Setup};
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::*;
use crate::slot::Item;
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::{CalState, Device};
use ferrite_core::VERSION;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::Row;
use ufmt::uwrite;

/// One item: name, value shown until it is wired (a list shows a summary), help line.
pub(crate) type Entry = (&'static str, &'static str, &'static str);

const G1: [Entry; 9] = [
    ("Watch", "A+B", "Which slots are heard besides the active one."),
    ("Watch behaviour", "Heard", "Audio, follow, TX slot, pause time."),
    ("Background job", "Off", "A zone scan or decoder on a watched slot."),
    ("Scan target", "Current zone", "What SIDE2 scans."),
    ("Resume", "5 s", "When a stopped scan moves on."),
    ("Scan options", "Up, 1 s", "Direction, dwell, smart scan, priority."),
    ("Range presets", "4 presets", "Named ranges to scan."),
    ("Skip list", "Empty", "Frequencies * skipped in this scan."),
    ("Tone search", "", "Find the CTCSS or DCS in use."),
];
const G2: [Entry; 5] = [
    ("Browse", "", "Zones, all channels, nearby, recent."),
    ("Add repeater", "", "Frequency, shift, tone, name."),
    ("Zones", "", "Name, scan options, priority channels."),
    ("Nearby", "Off", "Sort channels by distance."),
    ("Channel display", "Name", "Name, number or frequency on home."),
];
const G3: [Entry; 5] = [
    ("FM broadcast", "Monitor on", "Presets, step, AGC, monitor, sleep."),
    ("Band scope", "+/-1 MHz", "Span, step, speed, trigger, waterfall."),
    ("Weather alerts", "On", "SAME, location codes, tone, channel."),
    ("Frequency counter", "", "Find a nearby transmitter's frequency."),
    ("HF mode", "", "Below 64 MHz. Everything else pauses."),
];
const G4: [Entry; 8] = [
    ("Event log", "", "Every decode and alert: the inbox."),
    ("POCSAG", "439.9875", "Pager frequency, baud, capcodes."),
    ("DSC", "Off", "Marine distress calls, receive only."),
    ("AIS", "Off", "Ships near you."),
    ("Sensor reader", "", "433 MHz sensors and the jamming alarm."),
    ("Selcall, MDC1200", "Off", "Decode selcall and MDC1200 IDs."),
    ("DTMF decode", "Off", "Show and log DTMF sequences."),
    ("Popups", "On", "A 3 s toast for each decode."),
];
const G5: [Entry; 9] = [
    ("Heard", "", "Stations heard, nearest first."),
    ("Messages", "", "Messages sent to you and by you."),
    ("Beacon now", "", "Send one position beacon."),
    ("My station", "SSID 9", "SSID, symbol, comment, position."),
    ("Beacon", "Off", "When to beacon: manual, timed, smart."),
    ("Path", "WIDE1-1,WIDE2-1", "The digipeater path."),
    ("Radio", "144.390", "APRS frequency, delays, levels."),
    ("Digipeat", "Off", "Relay others' packets."),
    ("KISS TNC", "Off", "Raw KISS on the cable or Bluetooth."),
];
const G6: [Entry; 9] = [
    ("GPS", "On", "The GPS receiver."),
    ("Position", "", "Fix, satellites, position, grid."),
    ("Coordinate format", "Deg, min", "How positions are written."),
    ("Units", "Metric", "Distance, altitude and speed units."),
    ("Time zone", "UTC-8", "The clock's offset from UTC."),
    ("Clock", "24 h", "12 or 24 h, and set from GPS."),
    ("Waypoints", "", "Marked places; go to one."),
    ("GPS during RX/TX", "Always on", "Turn the GPS off around signals."),
    ("Fixed position", "None", "Used when there is no fix."),
];
const G7: [Entry; 9] = [
    ("Theme", "Dark", "Dark, daylight or night red."),
    ("Home layout", "Watch", "What home shows."),
    ("Display", "Icon + %", "Battery, power-on message, boot, S-meter."),
    ("Backlight", "15 s", "Timeout, brightness, wake."),
    ("Sounds", "Beep on", "Key beep, volume, roger beep, alerts."),
    ("LEDs", "RX and TX", "When the LEDs light."),
    ("Battery Save", "Level 1", "Receiver sleep: longer life, slower."),
    ("Auto power off", "Off", "Off after a time without key or signal."),
    ("Menu timeout", "30 s", "An idle menu returns home."),
];
const G8: [Entry; 9] = [
    ("Timeout", "120 s", "Transmit timeout and its pre-alert."),
    ("VOX", "Off", "Voice-operated transmit."),
    ("Mic gain", "5", "Microphone level, 1-9."),
    ("Tone burst", "1750 Hz", "PTT + SIDE2 sends it."),
    ("Tail elimination", "Off", "Squelch tail: mode, phase, delay."),
    ("DTMF", "100 ms", "Dialer, memories, timing, PTT ID."),
    ("Callsign and ID", "Empty", "Your callsign, and CW ID."),
    ("Busy lockout default", "Off", "For new channels and the VFO."),
    ("Extend TX", "Off", "Transmit beyond the amateur bands."),
];
const G9: [Entry; 9] = [
    ("Keys", "No auto lock", "Auto lock, lock covers, key test."),
    ("Connections", "BT off", "Bluetooth, paired devices, link."),
    ("PIN and lock", "Off", "PIN, channel-only mode."),
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
const SHORT: [&str; 9] = ["Scan", "Channels", "Listen", "Decoders", "APRS", "GPS", "Display", "Transmit", "System"];

/// The third-level lists, by group and item.
pub(crate) const SUBS: [((u8, u8), &[Entry]); 27] = [
    ((1, 2), &[
        ("Watched audio", "Heard", "Heard, muted, per slot."),
        ("Follow", "Off", "The active slot follows a watched one."),
        ("Follow return", "Switch back", "Stay, or switch back after."),
        ("TX on", "Active slot", "PTT keys the active or last heard."),
        ("Pause time", "1 min", "# on home pauses watch this long."),
    ]),
    ((1, 6), &[
        ("Direction", "Up", "Scan up or down."),
        ("Dwell", "1 s", "Wait before resuming."),
        ("Smart scan", "Off", "Skip what's always busy."),
        ("Return to", "Start", "Where a stopped scan returns."),
        ("Priority check", "Every 2 s", "Look at the priority channels."),
    ]),
    ((3, 1), &[
        ("Presets", "Empty", "20 stations."),
        ("Step", "100 kHz", "Tuning step."),
        ("AGC", "Auto", "Gain control."),
        ("Radio monitor", "On", "A slot opening interrupts the broadcast."),
        ("Sleep timer", "Off", "End the broadcast after a time."),
    ]),
    ((3, 2), &[
        ("Span", "+/-1 MHz", "How wide the scope looks."),
        ("Step", "25 kHz", "Distance between bars."),
        ("Sweep speed", "Normal", "Faster sweeps miss less."),
        ("Trigger level", "Off", "Mark signals above this."),
        ("Waterfall row", "Off", "History under the scope."),
    ]),
    ((3, 3), &[
        ("SAME", "On", "Decode weather alerts."),
        ("Location codes", "None", "Up to 8 FIPS or CLC codes."),
        ("Alert tone", "On", "Sound the alert tone."),
        ("Channel", "Auto", "The weather channel to watch."),
    ]),
    ((4, 2), &[
        ("Frequency", "439.9875", "The pager channel."),
        ("Baud", "1200", "512, 1200 or 2400."),
        ("Capcode filter", "None", "Only these capcodes."),
    ]),
    ((4, 3), &[("DSC", "Off", "Decode DSC on channel 70."), ("Own MMSI", "None", "Your MMSI, receive only.")]),
    ((4, 4), &[("AIS", "Off", "Decode AIS."), ("Range filter", "Off", "Only ships this close.")]),
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
    ]),
    ((5, 6), &[("Preset", "WIDE1-1,WIDE2-1", "The usual paths."), ("Custom path", "Empty", "Your own path.")]),
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
        ("Aliases", "WIDE1-1", "The aliases relayed."),
        ("Wait", "0 s", "Delay before relaying."),
    ]),
    ((7, 3), &[
        ("Battery as", "Icon + %", "Icon, percent or volts."),
        ("Power-on message", "Callsign", "Off, callsign or text."),
        ("Boot screen", "Logo", "Logo, or the owner card."),
        ("S-meter", "Bars", "Bars or dBm."),
    ]),
    ((7, 4), &[
        ("Timeout", "15 s", "Backlight off after this."),
        ("Brightness", "7", "1-10."),
        ("On transmit", "On", "Light up while transmitting."),
        ("Wake screen on", "Keys, receive", "What wakes the screen."),
    ]),
    ((7, 5), &[
        ("Key beep", "On", "Beep on key presses."),
        ("Beep volume", "3", "1-5."),
        ("Roger beep", "Off", "Beep or MDC1200 at unkey."),
        ("Alert tones", "On", "Tones for alerts."),
    ]),
    ((8, 1), &[("TX timeout", "120 s", "Transmit stops after this."), ("Pre-alert", "10 s", "Warning before the timeout.")]),
    ((8, 2), &[("VOX", "Off", "Transmit when you talk."), ("Level", "5", "1-9."), ("Delay", "1 s", "Hang time after speech.")]),
    ((8, 5), &[
        ("Mode", "Off", "Receive, transmit or both."),
        ("Phase", "55 Hz", "55 Hz or a phase shift."),
        ("Repeater tail delay", "0 s", "Wait for the repeater tail."),
    ]),
    ((8, 6), &[
        ("Dialer", "", "Type and send DTMF."),
        ("Memories", "", "16 stored sequences."),
        ("On and off time", "100 ms", "DTMF tone timing."),
        ("PTT ID on VFOs", "Off", "Send the ID code at PTT."),
        ("ID code", "Empty", "The radio's DTMF ID."),
    ]),
    ((8, 7), &[("Callsign", "Empty", "For APRS, CW ID, power-on message."), ("CW ID", "Off", "Send your callsign in CW.")]),
    ((9, 1), &[
        ("Auto lock", "Off", "Lock the keys after a time."),
        ("Lock covers", "Keys", "Keys, or keys and PTT."),
        ("Key test", "", "Each key's code, name and press."),
    ]),
    ((9, 2), &[
        ("Bluetooth", "Off", "The Bluetooth link to the web app."),
        ("Paired devices", "", "Approved devices; forget one."),
        ("Link status", "", "Cable and Bluetooth now."),
        ("Debug console", "Off", "Off, cable or Bluetooth."),
        ("Cable port", "Auto", "Link, terminal or KISS."),
    ]),
    ((9, 3), &[("PIN", "Off", "Ask for a PIN at power on."), ("Channel-only mode", "Off", "Hide VFO and editing.")]),
    ((9, 5), &[
        ("Advanced squelch", "", "Thresholds per band."),
        ("Squelch tail", "150 ms", "Audio after the signal drops."),
        ("Discriminator mode", "Off", "Flat audio for decoders."),
    ]),
    ((9, 7), &[
        ("Calibration", "", "The factory calibration, read only."),
        ("Crystal trim", "0 ppm", "Centre a known signal."),
        ("Battery reading trim", "+0.00 V", "Match the reading to a meter."),
        ("Speaker level trim", "0", "Not defined yet."),
    ]),
    ((9, 8), &[
        ("Run setup again", "", "The first-boot questions."),
        ("Reset settings", "", "Settings, or channels too."),
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
fn greyed(app: &App, p: &[u8]) -> Option<&'static str> {
    match p {
        [3, 3, ..] if !app.set.wx_region() => Some("US and Canada only"), // WX-3
        [5, 3] if app.set.callsign.is_empty() => Some("needs callsign"),  // SETUP-9
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
        [6, 4] => {
            pick(b, &UNITS, s.units);
            s.units != 0
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
        [3, 3, 1] => {
            pick(b, &ON_OFF, s.same as u8);
            !s.same
        }
        [9, 2, 1] => {
            pick(b, &ON_OFF, s.bt as u8);
            s.bt
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
        [9, 3] | [9, 3, 1] => {
            pick(b, &PIN_MODES, s.pin_mode);
            s.pin_mode != 0
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

fn cycle(v: u8, d: i32, n: usize) -> u8 {
    (v as i32 + d).rem_euclid(n as i32) as u8
}

/// The knob or ◀▶ on a wired setting (MENU-16); false if it isn't changeable here.
fn change(app: &mut App, p: &[u8], d: i32) -> bool {
    let s = &mut app.set;
    match p {
        [1, 1] => app.change(Item::Watch, d),
        [7, 7] => app.change(Item::BatterySave, d),
        [6, 4] => s.units = cycle(s.units, d, UNITS.len()),
        [6, 5] => s.tz = cycle(s.tz, d, TIME_ZONES.len()),
        [6, 6] => {
            // 24 h + GPS, 24 h, 12 h + GPS, 12 h
            let v = cycle(s.clock * 2 + !s.clock_gps as u8, d, 4);
            (s.clock, s.clock_gps) = (v / 2, v.is_multiple_of(2));
        }
        [7, 1] => s.theme = cycle(s.theme, d, THEMES.len()),
        [7, 2] => s.layout = cycle(s.layout, d, LAYOUTS.len()),
        [9, 4] => s.region = cycle(s.region.max(1) - 1, d, REGION_LIST.len()) + 1,
        [3, 1, 2] => s.fm_step = cycle(s.fm_step, d, FM_STEPS.len()),
        [3, 1, 3] => s.fm_agc = cycle(s.fm_agc, d, 11),
        [3, 1, 4] => s.fm_monitor = !s.fm_monitor,
        [3, 3, 1] => s.same = !s.same,
        [9, 2, 1] => s.bt = !s.bt,
        [8, 9] => set_extend(app, !app.set.extend_tx),
        [9, 3] | [9, 3, 1] => {
            let m = cycle(s.pin_mode, d, PIN_MODES.len());
            if m != 0 && s.pin_len == 0 {
                app.open(Dest::Pin); // a PIN first (PIN-1): the PIN screen sets it, then the mode
                app.screen = Screen::Pin(super::pin::Pin::set(m));
            } else {
                s.pin_mode = m;
                if m == 0 {
                    s.pin_len = 0;
                }
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
fn reset(app: &mut App, p: &[u8]) {
    let d = Settings::DEFAULT;
    let s = &mut app.set;
    match p {
        [1, 1] => app.change(Item::Watch, 1 - app.watch as i32),
        [7, 7] => app.change(Item::BatterySave, 1 - app.bsave as i32),
        [6, 4] => s.units = d.units,
        [6, 5] => s.tz = d.tz,
        [6, 6] => (s.clock, s.clock_gps) = (d.clock, d.clock_gps),
        [7, 1] => s.theme = d.theme,
        [7, 2] => s.layout = d.layout,
        [8, 9] => s.extend_tx = false,
        [3, 1, 2] => s.fm_step = d.fm_step,
        [3, 1, 3] => s.fm_agc = d.fm_agc,
        [3, 1, 4] => s.fm_monitor = d.fm_monitor,
        [3, 3, 1] => s.same = d.same,
        [9, 2, 1] => s.bt = d.bt,
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
            (Key::Left | Key::Right, Short | Repeat) => self.turn(app, if e.key == Key::Right { 1 } else { -1 }),
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

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("Knob", "Change"), ("OK", "Open"), ("BACK", "Back")])
    }
}

impl Group {
    /// The knob or ◀▶: change the highlighted value, or say why it can't.
    fn turn(self, app: &mut App, d: i32) {
        let (p, pl) = self.path(self.row);
        if let Some(why) = greyed(app, &p[..pl]) {
            app.show_toast(self.items()[self.row as usize].0.as_bytes(), why.as_bytes(), false);
        } else {
            change(app, &p[..pl], d);
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
                // The callsign: setup's callsign page on its own (SETUP-5: the same setting).
                app.open(Dest::Setup);
                app.screen = Screen::Setup(Setup::single(1));
            }
            [8, 9] if ok => set_extend(app, !app.set.extend_tx),
            [5, 3] if ok => app.show_toast(b"Beacon", b"APRS comes in milestone 5", false),
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
