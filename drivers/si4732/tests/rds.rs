//! RDS groups: station name, radio text, A/B changes, block errors.

use si4732::rds::*;

fn chars(s: &[u8]) -> u16 {
    (s[0] as u16) << 8 | s[1] as u16
}

#[test]
fn groups() {
    let mut r = Rds::init();
    // PS "CBC 88.1" in four 0A groups, out of order
    let ps = b"CBC 88.1";
    for seg in [2usize, 0, 3, 1] {
        assert!(!r.ps_complete());
        r.feed(0xC201, seg as u16, 0xE0CD, chars(&ps[seg * 2..]), 0);
    }
    assert!(r.ps_complete() && &r.ps == b"CBC 88.1" && r.pi == 0xC201 && r.groups == 4);
    // uncorrectable block D: ignored; bad block B: whole group ignored
    r.feed(0xC201, 0x0000, 0, chars(b"XX"), 0x03);
    assert_eq!(&r.ps, b"CBC 88.1");
    r.feed(0xC201, 0x0000, 0, chars(b"XX"), 0x30);
    assert_eq!(r.groups, 5);

    // radio text 2A: "Hello RT" then CR; complete once every segment up to the end is in, in any order
    assert!(!r.rt_complete());
    r.feed(0xC201, 0x2002, chars(b"\r "), chars(b"  "), 0);
    assert!(!r.rt_complete());
    r.feed(0xC201, 0x2000, chars(b"He"), chars(b"ll"), 0);
    assert!(!r.rt_complete());
    r.feed(0xC201, 0x2001, chars(b"o "), chars(b"RT"), 0);
    assert!(r.rt_complete());
    assert_eq!(r.text(), b"Hello RT");
    // A/B flip clears it, and it is incomplete again until the new text's end is in
    r.feed(0xC201, 0x2011, chars(b"xt"), chars(b"\r "), 0);
    assert!(!r.rt_complete());
    r.feed(0xC201, 0x2010, chars(b"Ne"), chars(b"w "), 0);
    assert!(r.rt_complete());
    assert_eq!(r.text(), b"New xt");
    // unprintable PS characters become spaces
    r.feed(0xC201, 0x0000, 0, 0x01 << 8 | b'A' as u16, 0);
    assert!(r.ps[0] == b' ' && r.ps[1] == b'A');
}

/// A 64-character text with no end marker is complete once all 16 segments are in.
#[test]
fn full_text() {
    let mut r = Rds::init();
    let t = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ!?";
    for seg in 0..16usize {
        assert!(!r.rt_complete());
        let s = &t[seg * 4..];
        r.feed(0xC201, 0x2000 | seg as u16, chars(s), chars(&s[2..]), 0);
    }
    assert!(r.rt_complete());
    assert_eq!(r.text(), &t[..]);
}

/// 2B radio text: two characters per group, in block D; block C (the PI again in version B) is not text.
#[test]
fn version_b_text() {
    let mut r = Rds::init();
    r.feed(0xC201, 0x2801, 0xC201, chars(b"i!"), 0);
    r.feed(0xC201, 0x2802, 0xC201, chars(b"\r "), 0);
    assert!(!r.rt_complete());
    // An uncorrectable block D: ignored (counted as a group, nothing taken).
    r.feed(0xC201, 0x2800, 0xC201, chars(b"XX"), 0x03);
    assert!(!r.rt_complete() && r.groups == 3);
    r.feed(0xC201, 0x2800, 0xC201, chars(b"Hi"), 0);
    assert!(r.rt_complete());
    assert_eq!(r.text(), b"Hii!");
}

/// A bad block A keeps the last PI; a 2A group with a bad block C is dropped whole; control characters in radio
/// text become spaces; other group types only count.
#[test]
fn bad_blocks_and_control_characters() {
    let mut r = Rds::init();
    r.feed(0xC201, 0x0000, 0, chars(b"AB"), 0);
    r.feed(0x1234, 0x0001, 0, chars(b"CD"), 0xC0);
    assert_eq!((r.pi, &r.ps[..4]), (0xC201, &b"ABCD"[..]));
    r.feed(0xC201, 0x2000, chars(b"XX"), chars(b"XX"), 0x0C);
    assert!(r.rt_seen == 0 && r.text().is_empty());
    r.feed(0xC201, 0x2000, chars(b"A\x07"), chars(b"\x7FB"), 0);
    assert_eq!(r.text(), b"A  B");
    // 4A (clock time): counted, nothing else changes.
    let before = r.groups;
    r.feed(0xC201, 0x4000, 0x1234, 0x5678, 0);
    assert_eq!((r.groups, r.text()), (before + 1, &b"A  B"[..]));
    // The zeroed state (before any retune) has an empty name and no text.
    let z = Rds::zeroed();
    assert!(z.ps == [0; 8] && z.text().is_empty() && !z.ps_complete() && !z.rt_complete());
}

/// BCR-26.8: the programme type, bits 9-5 of block B in every group; a block B with two or more errors corrected
/// doesn't change it.
#[test]
fn programme_type() {
    let mut r = Rds::init();
    assert_eq!(r.pty, 0);
    r.feed(0xC201, 5 << 5, 0xE0CD, chars(b"CB"), 0); // 0A, PTY 5 (Rock in RBDS)
    assert_eq!(r.pty, 5);
    r.feed(0xC201, 0x2000 | 15 << 5, chars(b"He"), chars(b"ll"), 0); // 2A carries it too
    assert_eq!(r.pty, 15);
    r.feed(0xC201, 0x2000 | 9 << 5, chars(b"He"), chars(b"ll"), 0x20); // block B: 3-5 errors corrected
    assert_eq!(r.pty, 15);
    r.feed(0xC201, 0x2000 | 9 << 5, chars(b"He"), chars(b"ll"), 0x10); // 1-2 corrected: taken
    assert_eq!(r.pty, 9);
}
