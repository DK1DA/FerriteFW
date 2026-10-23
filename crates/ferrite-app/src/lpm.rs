//! Low Power Mode (manual chapter 33: PWR-24, decision 234): "the squeeze". Everything not needed to receive and
//! transmit on one VFO is off, as runtime overrides that never touch the stored settings, so leaving puts every
//! one back as it was:
//!
//! - **Radio:** only the active slot (a broadcast slot gives way to A); watch off, scan stopped and refused, APRS
//!   (mode, KISS, beacons) off, the decoders off, FM broadcast, HF mode and the crossband repeater (two VFOs)
//!   closed and refused; the Radio task's Battery Save on [`ferrite_core::rxsleep::LOW_POWER`]. Tuning, editing,
//!   the menus and the active slot's tools (scope, tone search, counter) stay as they are.
//! - **Peripherals:** GPS and Bluetooth off (the cable link stays).
//! - **Lights:** the backlight at most [`LPM_BACKLIGHT`], the keypad light off; live redraws once a second (the
//!   firmware's UI task asks [`App::low_power`]).
//! - **Core:** 24 MHz one second after entering, the build's speed at once on leaving (PWR-24.9: the firmware's UI
//!   task switches, `hal::clock::set_low`).
//!
//! In by 0 7 7 2 On, or Auto below 0 7 7 3's battery % (once per crossing); out by holding SIDE1 + SIDE2 for
//! [`LPM_COMBO_MS`] (never in), by 0 7 7 2 Off, or, entered by Auto, by itself at [`AUTO_OFF_PCT`] (the charger); On by hand never leaves by itself.

use crate::App;
use ferrite_core::keys::Key;
use ferrite_core::msg::{RadioCmd, SLOTS};

/// In the mode now.
pub(crate) const LPM_ON: u8 = 1;
/// Auto was left by the keys: it doesn't come back until the battery is [`AUTO_REARM`] points above its level.
pub(crate) const LPM_HOLD: u8 = 2;
/// SIDE1 and SIDE2 are both down (in the mode), since [`App`]'s `lpm_at`.
pub(crate) const LPM_COMBO: u8 = 4;

/// PWR-24.4: how long SIDE1 + SIDE2 are held to leave.
pub const LPM_COMBO_MS: u32 = 1000;
/// PWR-24.3: entered by Auto, it leaves by itself at this battery % (on the charger); On by hand never does.
pub const AUTO_OFF_PCT: u8 = 65;
/// PWR-24.3: after leaving by the keys, Auto waits for the battery this far above its level.
pub const AUTO_REARM: u8 = 10;
/// PWR-24.5: the backlight's most in the mode, thousandths (the level table's nearest at or below 30 %).
pub const LPM_BACKLIGHT: u16 = 250;
/// [`App::home_layout`] in the mode: [`crate::screens::lowpower`]'s home (PWR-24.7).
pub const HOME_LOW_POWER: u8 = 0xFE;
/// No battery reading yet.
pub(crate) const BATT_UNKNOWN: u8 = 0xFF;

impl App {
    /// PWR-24: in Low Power Mode now.
    pub fn low_power(&self) -> bool {
        self.lpm & LPM_ON != 0
    }

    /// The battery % (PWR-22's gauge) each time the firmware reads it: Auto's input (PWR-24.3).
    pub fn battery(&mut self, pct: u8) {
        self.batt = pct;
    }

    /// 0 7 7 2 set (the menu, `set`, #): Off leaves, On enters, Auto looks at the battery from now (a hold from
    /// leaving by the keys is forgotten).
    pub(crate) fn set_lpm_mode(&mut self, m: u8) {
        self.set.lpm = m % crate::settings::LPM_MODES.len() as u8;
        self.lpm &= !LPM_HOLD;
        // PWR-24.3: choosing Auto decides from the battery now: in only below the level (from On at 32 % it
        // leaves; it doesn't wait for 80 %).
        if self.set.lpm == 2 && self.low_power() && self.batt != BATT_UNKNOWN && self.batt >= self.set.lpm_below {
            self.lpm_leave(false);
        }
        self.lpm_tick();
    }

    /// Each tick: in or out as 0 7 7 2 and the battery say (PWR-24.2, PWR-24.3).
    pub(crate) fn lpm_tick(&mut self) {
        let on = self.low_power();
        let (b, below) = (self.batt, self.set.lpm_below);
        let want = match self.set.lpm {
            1 => true,
            2 if b == BATT_UNKNOWN => on,
            2 => {
                if self.lpm & LPM_HOLD != 0 && b >= below.saturating_add(AUTO_REARM) {
                    self.lpm &= !LPM_HOLD;
                }
                if on { b < AUTO_OFF_PCT } else { self.lpm & LPM_HOLD == 0 && b < below }
            }
            _ => false,
        };
        if want && !on {
            self.lpm_enter();
        } else if !want && on {
            self.lpm_leave(false);
        }
        if self.lpm & LPM_COMBO != 0 && self.now.wrapping_sub(self.lpm_at) >= LPM_COMBO_MS {
            self.lpm &= !LPM_COMBO;
            if self.keys.is_held(Key::Side(1)) && self.keys.is_held(Key::Side(2)) && self.low_power() {
                self.lpm_leave(true);
            }
        }
    }

