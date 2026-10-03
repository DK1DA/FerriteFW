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

    // radio text 2A: "Hello RT" then CR
    r.feed(0xC201, 0x2000, chars(b"He"), chars(b"ll"), 0);
    r.feed(0xC201, 0x2001, chars(b"o "), chars(b"RT"), 0);
    r.feed(0xC201, 0x2002, chars(b"\r "), chars(b"  "), 0);
    assert_eq!(r.text(), b"Hello RT");
    // A/B flip clears it
    r.feed(0xC201, 0x2010, chars(b"Ne"), chars(b"w\r"), 0);
    assert_eq!(r.text(), b"New");
    // unprintable PS characters become spaces
    r.feed(0xC201, 0x0000, 0, 0x01 << 8 | b'A' as u16, 0);
    assert!(r.ps[0] == b' ' && r.ps[1] == b'A');
}
