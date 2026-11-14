//! The owner's lists over the link (LINK-54 – LINK-56, decision 327): the quick messages (TEXT-7), the waypoints
//! (GPS-10) and the DTMF memories with their names (DTMF-12), by `READ_MANY` and `WRITE_MANY` as the channels go,
//! each record checked as the radio's own editors check it and written through the store's queue only when it
//! changes. A range preset's skipped frequencies (LINK-58, SCAN-10) go the same way, and satellite mode's records
//! (LINK-63, LINK-64, SAT-2) too.

use super::{name_of, request, Out, Window};
use crate::chan::ChannelStore;
use crate::data::{Dtmf, Op, Pos, Waypoint, WpSrc, DTMF_MEMS, SKIP_CHUNK};
use crate::persist::Sink as PSink;
use crate::words::QUICK;
use ferrite_core::link::{what, Status, ADD, PRESET_SKIPS_MAX, QUICK_OWN, SATS_MAX, SAT_INFO_LEN, SAT_LEN, WAYPOINT_LEN};
use ferrite_core::sat::wire;
use ferrite_ui::text::Buf;

/// GPS-10's store (decision 333: 16 sectors of 127).
pub const WAYPOINTS_MAX: u16 = 2032;
/// A waypoint's sources in their record order (GPS-10).
const SOURCES: [WpSrc; 4] = [WpSrc::Marked, WpSrc::Aprs, WpSrc::Link, WpSrc::Grid];

/// How many indexes `w` has, None if it isn't one of these.
pub(super) fn indexes<S: ChannelStore>(st: &S, w: u8) -> Option<u16> {
    match w {
        what::QUICK => Some(QUICK as u16),
        what::WAYPOINT => Some(st.data().waypoints()),
        what::DTMF => Some(DTMF_MEMS as u16),
        what::PRESET_SKIPS => Some(st.data().presets() as u16),
        what::SAT => Some(SATS_MAX),
        what::SAT_INFO => Some(1),
        _ => None,
    }
}

/// Whether index `i` holds a record: every quick message (yours or the factory text), the waypoints there are, the
/// DTMF memories set.
pub(super) fn held<S: ChannelStore>(st: &S, w: u8, i: u16) -> bool {
    match w {
        what::QUICK => (i as usize) < QUICK,
        what::WAYPOINT => i < st.data().waypoints(),
        what::PRESET_SKIPS => i < 256 && preset_skips(st.data(), i as u8, |_| ()) > 0,
        what::SAT => i < SATS_MAX && st.data().sat_places() & 1 << i != 0,
        what::SAT_INFO => i == 0 && st.data().sat_info().is_some(),
        _ => i < DTMF_MEMS as u16 && st.data().dtmf(i as u8).is_some(),
    }
}

/// Record `idx` of `w`, as `READ_MANY` gives it.
pub(super) fn record<S: ChannelStore>(st: &S, w: u8, idx: u16, win: &mut Window) -> Status {
    let d = st.data();
    match w {
        // LINK-54: flags (the owner's own or the factory text), then the text the radio uses
        what::QUICK if (idx as usize) < QUICK => {
            let own = d.quick(idx as usize);
            win.put(&[if own.is_some() { QUICK_OWN } else { 0 }]);
            win.put(crate::screens::msgs::quick_text(st, idx as usize).as_bytes());
        }
        // LINK-55: name (16, 0-padded), latitude, longitude, source
        what::WAYPOINT => match d.waypoint(idx) {
            Some(p) => {
                let mut n = [0u8; 16];
                n[..p.name.len()].copy_from_slice(p.name.as_bytes());
                win.put(&n);
                win.put(&p.pos.lat.to_le_bytes());
                win.put(&p.pos.lon.to_le_bytes());
                win.put(&[p.src as u8]);
            }
            None => return Status::NotFound,
        },
        // LINK-56: the symbols (length, then as text), then the name
        what::DTMF => match d.dtmf(idx as u8).filter(|_| idx < DTMF_MEMS as u16) {
            Some(m) => {
                let mut b = Buf::<16>::new();
                m.text(&mut b);
                win.put(&[b.len() as u8]);
                win.put(b.as_bytes());
                win.put(d.dtmf_name(idx as u8).as_bytes());
            }
            None => return Status::NotFound,
        },
        // LINK-58: the skipped frequencies, u32 each
        what::PRESET_SKIPS if idx < 256 && preset_skips(d, idx as u8, |f| win.put(&f.to_le_bytes())) > 0 => {}
        // LINK-63, LINK-64: a satellite, the satellites' info
        what::SAT => match d.sat_rec(idx.min(255) as u8).filter(|_| idx < SATS_MAX) {
            Some(s) => win.put(&wire::encode(&s)),
            None => return Status::NotFound,
        },
        what::SAT_INFO => match d.sat_info().filter(|_| idx == 0) {
            Some(i) => win.put(&i.encode()),
            None => return Status::NotFound,
        },
        _ => return Status::NotFound,
    }
    Status::Ok
}