    fn lpm_enter(&mut self) {
        self.lpm |= LPM_ON;
        (self.drawn.screen, self.drawn.sbar) = (None, None); // PWR-24.7: its own home, drawn whole
        self.go_home(); // the mode screens close, and wanted_mode keeps them off
        if self.bcast_slot() == Some(self.active) && self.active != 0 {
            self.active = 0; // PWR-24.4: a broadcast slot gives way to A
            self.out.push(RadioCmd::Active(0));
        }
        for s in 0..SLOTS as u8 {
            if self.scan_mask & 1 << s != 0 {
                self.out.push(RadioCmd::Scan { slot: s, on: false });
            }
        }
        self.scan_mask = 0;
        self.c_follow(); // APRS gives C back; watch goes to the active slot alone
        self.out.push(RadioCmd::LowPower(true));
        self.show_toast(b"Low Power Mode", b"hold SIDE1+2: off", false);
    }

    /// Out: `keys` when SIDE1 + SIDE2 did it (On becomes Off; Auto holds off until the battery is back up).
    fn lpm_leave(&mut self, keys: bool) {
        self.lpm &= !LPM_ON;
        (self.drawn.screen, self.drawn.sbar) = (None, None); // the normal home, bars included, drawn whole
        if keys {
            match self.set.lpm {
                1 => self.set.lpm = 0,
                2 => self.lpm |= LPM_HOLD,
                _ => {}
            }
        }
        self.c_follow(); // watch and APRS as set
        self.out.push(RadioCmd::LowPower(false));
        self.reconcile_mode(); // the strip or a broadcast come back
        self.show_toast(b"Low Power Mode", b"off", false);
    }

    /// PWR-24.4: a side key went down in the mode with the other already held: both are taken (their own
    /// presses, holds and monitor don't happen) and the hold starts. True when it took this key.
    pub(crate) fn lpm_combo_down(&mut self, key: Key, now: u32) -> bool {
        let other = match key {
            Key::Side(1) => Key::Side(2),
            Key::Side(2) => Key::Side(1),
            _ => return false,
        };
        if !self.low_power() || !self.keys.is_held(other) {
            return false;
        }
        self.keys.down(key, now);
        self.keys.kill(Some(key));
        self.keys.kill(Some(other));
        if other == Key::Side(1) {
            self.screen.raw(self, Some(other), None, false); // SIDE1's held monitor ends
        }
        (self.lpm, self.lpm_at) = (self.lpm | LPM_COMBO, now);
        true
    }

    /// A side key let go: the hold is over.
    pub(crate) fn lpm_combo_up(&mut self, key: Key) {
        if matches!(key, Key::Side(1) | Key::Side(2)) {
            self.lpm &= !LPM_COMBO;
        }
    }

    /// The home layout drawn now: 0 7 2's. The one place home is chosen, so Low Power Mode's own home (its mock-up
    /// is waiting for approval) can take over here.
    pub fn home_layout(&self) -> u8 {
        if self.low_power() { HOME_LOW_POWER } else { self.set.layout }
    }

    /// PWR-24.4: how far the SIDE1 + SIDE2 hold to leave has got, in thousandths (None: not held), for a fill bar.
    pub fn lpm_leave_progress(&self) -> Option<u16> {
        (self.lpm & LPM_COMBO != 0).then(|| (self.now.wrapping_sub(self.lpm_at).min(LPM_COMBO_MS) * 1000 / LPM_COMBO_MS) as u16)
    }

    /// PWR-24.4: what the mode doesn't do (a scan, a mode screen) says so.
    pub(crate) fn lpm_refuse(&mut self) {
        self.show_toast(b"Low Power Mode", b"hold SIDE1+2: off", false); // the way out, when it's wanted
    }

    /// PWR-24: Bluetooth on as 0 9 2 1 says, but never in the mode.
    pub fn bt_on(&self) -> bool {
        self.set.bt && !self.low_power()
    }

    /// PWR-24: the GPS task's settings, with the module off in the mode.
    pub fn gps_cfg(&self) -> crate::gpsdata::GpsCfg {
        let mut c = crate::gpsdata::cfg(self.set);
        c.on &= !self.low_power();
        c
    }

    /// PWR-24: SAME alerts decoded (0 4 8 1, WX-3), never in the mode.
    pub fn same_wanted(&self) -> bool {
        self.set.same_on() && !self.low_power()
    }

    /// PWR-24: KISS on (KISS-6), never in the mode.
    pub fn kiss_on(&self) -> bool {
        self.set.kiss_on() && !self.low_power()
    }
}
