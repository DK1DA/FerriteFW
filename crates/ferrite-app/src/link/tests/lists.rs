//! LINK-54 – LINK-56 (decision 327): the quick messages, waypoints and DTMF memories by `READ_MANY` and `WRITE_MANY`.

use super::*;
use crate::data::{Data, Dtmf, Pos, RangePreset, Waypoint, WpSrc};
use ferrite_core::link::{ADD, QUICK_OWN};

/// Every record of `w` by `READ_MANY` from 0: (index, record).
fn all(pc: &mut Pc, w: u8) -> Vec<(u16, Vec<u8>)> {
    let mut got = Vec::new();
    let mut first = 0u16;
    loop {
        let mut p = std::vec![w];
        p.extend_from_slice(&first.to_le_bytes());
        p.extend_from_slice(&1000u16.to_le_bytes());
        let (s, a) = pc.ask(msg::READ_MANY, &p);
        assert_eq!(s, 0);
        let next = u16::from_le_bytes([a[0], a[1]]);
        let mut r = &a[2..];
        if r.is_empty() {
            return got; // the range is done
        }
        while r.len() >= 3 {
            let (i, l) = (u16::from_le_bytes([r[0], r[1]]), r[2] as usize);
            got.push((i, r[3..3 + l].to_vec()));
            r = &r[3 + l..];
        }
        first = next;
    }
}

fn quick_rec(own: bool, t: &[u8]) -> Vec<u8> {
    let mut r = std::vec![if own { QUICK_OWN } else { 0 }];
    r.extend_from_slice(t);
    r
}

/// LINK-54, TEXT-7: all nine quick messages, the factory text flagged as such; one set (empty stays empty), one put
/// back to the factory text; a text APRS can't carry is refused with the reason, as is one over 67; an unchanged
/// one writes nothing; `USED` of them is refused.
#[test]
fn quick_messages_over_the_link() {
    let mut pc = Pc::new(true);
    let q = all(&mut pc, what::QUICK);
    assert_eq!(q.len(), crate::words::QUICK);
    for (i, r) in &q {
        assert_eq!(r, &quick_rec(false, crate::words::quick(*i as usize)), "quick {i}: the factory text");
    }
    assert_eq!(pc.write_many(what::QUICK, &[(2, quick_rec(true, b"At the club")), (4, quick_rec(true, b""))]), (0, std::vec![2]));
    assert_eq!(pc.st.quick(2), Some(Buf::from(b"At the club")));
    assert_eq!(pc.st.quick(4), Some(Buf::new()), "left empty, it stays empty");
    let q = all(&mut pc, what::QUICK);
    assert_eq!((&q[2].1, &q[4].1), (&quick_rec(true, b"At the club"), &quick_rec(true, b"")));
    // the factory text back (the text sent with it is ignored)
    assert_eq!(pc.write_many(what::QUICK, &[(2, quick_rec(false, b"whatever"))]).0, 0);
    assert_eq!(pc.st.quick(2), None);
    // refused: | ~ {, 68 characters, a tenth one; nothing changed
    let ver = pc.st.version();
    for t in [&b"a|b"[..], b"a~b", b"a{b", &[b'x'; 68]] {
        let (s, a) = pc.write_many(what::QUICK, &[(0, quick_rec(true, t))]);
        assert_eq!(s, Status::Invalid as u8, "{t:?}");
        assert!(std::str::from_utf8(&a[1..]).unwrap().contains("67 printable"));
    }
    assert_eq!(pc.write_many(what::QUICK, &[(9, quick_rec(true, b"x"))]).0, Status::NotFound as u8);
    assert_eq!(pc.write_many(what::QUICK, &[(0, Vec::new())]).0, Status::BadArg as u8);
    // unchanged: nothing written
    assert_eq!(pc.write_many(what::QUICK, &[(4, quick_rec(true, b""))]).0, 0);
    assert_eq!(pc.st.version(), ver, "nothing written");
    assert_eq!(pc.ask(msg::USED, &[what::QUICK]).0, Status::BadArg as u8);
}