/// LINK-58.1: preset `i`'s skipped frequencies as stored, each to `put`, blank entries (erased flash's 0xFFFFFFFF, or
/// 0: never a frequency) left out; how many went.
fn preset_skips(d: &dyn crate::data::Data, i: u8, mut put: impl FnMut(u32)) -> usize {
    let n = d.preset(i).map_or(0, |p| p.skips.min(PRESET_SKIPS_MAX as u8));
    let mut k = 0;
    for f in (0..n).filter_map(|j| d.preset_skip(i, j)).filter(|&f| f != u32::MAX && f != 0) {
        put(f);
        k += 1;
    }
    k
}

/// Printable ASCII but `|`, `~` and `{` (APRS-7: what a message can carry, as `MSG_SEND` checks, LINK-45).
fn aprs_text(t: &[u8]) -> bool {
    t.iter().all(|&c| (0x20..0x7F).contains(&c) && !b"|~{".contains(&c))
}

/// A record of `w` written (`WRITE_MANY`): checked as the radio's editors check it; nothing is written when it is as
/// the radio has it.
#[inline(never)] // one copy (flash)
pub(super) fn apply<S: ChannelStore>(st: &S, w: u8, idx: u16, rec: &[u8], o: &mut Out) -> Status {
    let invalid = |o: &mut Out, why: &str| {
        o.put(why.as_bytes());
        Status::Invalid
    };
    let d = st.data();
    match w {
        // LINK-54: flag clear, the factory text back (the text is ignored); set, the owner's (TEXT-7: empty stays
        // empty)
        what::QUICK if (idx as usize) < QUICK => {
            let Some((&f, t)) = rec.split_first() else { return Status::BadArg };
            let new = if f & QUICK_OWN != 0 {
                if t.len() > 67 || !aprs_text(t) {
                    return invalid(o, "quick message: up to 67 printable characters, no | ~ {");
                }
                Some(Buf::from(t))
            } else {
                None
            };
            if d.quick(idx as usize) == new {
                return Status::Ok;
            }
            request(st, Op::Quick(idx as u8, new))
        }
        // LINK-55: replaced in its place, added at ADD, deleted with an empty record
        what::WAYPOINT if idx < d.waypoints() || idx == ADD => {
            if rec.is_empty() {
                return if idx == ADD { Status::BadArg } else { request(st, Op::DeleteWaypoint(idx)) };
            }
            if rec.len() != WAYPOINT_LEN {
                return Status::BadArg;
            }
            let Some(nm) = name_of::<16>(&rec[..16]).filter(|n| !n.is_empty()) else { return invalid(o, "name: 1 to 16 printable characters") };
            let at = |k: usize| i32::from_le_bytes([rec[k], rec[k + 1], rec[k + 2], rec[k + 3]]);
            let (lat, lon, src) = (at(16), at(20), rec[24]);
            if lat.unsigned_abs() > 900_000_000 || lon.unsigned_abs() > 1_800_000_000 {
                return invalid(o, "position: latitude -90 to 90, longitude -180 to 180");
            }
            let Some(&src) = SOURCES.get(src as usize) else { return invalid(o, "source: 0 marked, 1 APRS, 2 link, 3 grid") };
            let wp = Waypoint { name: nm, pos: Pos { lat, lon }, src };
            if idx == ADD && d.waypoints() >= WAYPOINTS_MAX {
                return invalid(o, "2032 waypoints");
            }
            if d.waypoint(idx) == Some(wp) {
                return Status::Ok;
            }
            request(st, Op::PutWaypoint(idx, wp))
        }
        // LINK-56: an empty record clears the memory and its name; else the symbols and the name, each written only
        // when it changed
        what::DTMF if idx < DTMF_MEMS as u16 => {
            let i = idx as u8;
            let Some((&n, rest)) = rec.split_first() else {
                return if d.dtmf(i).is_some() { request(st, Op::Dtmf(i, None)) } else { Status::Ok };
            };
            let (Some(sym), Some(name)) = (rest.get(..n as usize), rest.get(n as usize..)) else { return Status::BadArg };
            let Some(m) = Dtmf::parse(sym).filter(|m| m.len > 0) else { return invalid(o, "symbols: 1 to 16 of 0-9 A-D * #") };
            let Some(nm) = name_of::<10>(name).filter(|_| name.len() <= 10) else { return invalid(o, "name: up to 10 printable characters") };
            if d.dtmf(i) != Some(m) && request(st, Op::Dtmf(i, Some(m))) != Status::Ok {
                return Status::Busy;
            }
            if d.dtmf_name(i) != nm {
                return request(st, Op::DtmfName(i, nm));
            }
            Status::Ok
        }
        // LINK-58: the whole list replaced (length 0 clears it), each within the range (SCAN-17 adds only hits in
        // it), none twice; in chunks of SKIP_CHUNK through the store's queue, only when it changed
        what::PRESET_SKIPS if idx < d.presets() as u16 => {
            let Some(p) = d.preset(idx as u8) else { return Status::NotFound };
            if !rec.len().is_multiple_of(4) {
                return Status::BadArg;
            }
            let n = rec.len() / 4;
            if n > PRESET_SKIPS_MAX {
                return invalid(o, "48 skipped frequencies at most");
            }
            let f = |j: usize| u32::from_le_bytes([rec[4 * j], rec[4 * j + 1], rec[4 * j + 2], rec[4 * j + 3]]);
            if (0..n).any(|j| !(p.start..=p.end).contains(&f(j))) {
                return invalid(o, "skip: within the range's start and end");
            }
            if (0..n).any(|j| (0..j).any(|k| f(k) == f(j))) {
                return invalid(o, "skip: each frequency once");
            }
            if p.skips as usize == n && (0..n).all(|j| d.preset_skip(idx as u8, j as u8) == Some(f(j))) {
                return Status::Ok;
            }
            let mut at = 0;
            loop {
                let mut c = [0; SKIP_CHUNK];
                for (k, v) in c.iter_mut().enumerate().take(n.saturating_sub(at)) {
                    *v = f(at + k);
                }
                if request(st, Op::PresetSkips { preset: idx as u8, at: at as u8, count: n as u8, f: c }) != Status::Ok {
                    return Status::Busy;
                }
                at += SKIP_CHUNK;
                if at >= n {
                    return Status::Ok;
                }
            }
        }
        // LINK-63: replaced in its place (checked as SAT-2 and SAT-13 say), deleted with an empty record, all deleted
        // with an empty record at ADD; nothing written when it is as stored
        what::SAT if idx == ADD && rec.is_empty() => if d.sat_places() == 0 { Status::Ok } else { request(st, Op::ClearSats) },
        what::SAT if idx < SATS_MAX => {
            let k = idx as u8;
            if rec.is_empty() {
                return if d.sat_places() & 1 << k == 0 { Status::Ok } else { request(st, Op::DeleteSat(k)) };
            }
            if rec.len() != SAT_LEN {
                return Status::BadArg;
            }
            let s = match wire::decode(rec) {
                Ok(s) => s,
                Err(why) => return invalid(o, why),
            };
            if d.sat_rec(k) == Some(s) {
                return Status::Ok;
            }
            if !d.sat_stage(&s) {
                return Status::Busy;
            }
            request(st, Op::PutSat(k))
        }
        // LINK-64: the declination and the list's stamps; an empty record clears them
        what::SAT_INFO if idx == 0 => {
            let new = match rec.len() {
                0 => None,
                SAT_INFO_LEN => match wire::Info::decode(rec) {
                    Ok(i) => Some(i),
                    Err(why) => return invalid(o, why),
                },
                _ => return Status::BadArg,
            };
            if d.sat_info() == new {
                return Status::Ok;
            }
            request(st, Op::SatInfo(new))
        }
        _ => Status::NotFound,
    }
}
