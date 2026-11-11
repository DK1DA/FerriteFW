//! The radio state message (LINK-67, decision 373): the radio and its three slots as one fixed record, for Console
//! and the phone app. They read it whole by `STATE`, or have it pushed by `SUBSCRIBE` with only the parts that
//! changed, instead of parsing Live View's readback lines (LINK-47), which follow the screens and change when a
//! screen does. The layout is [`ferrite_core::link::state`]'s.

use crate::chan::ChannelStore;
use crate::App;
use ferrite_core::link::state as m;
use ferrite_core::msg::{Device, GpsState, LiveAll, ModeLive, SLOTS};

/// The whole record.
pub type Record = [u8; m::LEN];

/// The record now. `#[inline(never)]`: one copy, its buffer in its own frame.
#[inline(never)]
pub fn record<S: ChannelStore>(app: &App, st: &S, dev: &Device, live: &LiveAll) -> Record {
    let mut b = [0u8; m::LEN];
    radio(app, st, dev, live, &mut b[..m::RADIO_LEN]);
    for s in 0..SLOTS {
        let at = m::RADIO_LEN + s * m::SLOT_LEN;
        slot(app, live, s as u8, &mut b[at..at + m::SLOT_LEN]);
    }
    b
}

/// Where part `i` (0 the radio, 1-3 the slots) lies in the record.
pub fn part(i: usize) -> core::ops::Range<usize> {
    match i {
        0 => 0..m::RADIO_LEN,
        s => {
            let at = m::RADIO_LEN + (s - 1) * m::SLOT_LEN;
            at..at + m::SLOT_LEN
        }
    }
}

fn radio<S: ChannelStore>(app: &App, st: &S, dev: &Device, live: &LiveAll, b: &mut [u8]) {
    let flags = if app.radio.keyed.is_some() || live.tx.keyed { m::TX } else { 0 }
        | if app.dry_tx() { m::DRY } else { 0 }
        | if app.locked() { m::LOCKED } else { 0 }
        | if dev.battery_low { m::BATTERY_LOW } else { 0 };
    let gps = match dev.gps {
        GpsState::Off => 0,
        GpsState::Searching => 1,
        GpsState::Fix => 2,
    };
    let (lat, lon) = st.data().fix().map_or((m::NO_POS, m::NO_POS), |f| (f.pos.lat, f.pos.lon));
    b[..4].copy_from_slice(&[m::VERSION, app.active, flags, dev.battery_pct]);
    b[4..6].copy_from_slice(&dev.battery_mv.to_le_bytes());
    b[6..8].copy_from_slice(&[gps, dev.sats]);
    b[8..12].copy_from_slice(&lat.to_le_bytes());
    b[12..16].copy_from_slice(&lon.to_le_bytes());
}

fn slot(app: &App, live: &LiveAll, s: u8, b: &mut [u8]) {
    let i = s as usize;
    let sl = &app.slots[i];
    let lv = &live.slots[i];
    let bit = 1u8 << s;
    let on_ch = sl.on_channel() && sl.ch.is_some();
    let bcast = app.radio.fm_on && app.bcast_slot() == Some(s);
    let scanning = app.scan_mask & bit != 0;
    let keyed = app.radio.keyed == Some(s);
    let flags = if on_ch { m::ON_CHANNEL } else { 0 }
        | if scanning { m::SCANNING } else { 0 }
        | if lv.squelch_open && !bcast { m::OPEN } else { 0 }
        | if keyed { m::TRANSMITTING } else { 0 }
        | if app.aprs_slot(s) { m::APRS } else { 0 }
        | if bcast { m::BROADCAST } else { 0 }
        | if app.mute & bit != 0 { m::MUTED } else { 0 }
        | if app.slot_off(s) { m::OFF } else { 0 };
    let channel = sl.ch.filter(|_| on_ch).map_or(m::NONE, |c| c.num);
    let zone = match sl.zone {
        _ if !on_ch => m::NONE,
        crate::local::LOCAL => m::LOCAL,
        z => z,
    };
    // what it receives now: a scan's step, else where it is tuned
    let freq = if scanning && lv.freq != 0 { lv.freq } else { app.tuned_freq(s) };
    let c = app.tuned_cfg(s);
    let mut name = [0u8; m::NAME_LEN];
    let signal = match live.mode {
        // the broadcast: the SI4732's dBuV as dBm (50 ohms), and its RDS station name
        ModeLive::Fm { rssi, ps, .. } if bcast => {
            let ps = crate::bcast::trim(&ps);
            name[..ps.len()].copy_from_slice(ps);
            rssi as i16 - 107
        }
        _ if lv.squelch_open => lv.rssi,
        _ => m::NO_SIGNAL,
    };
    if let Some(ch) = sl.ch.filter(|_| on_ch) {
        let n = ch.name();
        let n = &n[..n.len().min(m::NAME_LEN)];
        name = [0; m::NAME_LEN];
        name[..n.len()].copy_from_slice(n);
    }
    b[0] = flags;
    b[1..3].copy_from_slice(&channel.to_le_bytes());
    b[3..5].copy_from_slice(&zone.to_le_bytes());
    b[5..9].copy_from_slice(&freq.to_le_bytes());
    b[9..11].copy_from_slice(&[c.mode, c.power]);
    b[11..13].copy_from_slice(&signal.to_le_bytes());
    b[13..13 + m::NAME_LEN].copy_from_slice(&name);
}