fn wp_rec(name: &[u8], lat: i32, lon: i32, src: u8) -> Vec<u8> {
    let mut r = std::vec![0u8; 16];
    r[..name.len()].copy_from_slice(name);
    r.extend_from_slice(&lat.to_le_bytes());
    r.extend_from_slice(&lon.to_le_bytes());
    r.push(src);
    r
}

/// LINK-55, GPS-10, GPS-13: the waypoints newest first as the list shows them; one replaced in its place, one added as the
/// newest (index 0xFFFF), one deleted by its index; names, positions and sources checked with the reasons; a
/// record of another length malformed; an index past the list `not found`.
#[test]
fn waypoints_over_the_link() {
    let mut pc = Pc::new(true);
    let w = all(&mut pc, what::WAYPOINT);
    let n = pc.st.waypoints();
    assert!(n > 1);
    assert_eq!(w.len(), n as usize);
    for (i, r) in &w {
        let p = pc.st.waypoint(*i).unwrap();
        assert_eq!(r, &wp_rec(p.name.as_bytes(), p.pos.lat, p.pos.lon, p.src as u8), "waypoint {i}");
    }
    // replaced in its place
    assert_eq!(pc.write_many(what::WAYPOINT, &[(1, wp_rec(b"Summit", 455_000_000, -760_000_000, 2))]), (0, std::vec![1]));
    assert_eq!(pc.st.waypoint(1), Some(Waypoint { name: Buf::from(b"Summit"), pos: Pos { lat: 455_000_000, lon: -760_000_000 }, src: WpSrc::Link }));
    assert_eq!(pc.st.waypoints(), n);
    // added as the newest
    assert_eq!(pc.write_many(what::WAYPOINT, &[(ADD, wp_rec(b"Camp 2", 456_000_000, -761_000_000, 2))]).0, 0);
    assert_eq!((pc.st.waypoints(), pc.st.waypoint(0).map(|w| w.name)), (n + 1, Some(Buf::from(b"Camp 2"))));
    assert_eq!(pc.st.waypoint(2).map(|w| w.name), Some(Buf::from(b"Summit")), "the others one further on");
    // deleted
    let gone = pc.st.waypoint(3).unwrap();
    assert_eq!(pc.write_many(what::WAYPOINT, &[(3, Vec::new())]).0, 0);
    assert_eq!(pc.st.waypoints(), n);
    assert!((0..n).all(|i| pc.st.waypoint(i) != Some(gone)));
    // refused, with the reasons
    let bad = [
        (wp_rec(b"", 0, 0, 2), "name"),
        (wp_rec(b"Caf\xe9", 0, 0, 2), "name"),
        (wp_rec(b"North", 900_000_001, 0, 2), "latitude"),
        (wp_rec(b"East", 0, 1_800_000_001, 2), "latitude"),
        (wp_rec(b"Odd", 0, 0, 4), "source"),
    ];
    for (r, why) in bad {
        let (s, a) = pc.write_many(what::WAYPOINT, &[(0, r)]);
        assert_eq!(s, Status::Invalid as u8, "{why}");
        assert!(std::str::from_utf8(&a[1..]).unwrap().contains(why), "{a:?}");
    }
    assert_eq!(pc.write_many(what::WAYPOINT, &[(0, std::vec![1; 24])]).0, Status::BadArg as u8);
    assert_eq!(pc.write_many(what::WAYPOINT, &[(ADD, Vec::new())]).0, Status::BadArg as u8, "nothing to delete at ADD");
    assert_eq!(pc.write_many(what::WAYPOINT, &[(n, wp_rec(b"Far", 0, 0, 2))]).0, Status::NotFound as u8);
    assert_eq!(pc.st.waypoints(), n, "nothing changed");
}

