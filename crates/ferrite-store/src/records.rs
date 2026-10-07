//! The fixed-size records at fixed addresses (`docs/firmware-design.md`, "Records"): channel n, zone z and range
//! preset p each have one place, computed from the number. Channels are 64 B (63 per sector), zones 32 B (127
//! per sector), presets 224 B (16 in one sector); the last bytes of every sector stay free, for the scratch
//! copy's footer ([`crate::scratch`]). Each record carries a CRC; an erased or torn record reads as empty.
//! Fields the UI doesn't model yet (a channel's position, coverage, service type, PTT ID option; a preset's skip
//! list) are kept as they were when a record is rewritten.

use crate::{crc16, u16_at, u32_at, SECTOR};
use ferrite_app::chan::{Channel, Name, NO_ZONE};
use ferrite_app::data::{Dtmf, Pos, RangePreset, ZoneCfg, DTMF_MEMS};
use ferrite_app::slot::{band_near, SlotCfg, BANDS, DEFAULT_CFG, SQ_GLOBAL};
use ferrite_core::layout::{CHANNELS, PRESETS, ZONES};
use ferrite_ui::text::Buf;

// ---- channels ----

pub const CH_LEN: usize = 64;
/// The record's format, at offset 60.
const CH_VERSION: u8 = 1;
/// Flag bits the UI's `Channel::flags` carries (0-2 skip, APRS, RX-only; 5-6 talkaround, hidden); 3-4 are
/// busy lockout from the slot settings, 7 "has position" (kept), 9 reverse, 10 split (TX at bytes 4-7 is the
/// transmit frequency itself, not RX plus a shift: CHAN-2); 8 and 11-15 are kept as they were. A record from
/// before split has bit 10 clear and reads as a shift, as it always did.
const UI_FLAGS: u16 = 0x0067;
/// The UI's priority flag (`FLAG_PRIORITY`, 0x80 in `Channel::flags`) lives in record bit 8.
const PRIORITY: u16 = 1 << 8;
const BUSY_SHIFT: u16 = 3;
const REVERSE: u16 = 1 << 9;
const SPLIT: u16 = 1 << 10;
const HAS_POS: u16 = 1 << 7;
pub const NO_POS: i32 = 0x7FFF_FFFF;

/// Channels in one sector.
pub const CH_PER_SECTOR: u32 = 63;

/// Channel `num`'s record: sector num / 63, slot num % 63.
pub fn ch_addr(num: u16) -> u32 {
    let n = num as u32;
    CHANNELS.start + n / CH_PER_SECTOR * SECTOR + n % CH_PER_SECTOR * CH_LEN as u32
}

/// Whether a record holds a channel: a good CRC and an RX frequency.
pub fn ch_valid(b: &[u8; CH_LEN]) -> bool {
    u32_at(b, 0) != 0xFFFF_FFFF && crc16(&b[..62]) == u16_at(b, 62) && b[60] == CH_VERSION
}

/// The channel in record `b` at number `num`, if it holds one.
pub fn ch_decode(num: u16, b: &[u8; CH_LEN]) -> Option<Channel> {
    if !ch_valid(b) {
        return None;
    }
    let rx = u32_at(b, 0);
    let tx = u32_at(b, 4);
    let flags = u16_at(b, 16);
    let mut cfg: SlotCfg = DEFAULT_CFG;
    cfg.mode = b[8];
    cfg.narrow = b[9] != 0;
    cfg.fold_bw(); // VFO-20: FM + narrow from before reads as NFM
    cfg.power = b[10].saturating_sub(1);
    cfg.step = match b[11] {
        0 => BANDS[band_near(rx)].step,
        s => s - 1,
    };
    cfg.rx_tone = u16_at(b, 12);
    cfg.tx_tone = u16_at(b, 14);
    cfg.split = flags & SPLIT != 0 && tx != 0;
    cfg.offset = if tx == 0 {
        0
    } else if cfg.split {
        tx as i32
    } else {
        (tx as i64 - rx as i64) as i32
    };
    cfg.busy = ((flags >> BUSY_SHIFT) & 3) as u8;
    cfg.reverse = flags & REVERSE != 0;
    // 0 = Global (the slot's level), 1-10 = levels 0-9 (CHAN-14). Records written before Global existed always
    // stored level + 1, so they read the same.
    cfg.squelch = match b[19] {
        0 => SQ_GLOBAL,
        s => (s - 1).min(9),
    };
    let mut name = [0u8; 16];
    name.copy_from_slice(&b[32..48]);
    let ui = (flags & UI_FLAGS) as u8 | if flags & PRIORITY != 0 { ferrite_app::chan::FLAG_PRIORITY } else { 0 };
    Some(Channel { num, zone: b[18], rx, name, cfg, flags: ui })
}

