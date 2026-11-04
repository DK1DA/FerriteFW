//! The channel list's sorted views, marks, prompts and changes to several channels at their edges (TERM-14).

use super::*;
use core::cmp::Ordering;

fn ch(num: u16, rx: Freq10) -> Channel {
    Channel { num, zone: 0, rx, name: [0; 16], cfg: DEFAULT_CFG, flags: 0, tones: 0 }
}

/// TERM-14: each sort column's order (name any case, RX, TX, mode, tone, power, flags), the number breaking
/// ties; a column past the last orders by number alone.
#[test]
fn sort_orders_by_each_column() {
    let mut a = ch(1, mhz(146));
    let mut b = ch(2, mhz(145));
    a.set_name(b"beta");
    b.set_name(b"Alpha");
    assert_eq!(tpl::order(1, &a, &b), Ordering::Greater, "name, any case");
    assert_eq!(tpl::order(2, &a, &b), Ordering::Greater, "RX");
    b.cfg.offset = 2_000_000;
    assert_eq!(tpl::order(3, &a, &b), Ordering::Less, "TX: b's is 147");
    b.cfg.mode = 2;
    assert_eq!(tpl::order(4, &a, &b), Ordering::Less, "mode");
    a.cfg.tx_tone = 1000;
    assert_eq!(tpl::order(5, &a, &b), Ordering::Greater, "tone");
    b.cfg.power = 2;
    assert_eq!(tpl::order(6, &a, &b), Ordering::Less, "power");
    a.flags = crate::chan::FLAG_SKIP;
    assert_eq!(tpl::order(7, &a, &b), Ordering::Greater, "flags");
    assert_eq!(tpl::order(99, &a, &b), Ordering::Less, "no column: the number");
    assert_eq!(tpl::order(2, &a, &a), Ordering::Equal);
}

/// TERM-14: in a sorted list longer than a page, the arrows go through every channel in order and the page
/// follows; PgDn and PgUp move a page; End and Home reach the ends.
#[test]
fn sorted_list_scrolls() {
    let mut r = Rig::list();
    r.send(b"s"); // by name
    let mut names: Vec<Vec<u8>> = r.nums().iter().map(|&n| r.store.get(n).unwrap().name().to_ascii_lowercase()).collect();
    names.sort();
    let sel = |r: &Rig| r.term.selected(&r.store).unwrap().name().to_ascii_lowercase();
    let n = names.len();
    assert!(n as u16 > r.term.page_rows(), "longer than a page");
    for want in names.iter().skip(1) {
        r.send(b"\x1b[B");
        assert_eq!(&sel(&r), want);
    }
    r.send(b"\x1b[B");
    assert_eq!(sel(&r), names[n - 1], "the last stays");
    for want in names.iter().rev().skip(1) {
        r.send(b"\x1b[A");
        assert_eq!(&sel(&r), want);
    }
    r.send(b"\x1b[A");
    assert_eq!(sel(&r), names[0], "the first stays");
    r.send(b"\x1b[6~");
    let pg = r.term.page_rows() as usize;
    assert_eq!(sel(&r), names[pg.min(n - 1)], "PgDn: the page after");
    r.send(b"\x1b[6~");
    assert_eq!(sel(&r), names[n - 1], "PgDn at the end: the last row");
    r.send(b"\x1b[5~");
    r.send(b"\x1b[5~");
    r.send(b"\x1b[5~");
    assert_eq!(sel(&r), names[0], "PgUp at the top: the first row");
    r.send(b"\x1b[F");
    assert_eq!(sel(&r), names[n - 1]);
    r.send(b"\x1b[H");
    assert_eq!(sel(&r), names[0]);
}

/// TERM-14: a sorted list of one channel: Down stays on it.
#[test]
fn sorted_list_of_one() {
    let mut r = Rig::main();
    let keep = r.store.get(0).unwrap();
    r.store.apply(ChanWrite::EraseAll);
    r.store.apply(ChanWrite::Put { ch: keep, from: None });
    r.send(b"1s");
    r.send(b"\x1b[B\x1b[B");
    assert_eq!(r.term.selected(&r.store).unwrap().num, keep.num);
    r.send(b"\x1b[A");
    assert_eq!(r.term.selected(&r.store).unwrap().num, keep.num);
}

/// TERM-14: A marks every channel in the list, U clears the marks, Space on a marked one unmarks it.
#[test]
fn mark_all_unmark() {
    let mut r = Rig::list();
    let n = r.nums().len() as u16;
    r.send(b"a");
    assert_eq!(r.term.marks_n(), n);
    r.send(b"u");
    assert_eq!(r.term.marks_n(), 0);
    let first = r.nums()[0];
    r.send(b" ");
    assert!(r.term.marked(first));
    r.send(b"\x1b[A ");
    assert!(!r.term.marked(first), "Space again unmarks");
}

/// TERM-14: the prompts name what they apply to (the selected channel, or the marked ones), and refuse answers
/// that aren't a zone or a channel number.
#[test]
fn prompts_and_bad_answers() {
    let mut r = Rig::list();
    r.send(b"m");
    assert!(r.vt.has("Move channel 1 to zone (number, name, or none): "), "{}", r.vt.text());
    r.send(b"nowhere\r");
    assert!(r.bottom().contains("No such zone: a number, a name, or none"), "{}", r.bottom());
    r.send(b"  ");
    r.send(b"c");
    assert!(r.vt.has("Copy 2 marked channels to free numbers from (Enter: first free): "), "{}", r.vt.text());
    r.send(b"x\r");
    assert!(r.bottom().contains("Copy to: a channel number from 1 to 4000"), "{}", r.bottom());
    r.send(b"r");
    assert!(r.vt.has("Renumber 2 marked channels from number: "), "{}", r.vt.text());
    r.send(b"0\r");
    assert!(r.bottom().contains("Renumber from: a channel number from 1 to 4000"), "{}", r.bottom());
    r.send(b"r4000\r");
    assert!(r.bottom().contains("2 channels from 4000 would pass 4000"), "{}", r.bottom());
    r.send(b"m2\r");
    r.settle();
    assert!(r.bottom().contains("2 channels moved") || r.store.get(r.nums()[0]).unwrap().zone == 1, "{}", r.bottom());
}

