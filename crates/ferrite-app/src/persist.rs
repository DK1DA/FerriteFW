//! What survives a power cycle, as bytes: the radio-wide settings and the slots (frequencies, VFO settings, the
//! channel each is on, watch and Battery Save). Each type writes its fields in a fixed order, little-endian, so
//! the format doesn't depend on the compiler's struct layout; a reader that runs out of bytes leaves the rest at
//! their current values (the defaults), so a record from an older firmware that had fewer fields still loads
//! (a version change migrates what it can). Values are not trusted: everything that indexes a table is taken
//! modulo or checked where it is used, as for a fresh setting.
//!
//! The storage crate streams these through its own sink and source, so no buffer of the whole record exists.

use crate::chan::ChannelStore;
use crate::screens::hf::HfMem;
use crate::settings::{Settings, Text};
use crate::slot::SlotCfg;
use crate::{App, SLOTS};
use ferrite_core::msg::{RadioCmd, XbCfg};

/// Where bytes go (a CRC, a flash page buffer, a test vector).
pub trait Sink {
    fn put(&mut self, b: &[u8]);
}

/// Where bytes come from. `take` fills `out` and returns true, or returns false (and leaves `out`) when fewer
/// bytes are left.
pub trait Source {
    fn take(&mut self, out: &mut [u8]) -> bool;
}

/// A type that saves and loads itself, field by field.
pub trait Persist {
    fn save(&self, s: &mut dyn Sink);
    fn load(&mut self, r: &mut dyn Source);
}

macro_rules! int {
    ($($t:ty),*) => {$(
        impl Persist for $t {
            fn save(&self, s: &mut dyn Sink) {
                s.put(&self.to_le_bytes());
            }
            fn load(&mut self, r: &mut dyn Source) {
                let mut b = [0u8; core::mem::size_of::<$t>()];
                if r.take(&mut b) {
                    *self = <$t>::from_le_bytes(b);
                }
            }
        }
    )*};
}
int!(u8, i8, u16, i16, u32, i32);

impl Persist for bool {
    fn save(&self, s: &mut dyn Sink) {
        s.put(&[*self as u8]);
    }
    fn load(&mut self, r: &mut dyn Source) {
        let mut b = [0u8];
        if r.take(&mut b) {
            *self = b[0] != 0;
        }
    }
}

/// A channel number or none: 0xFFFF is none.
impl Persist for Option<u16> {
    fn save(&self, s: &mut dyn Sink) {
        self.unwrap_or(0xFFFF).save(s);
    }
    fn load(&mut self, r: &mut dyn Source) {
        let mut v = self.unwrap_or(0xFFFF);
        v.load(r);
        *self = (v != 0xFFFF).then_some(v);
    }
}

impl<T: Persist, const N: usize> Persist for [T; N] {
    fn save(&self, s: &mut dyn Sink) {
        self.iter().for_each(|x| x.save(s));
    }
    fn load(&mut self, r: &mut dyn Source) {
        self.iter_mut().for_each(|x| x.load(r));
    }
}

impl<const N: usize> Persist for Text<N> {
    fn save(&self, s: &mut dyn Sink) {
        s.put(&self.0);
    }
    fn load(&mut self, r: &mut dyn Source) {
        let mut b = [0u8; N];
        if r.take(&mut b) {
            self.0 = b;
        }
    }
}

/// `persist!(Type { field, ... })`: saves and loads the fields in this order. New fields go at the end only.
macro_rules! persist {
    // Booleans after the fields share one byte, bit 0 first, each stored as "differs from the type's DEFAULT": a
    // record from before a flag joined reads the old byte (one bool, 0 or 1) as the first flag, and the flags it
    // didn't have read 0, their defaults.
    // Fields in `then` come after the flag byte, so an older record's flag byte stays where it was.
    ($t:ty { $($f:ident),* $(,)? } bits { $($b:ident),* $(,)? } $(then { $($g:ident),* $(,)? })?) => {
        impl Persist for $t {
            fn save(&self, s: &mut dyn Sink) {
                $(self.$f.save(s);)*
                let d = <$t>::DEFAULT;
                let mut v = 0u8;
                let mut _i = 0;
                $(v |= ((self.$b != d.$b) as u8) << _i; _i += 1;)*
                v.save(s);
                $($(self.$g.save(s);)*)?
            }
            fn load(&mut self, r: &mut dyn Source) {
                $(self.$f.load(r);)*
                let d = <$t>::DEFAULT;
                let mut v = 0u8;
                let mut _i = 0;
                $(v |= ((self.$b != d.$b) as u8) << _i; _i += 1;)*
                v.load(r);
                let mut _i = 0;
                $(self.$b = d.$b ^ (v >> _i & 1 != 0); _i += 1;)*
                $($(self.$g.load(r);)*)?
            }
        }
    };
    ($t:ty { $($f:ident),* $(,)? }) => {
        impl Persist for $t {
            fn save(&self, s: &mut dyn Sink) {
                $(self.$f.save(s);)*
            }
            fn load(&mut self, r: &mut dyn Source) {
                $(self.$f.load(r);)*
            }
        }
    };
}