/// The record for `ch`, keeping what the UI doesn't model from `old` (the record it replaces, or the one it
/// moves from); a new record has no position.
pub fn ch_encode(ch: &Channel, old: &[u8; CH_LEN]) -> [u8; CH_LEN] {
    let keep = ch_valid(old);
    let mut b = [0u8; CH_LEN];
    if keep {
        b[20..32].copy_from_slice(&old[20..32]);
        b[48..60].copy_from_slice(&old[48..60]);
    } else {
        b[20..24].copy_from_slice(&NO_POS.to_le_bytes());
        b[24..28].copy_from_slice(&NO_POS.to_le_bytes());
        b[48..60].fill(0xFF);
        b[48] = 0;
    }
    let c = &ch.cfg;
    b[0..4].copy_from_slice(&ch.rx.to_le_bytes());
    let tx = if c.split {
        c.offset.max(0) as u32
    } else if c.offset == 0 {
        0
    } else {
        (ch.rx as i64 + c.offset as i64) as u32
    };
    b[4..8].copy_from_slice(&tx.to_le_bytes());
    b[8] = c.mode;
    b[9] = c.narrow as u8;
    b[10] = c.power + 1;
    b[11] = c.step + 1;
    b[12..14].copy_from_slice(&c.rx_tone.to_le_bytes());
    b[14..16].copy_from_slice(&c.tx_tone.to_le_bytes());
    let old_flags = if keep { u16_at(old, 16) } else { 0 };
    let flags = (old_flags & !(UI_FLAGS | 3 << BUSY_SHIFT | REVERSE | SPLIT | PRIORITY))
        | (ch.flags as u16 & UI_FLAGS)
        | if ch.flags & ferrite_app::chan::FLAG_PRIORITY != 0 { PRIORITY } else { 0 }
        | ((c.busy as u16 & 3) << BUSY_SHIFT)
        | if c.reverse { REVERSE } else { 0 }
        | if c.split { SPLIT } else { 0 };
    b[16..18].copy_from_slice(&flags.to_le_bytes());
    b[18] = ch.zone;
    b[19] = if c.squelch >= SQ_GLOBAL { 0 } else { c.squelch + 1 };
    b[32..48].copy_from_slice(&ch.name);
    b[60] = CH_VERSION;
    b[61] = 0xFF;
    let crc = crc16(&b[..62]);
    b[62..64].copy_from_slice(&crc.to_le_bytes());
    b
}

/// A channel's position, if its record has one (CHAN-2, CHAN-15).
pub fn ch_position(b: &[u8; CH_LEN]) -> Option<Pos> {
    let (lat, lon) = (u32_at(b, 20) as i32, u32_at(b, 24) as i32);
    (ch_valid(b) && u16_at(b, 16) & HAS_POS != 0 && lat != NO_POS).then_some(Pos { lat, lon })
}

/// A record with a position set (the web app's RepeaterBook import; the simulator's sample).
pub fn ch_with_position(b: &[u8; CH_LEN], p: Pos) -> [u8; CH_LEN] {
    let mut r = *b;
    r[20..24].copy_from_slice(&p.lat.to_le_bytes());
    r[24..28].copy_from_slice(&p.lon.to_le_bytes());
    let f = u16_at(&r, 16) | HAS_POS;
    r[16..18].copy_from_slice(&f.to_le_bytes());
    let crc = crc16(&r[..62]);
    r[62..64].copy_from_slice(&crc.to_le_bytes());
    r
}

// ---- zones ----

pub const ZONE_LEN: usize = 32;
/// Zones in one sector.
pub const ZONES_PER_SECTOR: u32 = 127;
/// Zones 0-253: two sectors of 127 (255 is "no zone").
pub const MAX_ZONES: u8 = 254;
const _: () = assert!(MAX_ZONES < NO_ZONE);

/// Zone `z`'s record: sector z / 127, slot z % 127.
pub fn zone_addr(z: u8) -> u32 {
    let z = z as u32;
    ZONES.start + z / ZONES_PER_SECTOR * SECTOR + z % ZONES_PER_SECTOR * ZONE_LEN as u32
}

/// Name (16) · flags: scan all, shown, by distance (1) · resume (1) · dwell (1) · smart (1) · priority channels
/// (2 + 2, 0xFFFF none) · reserved (6) · CRC-16 (2).
pub fn zone_decode(b: &[u8; ZONE_LEN]) -> Option<(Name, ZoneCfg)> {
    if crc16(&b[..30]) != u16_at(b, 30) || b[..30] == [0xFF; 30] {
        return None;
    }
    let n = b[..16].iter().position(|&c| c == 0).unwrap_or(16);
    let pri = |i: usize| Some(u16_at(b, i)).filter(|&v| v != 0xFFFF);
    let cfg = ZoneCfg {
        scan_all: b[16] & 1 != 0,
        shown: b[16] & 2 != 0,
        by_distance: b[16] & 4 != 0,
        resume: b[17],
        dwell: b[18],
        smart: b[19] != 0,
        pri: [pri(20), pri(22)],
    };
    Some((Name::from(&b[..n]), cfg))
}