/// TERM-14: find with nothing typed and nothing found before asks what to find.
#[test]
fn find_needs_text() {
    let mut r = Rig::list();
    r.send(b"/\r");
    assert!(r.bottom().contains("Find what? Part of a name or MHz"), "{}", r.bottom());
}

/// TERM-14: find in a sorted list goes in the list's order from the row after the selection, wrapping; the
/// selected one matching alone stays; no match says so.
#[test]
fn find_in_a_sorted_list() {
    let mut r = Rig::list();
    r.send(b"s"); // by name
    let names: Vec<String> = {
        let mut v: Vec<Vec<u8>> = r.nums().iter().map(|&n| r.store.get(n).unwrap().name().to_vec()).collect();
        v.sort_by_key(|n| n.to_ascii_lowercase());
        v.iter().map(|n| String::from_utf8_lossy(n).into_owned()).collect()
    };
    let last = names.last().unwrap().clone();
    r.send(b"/");
    r.send(last.as_bytes());
    r.send(b"\r");
    assert_eq!(r.term.selected(&r.store).unwrap().name(), last.as_bytes());
    // again: wraps round to the same one, the only match
    r.send(b"/\r");
    assert_eq!(r.term.selected(&r.store).unwrap().name(), last.as_bytes());
    r.send(b"/zzqq\r");
    assert!(r.bottom().contains("No channel here matches \"zzqq\""), "{}", r.bottom());
    // the first name, found from the last row (wrapping)
    r.send(b"/");
    r.send(names[0].as_bytes());
    r.send(b"\r");
    assert_eq!(r.term.selected(&r.store).unwrap().name(), names[0].as_bytes());
}

/// TERM-14: Esc while several channels change stops it; what was done stays done.
#[test]
fn esc_stops_a_bulk_change() {
    let mut r = Rig::list();
    let before = r.nums().len();
    r.send(b"ad");
    r.term.feed(&mut r.app, b'y', r.now, &r.store, &r.dev, &r.live);
    assert!(r.term.bulk_on());
    r.poll(); // one write
    r.term.feed(&mut r.app, 0x1b, r.now, &r.store, &r.dev, &r.live);
    r.now += 1000;
    r.poll(); // the lone ESC is Esc
    assert!(!r.term.bulk_on());
    assert!(r.bottom().contains("Stopped: what was done stays done"), "{}", r.bottom());
    let after = r.nums().len();
    assert!(after < before && after > 0, "{before} -> {after}");
}

/// A full radio: 1000 channels.
fn full() -> Rig<Blank> {
    let b = Blank::default();
    for n in 0..crate::chan::MAX_CHANNELS {
        b.apply(ChanWrite::Put { ch: ch(n, mhz(146) + n as u32 * 1250), from: None });
    }
    let mut r = Rig::with(b);
    r.send(b"\r\r2");
    r
}

/// TERM-14: with all 1000 channels used, N says so and a copy stops at the first it can't place.
#[test]
fn a_full_radio() {
    let mut r = full();
    r.send(b"n");
    assert!(r.bottom().contains("All 1000 channels are in use"), "{}", r.bottom());
    assert_eq!(r.term.page(), Page::Channels);
    r.send(b"c\r");
    for _ in 0..5 {
        r.poll();
    }
    assert!(!r.term.bulk_on());
    assert!(r.bottom().contains("No free numbers left: copied 0 of 1"), "{}", r.bottom());
}

/// CHAN-22: N on a slot whose TX wouldn't fit a channel (a split TX outside every band) is refused with the
/// reason.
#[test]
fn new_channel_refused() {
    let mut r = Rig::list();
    let mut cfg = *r.app.slot(r.app.active()).cfg();
    cfg.split = true;
    cfg.offset = 10_000; // 0.1 MHz
    r.app.set_vfo_cfg(r.app.active(), cfg);
    let n = r.store.count();
    r.send(b"n");
    assert_eq!(r.store.count(), n, "nothing made");
    assert_eq!(r.term.page(), Page::Channels);
    assert!(!r.bottom().is_empty());
}

/// TERM-17: the radio's channel open in an editor opened from its channel editor (a value, tones, text) counts
/// as open; a third-level menu row is the setting the radio has highlighted.
#[test]
fn radio_editor_and_setting_paths() {
    use crate::screens::nav::Dest;
    let mut r = Rig::list();
    let c = r.store.get(9).unwrap();
    r.app.set_channel_rec(0, c);
    assert_eq!(radio_channel(&r.app), None, "home: no editor open");
    r.app.open(Dest::ChannelEdit);
    assert_eq!(radio_channel(&r.app), Some(9));
    r.app.open(Dest::Value(crate::slot::Item::Squelch));
    assert_eq!(radio_channel(&r.app), Some(9), "a value opened from the channel editor");
    r.app.open(Dest::Sub(7, 3));
    let (p, n) = radio_setting(&r.app).expect("a third-level list");
    assert_eq!((&p[..n], n), (&[7, 3, 1][..], 3));
}