fn dtmf_rec(sym: &[u8], name: &[u8]) -> Vec<u8> {
    let mut r = std::vec![sym.len() as u8];
    r.extend_from_slice(sym);
    r.extend_from_slice(name);
    r
}

/// LINK-56, DTMF-12: only the memories set are listed (symbols as text, then the name); one set with its name,
/// one renamed, one cleared with its name; bad symbols and names refused with the reasons; index 16 `not found`.
#[test]
fn dtmf_memories_over_the_link() {
    let mut pc = Pc::new(true);
    let before: Vec<u16> = (0..16).filter(|&i| pc.st.dtmf(i as u8).is_some()).collect();
    let d = all(&mut pc, what::DTMF);
    assert_eq!(d.iter().map(|x| x.0).collect::<Vec<_>>(), before);
    for (i, r) in &d {
        let mut t = Buf::<16>::new();
        pc.st.dtmf(*i as u8).unwrap().text(&mut t);
        assert_eq!(r, &dtmf_rec(t.as_bytes(), pc.st.dtmf_name(*i as u8).as_bytes()));
    }
    let free = (0..16u8).find(|&i| pc.st.dtmf(i).is_none()).unwrap();
    assert_eq!(pc.write_many(what::DTMF, &[(free as u16, dtmf_rec(b"*79#a", b"Link on"))]), (0, std::vec![1]));
    assert_eq!((pc.st.dtmf(free), pc.st.dtmf_name(free)), (Dtmf::parse(b"*79#A"), Buf::from(b"Link on")));
    // renamed only
    assert_eq!(pc.write_many(what::DTMF, &[(free as u16, dtmf_rec(b"*79#A", b"IRLP 3500"))]).0, 0);
    assert_eq!((pc.st.dtmf(free), pc.st.dtmf_name(free)), (Dtmf::parse(b"*79#A"), Buf::from(b"IRLP 3500")));
    let rec = all(&mut pc, what::DTMF).into_iter().find(|x| x.0 == free as u16).unwrap().1;
    assert_eq!(rec, dtmf_rec(b"*79#A", b"IRLP 3500"), "read back, the symbols in capitals");
    // refused
    for (r, why) in [
        (dtmf_rec(b"", b"x"), "symbols"),
        (dtmf_rec(b"12E", b""), "symbols"),
        (dtmf_rec(&[b'1'; 17], b""), "symbols"),
        (dtmf_rec(b"1", b"Eleven char"), "name"),
    ] {
        let (s, a) = pc.write_many(what::DTMF, &[(free as u16, r)]);
        assert_eq!(s, Status::Invalid as u8, "{why}");
        assert!(std::str::from_utf8(&a[1..]).unwrap().contains(why));
    }
    assert_eq!(pc.write_many(what::DTMF, &[(free as u16, std::vec![5, b'1'])]).0, Status::BadArg as u8);
    assert_eq!(pc.write_many(what::DTMF, &[(16, dtmf_rec(b"1", b""))]).0, Status::NotFound as u8);
    // cleared, its name with it
    assert_eq!(pc.write_many(what::DTMF, &[(free as u16, Vec::new())]).0, 0);
    assert_eq!((pc.st.dtmf(free), pc.st.dtmf_name(free)), (None, Buf::new()));
}

/// LINK-35.2: a radio says `LISTS` (128); `WRITE_MANY` and `READ_MANY` of quick messages, waypoints and DTMF memories
/// answer, `READ` and `WRITE` of them don't (they go whole, several to a frame).
#[test]
fn lists_go_by_many_only() {
    let mut pc = Pc::new(true);
    for w in [what::QUICK, what::WAYPOINT, what::DTMF] {
        assert_eq!(pc.ask(msg::READ, &[w, 0, 0, 0, 0, 100]).0, Status::BadArg as u8);
        assert_eq!(pc.write_parts(w, 0, &[1, 2], 200).0, Status::Refused as u8);
    }
}