pub fn zone_encode(name: &[u8], cfg: &ZoneCfg) -> [u8; ZONE_LEN] {
    let mut b = [0xFFu8; ZONE_LEN];
    b[..16].fill(0);
    let n = name.len().min(16);
    b[..n].copy_from_slice(&name[..n]);
    b[16] = cfg.scan_all as u8 | (cfg.shown as u8) << 1 | (cfg.by_distance as u8) << 2;
    b[17] = cfg.resume;
    b[18] = cfg.dwell;
    b[19] = cfg.smart as u8;
    b[20..22].copy_from_slice(&cfg.pri[0].unwrap_or(0xFFFF).to_le_bytes());
    b[22..24].copy_from_slice(&cfg.pri[1].unwrap_or(0xFFFF).to_le_bytes());
    let crc = crc16(&b[..30]);
    b[30..32].copy_from_slice(&crc.to_le_bytes());
    b
}

/// A new zone's settings: in "scan all", shown in ◀▶, the radio's resume and dwell.
pub const NEW_ZONE: ZoneCfg = ZoneCfg { scan_all: true, shown: true, by_distance: false, resume: 0, dwell: 255, smart: false, pri: [None, None] };

// ---- range presets ----

pub const PRESET_LEN: usize = 224;
pub const MAX_PRESETS: u8 = 16;

pub fn preset_addr(i: u8) -> u32 {
    PRESETS.start + i as u32 * PRESET_LEN as u32
}

/// Name (16) · start (4) · end (4) · step, mode, bandwidth, resume (1 each) · skips in use (1) · reserved (1) ·
/// CRC-16 of the 30 bytes before (2) · 48 skipped frequencies (192, kept as they are).
pub fn preset_decode(b: &[u8]) -> Option<RangePreset> {
    if crc16(&b[..30]) != u16_at(b, 30) || b[..30] == [0xFF; 30] {
        return None;
    }
    let n = b[..16].iter().position(|&c| c == 0).unwrap_or(16).min(12);
    let mut p = RangePreset {
        name: Buf::from(&b[..n]),
        start: u32_at(b, 16),
        end: u32_at(b, 20),
        step: b[24],
        mode: b[25],
        narrow: b[26] != 0,
        skips: b[28],
    };
    p.fold_bw(); // VFO-20
    Some(p)
}

