//! Smart beaconing (Tony Arnerich KD7TA and Steve Bragg KA9MVA, HamHUD): beacon often when moving fast or
//! turning, rarely when slow or parked.

/// Settings; the defaults are the common ones for a car.
#[derive(Clone, Copy, Debug)]
pub struct SmartBeacon {
    /// At or above this speed (km/h), beacon every `fast_rate` seconds.
    pub fast_speed: u16,
    pub fast_rate: u32,
    /// Below this speed (km/h), beacon every `slow_rate` seconds.
    pub slow_speed: u16,
    pub slow_rate: u32,
    /// A turn of at least `min_turn_angle` + `turn_slope` / speed degrees beacons at once, if `min_turn_time`
    /// seconds have passed since the last beacon.
    pub min_turn_angle: u16,
    pub turn_slope: u16,
    pub min_turn_time: u32,
}

impl Default for SmartBeacon {
    fn default() -> Self {
        SmartBeacon {
            fast_speed: 100,
            fast_rate: 180,
            slow_speed: 5,
            slow_rate: 1800,
            min_turn_angle: 28,
            turn_slope: 255,
            min_turn_time: 30,
        }
    }
}

/// Smart beaconing's state: when and which way the last beacon went.
#[derive(Clone, Copy, Debug, Default)]
pub struct Beaconer {
    pub cfg: SmartBeacon,
    last: Option<(u32, u16)>,
}

impl Beaconer {
    pub fn new(cfg: SmartBeacon) -> Self {
        Beaconer { cfg, last: None }
    }

    /// Is a beacon due? `now` in seconds (any epoch, wrapping), speed in km/h, heading in degrees.
    pub fn due(&self, now: u32, speed: u16, heading: u16) -> bool {
        let Some((t, h)) = self.last else { return true };
        let c = &self.cfg;
        let since = now.wrapping_sub(t);
        let rate = if speed < c.slow_speed {
            c.slow_rate
        } else if speed >= c.fast_speed {
            c.fast_rate
        } else {
            (c.fast_rate as u64 * c.fast_speed as u64 / speed.max(1) as u64).min(u32::MAX as u64) as u32
        };
        if since >= rate {
            return true;
        }
        if speed >= c.slow_speed && since >= c.min_turn_time {
            let turn = (heading as i32 - h as i32).rem_euclid(360);
            let turn = turn.min(360 - turn) as u32;
            if turn >= c.min_turn_angle as u32 + c.turn_slope as u32 / speed.max(1) as u32 {
                return true;
            }
        }
        false
    }

    /// A beacon went out.
    pub fn sent(&mut self, now: u32, heading: u16) {
        self.last = Some((now, heading));
    }
}