persist!(crate::data::Pos { lat, lon });
persist!(XbCfg { side1, side2, power, timeout_10s, hang_ds, flags });
persist!(HfMem { freq, mode, step, bw, agc, lna, bfo, recent, bands, band_bfo });
/// A slot's settings. Split rides in the reverse byte (bit 0 reverse, bit 1 split) so the record keeps its
/// length and older records (0 or 1 there) read as not split.
impl Persist for SlotCfg {
    fn save(&self, s: &mut dyn Sink) {
        self.mode.save(s);
        self.narrow.save(s);
        self.rx_tone.save(s);
        self.tx_tone.save(s);
        self.offset.save(s);
        self.power.save(s);
        self.step.save(s);
        self.squelch.save(s);
        self.busy.save(s);
        (self.reverse as u8 | (self.split as u8) << 1).save(s);
    }
    fn load(&mut self, r: &mut dyn Source) {
        self.mode.load(r);
        self.narrow.load(r);
        self.rx_tone.load(r);
        self.tx_tone.load(r);
        self.offset.load(r);
        self.power.load(r);
        self.step.load(r);
        self.squelch.load(r);
        self.busy.load(r);
        let mut f = self.reverse as u8 | (self.split as u8) << 1;
        f.load(r);
        (self.reverse, self.split) = (f & 1 != 0, f & 2 != 0);
        self.fold_bw(); // VFO-20: a saved FM + narrow reads as NFM
    }
}
persist!(Settings {
    callsign, owner, boot_card, region, extend_tx, extend_warned, tz, clock, clock_gps, units, layout, theme,
    pin_mode, pin, pin_len, pin_misses, pin_wait, bt, same, fm_freq, fm_step, fm_agc, fm_monitor, setup_done,
    home_ch, xb, hf, cable_port, coord_fmt, fixed_on, fixed, fixed_alt, same_codes, scope_bars, scope_step,
    scope_dwell, scope_trig, scope_line, scope_wf, scope_mode, term_colour,
    aprs_mode, predictive,
    spare_tokens, backlight, keypad_light, leds, auto_off, auto_lock, dtmf_sidetone, arrival_tone,
} bits { show_hidden, chan_only, cable_log, spare_flag2, scan_tx } then { pon, pon_text, tone_level, scan_resume, scan_prio, tail_mode, tail_phase, end_tone, dtmf_ms, dtmf_delay, cw_id, gps_on, gps_rxtx, arrival, kiss_port, pocsag_on, pocsag_freq, pocsag_cap, popups, key_beep, beep_vol, alert_tones, vox, vox_level, vox_tail, fm_sleep, brightness, brightness_night, tx_light, wake_on, mcu_temp_off });

/// The format of [`App::save_state`]; a reader of a different one keeps the defaults.
pub const STATE_VERSION: u8 = 1;

impl App {
    /// The App's part of what survives a power cycle: the active slot, watch, Battery Save and each slot (VFO or
    /// channel mode, its VFO, the VFO's settings and band memories, its zone and position, the channel it is
    /// on). The radio-wide [`Settings`] are saved apart, as the boot screen needs them before the App exists.
    pub fn save_state(&self, s: &mut dyn Sink) {
        STATE_VERSION.save(s);
        (SLOTS as u8).save(s);
        self.active.save(s);
        self.watch.save(s);
        self.bsave.save(s);
        for sl in &self.slots {
            sl.vfo_mode.save(s);
            sl.vfo.save(s);
            sl.cfg.save(s);
            sl.mem.save(s);
            sl.zone.save(s);
            sl.pos.save(s);
            sl.ch.map(|c| c.num).save(s);
        }
    }