/// The first 32 bytes of a preset record (the skip list after them is left as it is).
pub fn preset_encode(p: &RangePreset) -> [u8; 32] {
    let mut b = [0u8; 32];
    let name = p.name.as_bytes();
    b[..name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
    b[16..20].copy_from_slice(&p.start.to_le_bytes());
    b[20..24].copy_from_slice(&p.end.to_le_bytes());
    b[24] = p.step;
    b[25] = p.mode;
    b[26] = p.narrow as u8;
    b[27] = 0;
    b[28] = p.skips;
    b[29] = 0xFF;
    let crc = crc16(&b[..30]);
    b[30..32].copy_from_slice(&crc.to_le_bytes());
    b
}

// ---- DTMF memories ----

/// 16 records of 16 B after the range presets, in the same sector (0x041E00-0x041EFF).
pub const DTMF_LEN: usize = 16;

pub fn dtmf_addr(i: u8) -> u32 {
    PRESETS.start + MAX_PRESETS as u32 * PRESET_LEN as u32 + (i % DTMF_MEMS) as u32 * DTMF_LEN as u32
}

/// Length (1) · symbols, two to a byte (8) · reserved (5) · CRC-16 of the 14 bytes before (2).
pub fn dtmf_decode(b: &[u8]) -> Option<Dtmf> {
    if b[..14] == [0xFF; 14] || crc16(&b[..14]) != u16_at(b, 14) || b[0] > Dtmf::MAX {
        return None;
    }
    let mut d = [0u8; 8];
    d.copy_from_slice(&b[1..9]);
    Some(Dtmf { len: b[0], d })
}

pub fn dtmf_encode(m: &Dtmf) -> [u8; DTMF_LEN] {
    let mut b = [0xFFu8; DTMF_LEN];
    b[0] = m.len;
    b[1..9].copy_from_slice(&m.d);
    let crc = crc16(&b[..14]);
    b[14..16].copy_from_slice(&crc.to_le_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrite_app::chan::channel;
    use ferrite_app::slot::DCS_FLAG;

    #[test]
    fn channel_round_trip_keeps_unmodelled_fields() {
        let mut cfg = DEFAULT_CFG;
        cfg.tx_tone = DCS_FLAG | 0o023;
        cfg.offset = -60_000;
        cfg.power = 2;
        cfg.busy = 2;
        cfg.reverse = true;
        cfg.squelch = 0;
        let mut c = channel(11, 1, 14_694_000, "North hill", cfg);
        c.flags = ferrite_app::chan::FLAG_SKIP;
        let r = ch_encode(&c, &[0xFF; CH_LEN]);
        assert_eq!(ch_decode(11, &r), Some(c));
        // Split: TX is stored as itself and comes back the same (RX 446.000, TX 146.520).
        let mut sp = c;
        sp.rx = 44_600_000;
        sp.cfg.split = true;
        sp.cfg.offset = 14_652_000;
        let rs = ch_encode(&sp, &[0xFF; CH_LEN]);
        assert_eq!(u32_at(&rs, 4), 14_652_000);
        assert_eq!(ch_decode(11, &rs), Some(sp));
        // Global squelch is stored as 0 and reads back as Global (CHAN-14).
        let mut g = c;
        g.cfg.squelch = SQ_GLOBAL;
        let rg = ch_encode(&g, &[0xFF; CH_LEN]);
        assert_eq!(rg[19], 0);
        assert_eq!(ch_decode(11, &rg), Some(g));
        assert_eq!(ch_position(&r), None);
        let p = ch_with_position(&r, Pos { lat: 1, lon: -2 });
        assert_eq!(ch_position(&p), Some(Pos { lat: 1, lon: -2 }));
        // Rewritten (renamed): the position stays.
        c.set_name(b"Hill");
        let q = ch_encode(&c, &p);
        assert_eq!(ch_position(&q), Some(Pos { lat: 1, lon: -2 }));
        assert_eq!(ch_decode(11, &q), Some(c));
        // Torn: empty.
        let mut t = q;
        t[40] ^= 1;
        assert_eq!(ch_decode(11, &t), None);
        assert_eq!(ch_decode(0, &[0xFF; CH_LEN]), None);
    }

    /// Decision 63: Hidden (UI bit 6, record bit 6) and Priority (UI bit 7, record bit 8) survive a round trip;
    /// record bit 7 stays "has position".
    #[test]
    fn hidden_and_priority_round_trip() {
        use ferrite_app::chan::{FLAG_HIDDEN, FLAG_PRIORITY, FLAG_SKIP};
        let c = Channel { num: 3, zone: 0, rx: 14_652_000, name: [0; 16], cfg: DEFAULT_CFG, flags: FLAG_HIDDEN | FLAG_PRIORITY };
        let r = ch_encode(&c, &[0xFF; CH_LEN]);
        assert_eq!(u16_at(&r, 16) & (1 << 8), 1 << 8);
        assert_eq!(u16_at(&r, 16) & (1 << 7), 0, "a new record has no position");
        assert_eq!(ch_decode(3, &r).unwrap().flags, FLAG_HIDDEN | FLAG_PRIORITY);
        let c2 = Channel { flags: FLAG_SKIP, ..c };
        assert_eq!(ch_decode(3, &ch_encode(&c2, &r)).unwrap().flags, FLAG_SKIP, "cleared again");
    }

    #[test]
    fn zones_and_presets_round_trip() {
        let cfg = ZoneCfg { scan_all: true, shown: false, by_distance: true, resume: 11, dwell: 3, smart: true, pri: [Some(9), None] };
        let z = zone_encode(b"Repeaters", &cfg);
        assert!(zone_decode(&z) == Some((Name::from(b"Repeaters"), cfg)));
        assert!(zone_decode(&[0xFF; ZONE_LEN]).is_none());
        let p = RangePreset { name: Buf::from(b"Airband"), start: 11_800_000, end: 13_700_000, step: 10, mode: 2, narrow: false, skips: 3 };
        let mut rec = [0xFFu8; PRESET_LEN];
        rec[..32].copy_from_slice(&preset_encode(&p));
        assert!(preset_decode(&rec) == Some(p));
        assert!(preset_decode(&[0xFF; PRESET_LEN]).is_none());
        // DTMF memories sit after the presets, inside the sector, clear of its free tail.
        let m = Dtmf::parse(b"*79#").unwrap();
        assert_eq!(dtmf_decode(&dtmf_encode(&m)), Some(m));
        assert!(dtmf_decode(&[0xFF; DTMF_LEN]).is_none());
        assert!(dtmf_addr(DTMF_MEMS - 1) + DTMF_LEN as u32 <= PRESETS.end - 32);
    }
}
