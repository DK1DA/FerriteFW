//! TERM-11, TERM-16, TERM-47: every page's bytes on the wire for a script of keys through all of them (plain and in
//! colour, 80 x 24 and larger), kept in `term/golden/pages.txt`: the pages drawn from templates (decision 342) send
//! what the hand-written pages sent. `UPDATE_GOLDEN=1` writes the file again (look at the diff).

use super::*;

/// TERM-47: the pages drawn from the template pack send the bytes the hand-written pages sent.
#[test]
fn pages_golden() {
    const M: &[u8] = b"\x1b\x1b\x1b\x1b\x1b\r"; // back to the main menu from anywhere
    const STEPS: &[&[u8]] = &[
        b"\r", b"?", b"\x1b", b"\r", b"\x1b[B", b"\x1b[B", b"\x1b[A", b"?", b"\x1b",
        // channels: moves, sort, marks, find, zones
        b"2", b"\x1b[B", b"\x1b[B", b"\x1b[6~", b"\x1b[F", b"\x1b[H", b"s", b"\x1b[B", b"s", b" ", b" ", b"a", b"u", b"/", b"Club", b"\r",
        b"/", b"zzzz", b"\r", b"z", b"z", b"Z", b"s", b"s", b"s", b"s", b"s", b"s", b"\x1b[B", b"\x1b[6~", b"\x1b[5~", b"\x1b[F", b"\x1b[H",
        b" ", b" ", b"m", b"9", b"\r", b"m", b"1", b"\r", b"d", b"n", b"c", b"\x1b", b"r", b"\x1b", b"u",
        // the editor
        b"\r", b"\x1b[B", b"\x1b[B", b"\x1b[C", b"\x1b[D", b"#", b"\x1b[B", b"\x1b[B", b"\x1b[B", b"\x1b[C", b"\x1b[F", b"\x1b[H", b"\r", b"X", b"\x7f", b"\r",
        b"\x1b[B", b"\x1b[B", b"\r", b"146.52", b"\r", b"\x1b[B", b"\x1b[B", b"\x1b[B", b"\x1b[B", b"\x1b[C", b"\r", b"147.1", b"\r", b"?", b"\x1b", M,
        // zones
        b"3", b"\x1b[B", b"\x1b[B", b"\x1b[F", b"\x1b[H", b"\x1b[A", b"\r", b"\x1b", b"\x1b[B", b"\r", b"\x1b", M,
        // settings
        b"4", b"\x1b[B", b"\x1b[F", b"\x1b[H", b"7", b"\x1b[B", b"\x1b[C", b"\x1b[D", b"#", b"1", b"\x1b", b"\x1b", b"9", b"3", b"\x1b[C", b"\x1b", b"\x1b",
        b"8", b"8", b"\x1b[B", b"\x1b[B", b"\x1b[B", b"\x1b[B", b"\x1b[C", b"?", b"\x1b", M,
        // the others, and the command line over a page
        b"7", b"?", b"\x1b", M, b"8", b"?", b"\x1b", M,
        // the radio, APRS and GPS (TERM-48 to TERM-50)
        b"1", b"\x1b[B", b"\x1b[C", b"\x1b[D", b"#", b"b", b"a", b"?", b"\x1b", M, b"5", b"\x1b[B", b"\x1b[C", b"\x1b[B", b"?", b"\x1b", M,
        b"6", b"?", b"\x1b", M, b":slot\r", b"\r", b":frob\r", b"\x1b[B", b"\x03", b"\r",
    ];
    let mut out = String::new();
    for (colour, size) in [(false, (80, 24)), (true, (80, 24)), (false, (132, 40))] {
        let mut r = Rig::new();
        r.app.set.term_colour = colour;
        r.term.set_size(size.0, size.1);
        let mut seen = 0;
        for s in STEPS {
            r.send(s);
            r.now += 100;
            r.poll();
            let raw = &r.vt.raw[seen..];
            seen = r.vt.raw.len();
            out.push_str(&std::format!("> {:?}\n", String::from_utf8_lossy(s)));
            out.push_str(&std::format!("  {}\n", raw.escape_ascii()));
        }
        // a second's refresh sends only what changed
        r.now += 1000;
        r.poll();
        out.push_str(&std::format!("> refresh\n  {}\n", r.vt.raw[seen..].escape_ascii()));
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/term/golden/pages.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &out).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("the golden file (UPDATE_GOLDEN=1 writes it)");
    if want != out {
        let at = want.lines().zip(out.lines()).position(|(a, b)| a != b).unwrap_or(0);
        panic!("line {}:\nwant {:?}\n got {:?}", at + 1, want.lines().nth(at), out.lines().nth(at));
    }
}