    /// The saved state back, at power on before the first draw. A slot on a channel reads that channel from the
    /// store again (edits not saved to it are gone, as on the radio); a channel that no longer exists leaves the
    /// slot on its VFO. Then the radio gets every slot again.
    pub fn restore_state(&mut self, r: &mut dyn Source, store: &impl ChannelStore) {
        let (mut ver, mut n) = (0u8, 0u8);
        ver.load(r);
        n.load(r);
        if ver != STATE_VERSION || n as usize != SLOTS {
            return;
        }
        self.active.load(r);
        self.active %= SLOTS as u8;
        self.watch.load(r);
        self.watch %= 3;
        self.bsave.load(r);
        self.bsave %= 4;
        for sl in &mut self.slots {
            sl.vfo_mode.load(r);
            sl.vfo.load(r);
            sl.cfg.load(r);
            sl.mem.load(r);
            sl.zone.load(r);
            sl.pos.load(r);
            let mut num: Option<u16> = None;
            num.load(r);
            sl.ch = None;
            if let Some((c, pos)) = num.and_then(|n| store.by_num(n)) {
                (sl.ch, sl.zone, sl.pos) = (Some(c), c.zone, pos);
            } else {
                sl.vfo_mode = true;
            }
        }
        for s in 0..SLOTS as u8 {
            self.send_slot(s);
        }
        self.out.push(RadioCmd::Active(self.active));
        self.out.push(RadioCmd::Watch(self.watch_mask()));
        self.out.push(RadioCmd::Awake((self.set.aprs_mode as u8) << crate::aprs::APRS_SLOT));
        self.send_radio_settings();
        self.out.push(RadioCmd::BatterySave(crate::level(self.bsave)));
    }
}

/// A byte slice as a [`Source`] (tests, the simulator).
pub struct SliceSource<'a>(pub &'a [u8]);

impl Source for SliceSource<'_> {
    fn take(&mut self, out: &mut [u8]) -> bool {
        if self.0.len() < out.len() {
            return false;
        }
        let (a, b) = self.0.split_at(out.len());
        out.copy_from_slice(a);
        self.0 = b;
        true
    }
}

/// Counts what is saved (sizing a record before writing it).
pub struct Count(pub usize);

impl Sink for Count {
    fn put(&mut self, b: &[u8]) {
        self.0 += b.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sample;

    struct Vec2(std::vec::Vec<u8>);
    impl Sink for Vec2 {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }

    #[test]
    fn settings_round_trip_and_short_records_keep_defaults() {
        let mut a = crate::sample::settings();
        a.set_callsign(b"ve3abc");
        a.home_ch = Some(11);
        a.hf.bfo = -120;
        let mut v = Vec2(std::vec::Vec::new());
        a.save(&mut v);
        let mut b = Settings::DEFAULT;
        b.load(&mut SliceSource(&v.0));
        assert_eq!(a, b);
        // An older, shorter record: what it has loads, the rest stays default.
        let mut c = Settings::DEFAULT;
        c.load(&mut SliceSource(&v.0[..10]));
        assert_eq!(c.callsign, a.callsign);
        assert_eq!(c.hf, Settings::DEFAULT.hf);
    }

    /// The settings' length, the settings and the slots must fit the store's 1 KB record (ferrite-store
    /// `settings::MAX_PAYLOAD`).
    #[test]
    fn record_fits_the_store() {
        let leak = || -> &'static mut Settings { std::boxed::Box::leak(std::boxed::Box::new(Settings::DEFAULT)) };
        let alert = || std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        let a = App::new(leak(), alert());
        let mut c = Count(2);
        Settings::DEFAULT.save(&mut c);
        a.save_state(&mut c);
        assert!(c.0 <= 1024, "{} bytes", c.0);
    }

    #[test]
    fn app_state_round_trip() {
        let store = Sample::new();
        let leak = || -> &'static mut Settings { std::boxed::Box::leak(std::boxed::Box::new(Settings::DEFAULT)) };
        let alert = || std::boxed::Box::leak(std::boxed::Box::new(crate::screens::alert::AlertInfo::NONE));
        let mut a = App::new(leak(), alert());
        a.set_channel(1, 1, 1, &store);
        a.set_watch(2);
        a.slots[0].vfo = 14_652_000;
        a.active = 2;
        let mut v = Vec2(std::vec::Vec::new());
        a.save_state(&mut v);
        let mut b = App::new(leak(), alert());
        b.restore_state(&mut SliceSource(&v.0), &store);
        assert_eq!(b.active, 2);
        assert_eq!(b.watch, 2);
        assert_eq!(b.slots[0].vfo, 14_652_000);
        assert_eq!(b.slots[1].ch.map(|c| c.num), a.slots[1].ch.map(|c| c.num));
        assert!(!b.slots[1].vfo_mode);
        // A record of another format leaves everything as it was.
        let mut c = App::new(leak(), alert());
        v.0[0] = 99;
        c.restore_state(&mut SliceSource(&v.0), &store);
        assert_eq!(c.active, 0);
    }
}
