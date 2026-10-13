//! The backlight, keypad light and LEDs, the auto key lock and auto power off (manual chapters 7, 31 and 33:
//! KEY-20 and decision 63). The App decides; the firmware drives the pins (screen light PC6, keypad light PB3,
//! LEDs PC13/PC14) from [`App::dark`], [`App::keypad_lit`] and [`App::leds`], and switches off when
//! [`App::take_power_off`] says so.
//!
//! - **Dark:** no key, PTT or receiving for the backlight time (0 7 4 1). The key press that wakes a dark screen
//!   only wakes it; PTT acts at once (decision 63). Receive and transmit wake it and hold it lit only as 0 7 4 4
//!   says (DISP-15); with 0 7 4 3 on, a lit screen stays lit through a transmission, off it is dark while keyed
//!   (DISP-26).
//! - **Brightness:** 0 7 4 2's level for the theme (night red its own), as a PWM duty: [`App::backlight_duty`]
//!   (DISP-16, DISP-23).
//! - **LEDs:** red while transmitting, green while a slot receives; while the screen is dark, a heartbeat in the
//!   battery's colour (green above 80 %, yellow = both between the low warning and 80 %, red at or below it):
//!   one short flash every 5 s, or a double flash every 2 s while a message or alert is unread.

use crate::settings::{AUTO_LOCK_TIMES, AUTO_OFF_TIMES, BACKLIGHT_TIMES};
use crate::App;

/// 0 7 4 2's levels 1-10 as PWM duty, thousandths, for the screen and the keypad light alike: a geometric curve
/// (each step about 1.41 times the last), so the steps look even to the eye. Level 1 is 4.5 %, the dimmest found
/// readable on the radio (Oct 12, 2026: the old 2 % was too dark); level 10 is full on.
pub const BRIGHTNESS: [u16; 10] = [45, 65, 90, 125, 180, 250, 355, 500, 710, 1000];

/// `a` is at or after `b` (wrapping ms).
fn after(a: u32, b: u32) -> bool {
    a.wrapping_sub(b) < 1 << 31
}
use ferrite_core::msg::{Device, LiveAll};

const DARK: u8 = 1;
const WARNED: u8 = 2;
const WANT_OFF: u8 = 4;
const SWALLOW: u8 = 8;
/// Keyed with 0 7 4 3 off: dark while keyed (DISP-26), the timeout left as it was.
const TX_DARK: u8 = 16;

/// A flash's length, and the two heartbeat periods.
const FLASH_MS: u32 = 80;
const BEAT_MS: u32 = 5000;
const UNREAD_MS: u32 = 2000;
/// The second flash of the double one starts this long after the first.
const SECOND_MS: u32 = 250;
/// Auto power off warns this long before.
const OFF_WARN_MS: u32 = 10_000;
/// "On key press": the keypad stays lit this long after a key.
const KEY_LIGHT_MS: u32 = 5000;

/// The LEDs now, and when they next change (ms from now; 0 when they hold).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Leds {
    pub red: bool,
    pub green: bool,
    pub next_ms: u32,
}

impl App {
    /// The screen's backlight is off.
    pub fn dark(&self) -> bool {
        self.lit & DARK != 0
    }

    /// The backlight's PWM duty in thousandths (DISP-16, DISP-23): 0 while dark, else 0 7 4 2's level for the theme
    /// in use on a perceptual curve, 1000 at 10.
    pub fn backlight_duty(&self) -> u16 {
        if self.dark() || self.lit & TX_DARK != 0 {
            return 0;
        }
        BRIGHTNESS[self.set.brightness() as usize - 1]
    }

    /// The keypad light's PWM duty in thousandths (DISP-22): 0 while [`App::keypad_lit`] says off, else the same
    /// level as the screen's (the theme's 0 7 4 2), lit or not, so one level dims both.
    pub fn keypad_duty(&self) -> u16 {
        if self.keypad_lit() {
            BRIGHTNESS[self.set.brightness() as usize - 1]
        } else {
            0
        }
    }

    /// The keypad light (0 7 4 5, DISP-22): with the screen, or for [`KEY_LIGHT_MS`] after any key or knob step
    /// (the one that woke the screen too).
    pub fn keypad_lit(&self) -> bool {
        let lit = !self.dark() && self.lit & TX_DARK == 0;
        match self.set.keypad_light {
            0 => false,
            1 => lit,
            2 => lit && self.now.wrapping_sub(self.pressed) < KEY_LIGHT_MS,
            _ => true,
        }
    }

    /// Auto power off has run out: the firmware switches off as the power switch does (PWR-17).
    pub fn take_power_off(&mut self) -> bool {
        let w = self.lit & WANT_OFF != 0;
        self.lit &= !WANT_OFF;
        w
    }

