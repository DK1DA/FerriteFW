//! The owner's lists over the link (LINK-54 – LINK-56, decision 327): the quick messages (TEXT-7), the waypoints
//! (GPS-10) and the DTMF memories with their names (DTMF-12), by `READ_MANY` and `WRITE_MANY` as the channels go,
//! each record checked as the radio's own editors check it and written through the store's queue only when it
//! changes. Only from radios whose `HELLO` says [`ferrite_core::link::caps::LISTS`]; older ones answer `bad argument`.

use super::{name_of, request, Out, Window};
use crate::chan::ChannelStore;
use crate::data::{Dtmf, Op, Pos, Waypoint, WpSrc, DTMF_MEMS};
use crate::persist::Sink as PSink;
use crate::words::QUICK;
use ferrite_core::link::{what, Status, ADD, QUICK_OWN, WAYPOINT_LEN};
use ferrite_ui::text::Buf;

/// GPS-10's store.
const WAYPOINTS_MAX: u16 = 256;
/// A waypoint's sources in their record order (GPS-10).
const SOURCES: [WpSrc; 4] = [WpSrc::Marked, WpSrc::Aprs, WpSrc::Link, WpSrc::Grid];

/// How many indexes `w` has, None if it isn't one of these.
pub(super) fn indexes<S: ChannelStore>(st: &S, w: u8) -> Option<u16> {
    match w {
        what::QUICK => Some(QUICK as u16),
        what::WAYPOINT => Some(st.data().waypoints()),
        what::DTMF => Some(DTMF_MEMS as u16),
        _ => None,
    }
}

/// Whether index `i` holds a record: every quick message (yours or the factory text), the waypoints there are, the
/// DTMF memories set.
pub(super) fn held<S: ChannelStore>(st: &S, w: u8, i: u16) -> bool {
    match w {
        what::QUICK => (i as usize) < QUICK,
        what::WAYPOINT => i < st.data().waypoints(),
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
        _ => return Status::NotFound,
    }
    Status::Ok
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
                return invalid(o, "256 waypoints");
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
        _ => Status::NotFound,
    }
}