fn skips_rec(f: &[u32]) -> Vec<u8> {
    f.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// LINK-58, SCAN-10 (decision 329): a range preset's skipped frequencies, listed for the presets that have any; a
/// list replaced whole (more than one store write's worth), read back, cleared; out of range, twice or 49 refused
/// with the reasons; the list stays with its preset when a preset before it is deleted (its index moves, its place
/// doesn't); `USED` of them refused.
#[test]
fn preset_skips_over_the_link() {
    let mut pc = Pc::new(true);
    let have: Vec<u16> = (0..pc.st.presets() as u16).filter(|&i| pc.st.preset(i as u8).unwrap().skips > 0).collect();
    let got = all(&mut pc, what::PRESET_SKIPS);
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), have);
    for (i, r) in &got {
        let n = pc.st.preset(*i as u8).unwrap().skips;
        assert_eq!(r, &skips_rec(&(0..n).map(|j| pc.st.preset_skip(*i as u8, j).unwrap()).collect::<Vec<_>>()));
    }
    let p = pc.st.preset(1).unwrap(); // 70 cm
    let list: Vec<u32> = (0..20).map(|j| p.start + 2_500 * (j + 1)).collect();
    assert_eq!(pc.write_many(what::PRESET_SKIPS, &[(1, skips_rec(&list))]), (0, std::vec![1]));
    assert_eq!(pc.st.preset(1).unwrap().skips, 20);
    assert_eq!((0..20).map(|j| pc.st.preset_skip(1, j).unwrap()).collect::<Vec<_>>(), list);
    assert_eq!(all(&mut pc, what::PRESET_SKIPS).into_iter().find(|g| g.0 == 1).unwrap().1, skips_rec(&list), "read back");
    assert!(pc.st.preset(1).map(|q| RangePreset { skips: p.skips, ..q }) == Some(p), "the preset itself as it was");
    // refused, with the reasons
    let ver = pc.st.version();
    for (r, why) in [
        (skips_rec(&[p.end + 1]), "within the range"),
        (skips_rec(&[p.start, p.start]), "once"),
        (skips_rec(&(0..49).map(|j| p.start + j).collect::<Vec<_>>()), "48"),
    ] {
        let (s, a) = pc.write_many(what::PRESET_SKIPS, &[(1, r)]);
        assert_eq!(s, Status::Invalid as u8, "{why}");
        assert!(std::str::from_utf8(&a[1..]).unwrap().contains(why), "{a:?}");
    }
    assert_eq!(pc.write_many(what::PRESET_SKIPS, &[(1, std::vec![1, 2, 3])]).0, Status::BadArg as u8);
    assert_eq!(pc.write_many(what::PRESET_SKIPS, &[(pc.st.presets() as u16, Vec::new())]).0, Status::NotFound as u8);
    assert_eq!(pc.write_many(what::PRESET_SKIPS, &[(1, skips_rec(&list))]).0, 0);
    assert_eq!(pc.st.version(), ver, "as stored: nothing written");
    // the preset before it deleted: the list goes with its preset, now index 0
    assert_eq!(pc.ask(msg::DELETE, &[what::PRESET, 0, 0]).0, 0);
    assert_eq!(pc.st.preset(0).map(|q| q.name), Some(p.name));
    assert_eq!((0..20).map(|j| pc.st.preset_skip(0, j).unwrap()).collect::<Vec<_>>(), list, "with its preset");
    // cleared
    assert_eq!(pc.write_many(what::PRESET_SKIPS, &[(0, Vec::new())]).0, 0);
    assert_eq!((pc.st.preset(0).unwrap().skips, pc.st.preset_skip(0, 0)), (0, None));
    assert_eq!(pc.ask(msg::USED, &[what::PRESET_SKIPS]).0, Status::BadArg as u8);
}