    /// The LEDs (0 7 6).
    pub fn leds(&self, live: &LiveAll, dev: &Device) -> Leds {
        let mode = self.set.leds;
        if mode == 0 {
            return Leds::default();
        }
        if live.tx.keyed {
            return Leds { red: true, ..Leds::default() };
        }
        if live.slots.iter().any(|s| s.squelch_open) {
            return Leds { green: true, ..Leds::default() };
        }
        if mode < 2 || !self.dark() {
            return Leds::default();
        }
        // The heartbeat: where we are in its period, and whether a flash is on.
        let (period, flashes) = if self.inbox > 0 { (UNREAD_MS, 2) } else { (BEAT_MS, 1) };
        let t = self.now % period;
        let on = t < FLASH_MS || (flashes == 2 && (SECOND_MS..SECOND_MS + FLASH_MS).contains(&t));
        let next_ms = match t {
            _ if t < FLASH_MS => FLASH_MS - t,
            _ if flashes == 2 && t < SECOND_MS => SECOND_MS - t,
            _ if flashes == 2 && t < SECOND_MS + FLASH_MS => SECOND_MS + FLASH_MS - t,
            _ => period - t,
        };
        let (red, green) = if dev.battery_low || dev.battery_cutoff {
            (true, false)
        } else if dev.battery_pct > 80 {
            (false, true)
        } else {
            (true, true)
        };
        Leds { red: on && red, green: on && green, next_ms }
    }

    /// A key or the knob while dark: light up; true when this input only woke the screen.
    pub(crate) fn wake(&mut self, now: u32) -> bool {
        self.seen = now;
        self.pressed = now;
        self.lit &= !WARNED;
        if self.dark() {
            self.lit &= !DARK;
            return true;
        }
        false
    }

    /// The waking key's release is swallowed too (set when a key woke the screen).
    pub(crate) fn swallow(&mut self, on: bool) -> bool {
        let was = self.lit & SWALLOW != 0;
        if on {
            self.lit |= SWALLOW;
        } else {
            self.lit &= !SWALLOW;
        }
        was
    }

    /// A key woke the screen and its presses are being ignored until it is let go.
    pub(crate) fn swallowing(&self) -> bool {
        self.lit & SWALLOW != 0
    }

    /// The backlight time runs out now, as if no key had been pressed for it (the simulators' "Screen off"
    /// button, to see the dark-screen heartbeat). Nothing when the backlight is set to stay on.
    pub fn backlight_out(&mut self) {
        let bl = BACKLIGHT_TIMES[self.set.backlight as usize % BACKLIGHT_TIMES.len()] as u32 * 1000;
        if bl != 0 {
            self.seen = self.now.wrapping_sub(bl);
        }
    }

    /// Time passed: receiving keeps the radio awake (auto off); receive and transmit wake the screen as 0 7 4 4
    /// says (DISP-15), and 0 7 4 3 holds a lit screen through a transmission or darkens it (DISP-26); the
    /// backlight, auto lock and auto off.
    pub(crate) fn lights_tick(&mut self, now: u32, live: &LiveAll) {
        let rx = live.slots.iter().any(|s| s.squelch_open);
        let tx = live.tx.keyed;
        // Auto off counts any activity, whatever wakes the screen.
        if rx || tx {
            self.awake = now;
            self.lit &= !WARNED;
        }
        if rx && self.set.wake_on >= 1 || tx && (self.set.wake_on >= 2 || self.set.tx_light && !self.dark()) {
            self.seen = now;
        }
        if tx && !self.set.tx_light {
            self.lit |= TX_DARK;
        } else {
            self.lit &= !TX_DARK;
        }
        let idle = now.wrapping_sub(self.seen);
        let bl = BACKLIGHT_TIMES[self.set.backlight as usize % BACKLIGHT_TIMES.len()] as u32 * 1000;
        if bl != 0 && idle >= bl && self.lit & WARNED == 0 {
            self.lit |= DARK;
        } else if idle < bl || bl == 0 {
            self.lit &= !DARK;
        }
        // KEY-20: keys and the knob only (receiving doesn't keep the keys unlocked).
        let lock = AUTO_LOCK_TIMES[self.set.auto_lock as usize % AUTO_LOCK_TIMES.len()] as u32 * 1000;
        if lock != 0 && !self.locked && now.wrapping_sub(self.last_input) >= lock {
            self.locked = true;
        }
        let off = AUTO_OFF_TIMES[self.set.auto_off as usize % AUTO_OFF_TIMES.len()] as u32 * 60_000;
        let idle = now.wrapping_sub(if after(self.awake, self.seen) { self.awake } else { self.seen });
        if off != 0 {
            if idle >= off {
                self.lit |= WANT_OFF;
            } else if idle + OFF_WARN_MS >= off && self.lit & WARNED == 0 {
                self.lit = (self.lit | WARNED) & !DARK;
                self.show_toast(b"Off in 10 s", b"press a key", false);
            }
        }
    }
}

