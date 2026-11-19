use super::*;
use std::string::String;
use std::vec::Vec;

/// A client's lines.
#[derive(Default)]
struct Pc(Vec<Vec<u8>>, Option<u8>);

impl Lines for Pc {
    fn count(&self) -> usize {
        self.0.len()
    }
    fn set_count(&mut self, n: usize) {
        self.0.resize(n, Vec::new());
    }
    fn line(&self, i: usize) -> &[u8] {
        &self.0[i]
    }
    fn set_line(&mut self, i: usize, t: &[u8]) {
        self.0[i] = t.to_vec();
    }
}

impl Pc {
    fn take(&mut self, ev: &[u8]) -> Result<u8, Resync> {
        let r = apply(self, ev, self.1);
        if let Ok(s) = r {
            self.1 = Some(s);
        }
        r
    }
    fn text(&self) -> String {
        self.0.iter().map(|l| String::from_utf8_lossy(l).into_owned() + "\n").collect()
    }
}

/// The radio's side: `screen` as readback lines; the event, if any.
fn ev(t: &mut Track, screen: &str, room: usize) -> Option<Vec<u8>> {
    let lines: Vec<&str> = screen.lines().collect();
    let mut out = std::vec![0u8; room];
    let n = t.update(
        |i, b| {
            let l = lines.get(i as usize)?.as_bytes();
            let l = &l[..l.len().min(255)]; // as the readback's line buffer
            b[..l.len()].copy_from_slice(l);
            Some(l.len())
        },
        &mut out,
    )?;
    out.truncate(n);
    Some(out)
}

/// Events until the client has all of `screen`: how many it took and their bytes.
fn sync(t: &mut Track, pc: &mut Pc, screen: &str) -> (usize, usize) {
    let (mut n, mut bytes) = (0, 0);
    while let Some(d) = ev(t, screen, 240) {
        pc.take(&d).unwrap();
        (n, bytes) = (n + 1, bytes + d.len());
        if d[1] & MORE == 0 {
            break;
        }
    }
    assert_eq!(pc.text(), compacted(screen));
    (n, bytes)
}

/// The compact form of a whole screen, as the client should end up with it.
fn compacted(screen: &str) -> String {
    screen
        .lines()
        .take(LINES)
        .map(|l| {
            let mut b = [0u8; 256];
            let n = compact(&l.as_bytes()[..l.len().min(255)], &mut b);
            String::from_utf8_lossy(&b[..n]).into_owned() + "\n"
        })
        .collect()
}

const HOME: &str = "screen Home \"Home\" title=\"\"
state frame=17 ago=40 dark=0 locked=0 active=A keyed=- remote=0 mode=off fm=0
bar lock=0 scan=0 watch=3 inbox=3 alert=0 gps=fix bt_on=0 bt=0 battery=82 low=0 cutoff=0 dry=0 clock=19:06 utc=1 call=\"\"
card A active=1 head=\"VFO · VHF\" state=\"idle\" freq=146.52000 name=\"\" mode=\"FM\" tone=\"T 100.0\" shift=\"-600k\" power=\"LOW\" s=\"S1\" meter=1 rx=0 scan=0 kind=slot
card B active=0 head=\"012 · Repeaters\" state=\"idle\" freq=443.47500 name=\"Club repeater\" mode=\"NFM\" tone=\"T 100.0\" shift=\"+5M\" power=\"LOW\" s=\"S0\" meter=1 rx=0 scan=0 kind=slot
card C active=0 head=\"SCAN · Airband\" state=\"scanning\" freq=119.35000 name=\"\" mode=\"AM\" tone=\"\" shift=\"\" power=\"No TX\" s=\"S0\" meter=1 rx=0 scan=1 kind=slot
hints \"SLOT\" \"A·B·C\" \"OK\" \"Quick\" \"<>\" \"Band\"
";

const QUICK: &str = "screen Quick \"Quick\" title=\"Quick\"
state frame=27 ago=380 dark=0 locked=0 active=A keyed=- remote=0 mode=off fm=0
bar lock=0 scan=0 watch=0 inbox=3 alert=0 gps=fix bt_on=0 bt=0 battery=82 low=0 cutoff=0 dry=0 clock=19:06 utc=1 call=\"\"
row 1 \"Squelch\" \"4\" sel
row 2 \"TX power\" \"Low\"
row 3 \"Mode\" \"FM\" dot
row 4 \"Tones\" \"T 100.0\" dot
row 5 \"Offset\" \"-600 kHz\" dot
row 6 \"Step\" \"5 kHz\" dot
row 7 \"Watch\" \"A+B+C\" dot
row 8 \"Channel\" \"Save as channel\"
row 9 \"Battery Save\" \"Level 1\"
row 0 \"Advanced settings\" \">\"
hints \"^v\" \"Move\" \"Knob\" \"Change\" \"BACK\" \"Back\"
";

/// LINK-47: the compact form leaves out what moves on every redraw (`frame=`, `ago=`), keeps everything else, and
/// splits tokens at spaces outside quotes.
#[test]
fn compact_form() {
    let mut b = [0u8; 256];
    let n = compact(b"state frame=17 ago=40 dark=0 locked=0", &mut b);
    assert_eq!(&b[..n], b"state dark=0 locked=0");
    let t: Vec<&[u8]> = tokens(b"row 1 \"Tones \\\"x\\\" y\" \"T 100.0\" sel").collect();
    assert_eq!(t, [&b"row"[..], b"1", b"\"Tones \\\"x\\\" y\"", b"\"T 100.0\"", b"sel"]);
    assert_eq!(key(b"meter=15"), Some(&b"meter="[..]));
    assert_eq!(key(b"\"a=b\""), None);
    // the dictionary codes what it can and decodes back exactly
    let line = HOME.lines().nth(3).unwrap().as_bytes();
    let mut e = [0u8; 256];
    let k = encode(line, &mut e).unwrap();
    assert!(k < line.len() * 2 / 3, "{k} of {}", line.len());
    let mut d = [0u8; 512];
    let dn = decode(&e[..k], &mut d);
    assert_eq!(&d[..dn], line);
}

/// LINK-47, LINK-48: the whole state once, then only what changed: an S-meter and the clock moving on an idle home
/// screen are a few bytes; the client's lines always match the radio's; nothing changed sends nothing.
#[test]
fn deltas_are_small_and_exact() {
    let (mut t, mut pc) = (Track::new(), Pc::default());
    let (n, bytes) = sync(&mut t, &mut pc, HOME);
    assert!(n <= 3 && bytes < 600, "{n} events, {bytes} bytes");
    assert!(ev(&mut t, HOME, 240).is_none(), "nothing changed");
    // the frame counter alone moved: nothing
    let redraw = HOME.replace("frame=17 ago=40", "frame=18 ago=250");
    assert!(ev(&mut t, &redraw, 240).is_none());
    let moved = HOME.replace("s=\"S1\" meter=1", "s=\"S3\" meter=5").replace("clock=19:06", "clock=19:07");
    let d = ev(&mut t, &moved, 240).unwrap();
    assert!(d.len() <= 25, "{} bytes: {d:?}", d.len());
    pc.take(&d).unwrap();
    assert_eq!(pc.text(), compacted(&moved));
    // to another screen and back
    sync(&mut t, &mut pc, QUICK);
    sync(&mut t, &mut pc, &moved);
    sync(&mut t, &mut pc, QUICK);
    // the selection moving down a row: two small token updates
    let down = QUICK.replace("\" sel\nrow 2 \"TX power\" \"Low\"", "\"\nrow 2 \"TX power\" \"Low\" sel");
    let d = ev(&mut t, &down, 240).unwrap();
    assert!(d.len() < 20, "{d:?}");
    pc.take(&d).unwrap();
    assert_eq!(pc.text(), compacted(&down));
}

/// LINK-48: a lost update is noticed by its sequence number (the client asks again and gets the whole state); an
/// update cut to the room left (rate cap) says more follows and carries the rest next.
#[test]
fn loss_and_cuts() {
    let (mut t, mut pc) = (Track::new(), Pc::default());
    sync(&mut t, &mut pc, HOME);
    let a = HOME.replace("meter=1 rx=0 scan=1", "meter=9 rx=1 scan=1");
    let _lost = ev(&mut t, &a, 240).unwrap();
    let b = a.replace("clock=19:06", "clock=19:08");
    assert_eq!(pc.take(&ev(&mut t, &b, 240).unwrap()), Err(Resync::Seq));
    t.restart();
    let f = ev(&mut t, &b, 240).unwrap();
    assert_eq!(f[1] & FULL, FULL);
    pc.take(&f).unwrap();
    sync(&mut t, &mut pc, &b);
    // the menu arrives in several events, only the last one checked
    let (n, _) = sync(&mut t, &mut pc, QUICK);
    assert!(n >= 2, "{n}");
}

/// LINK-47: a line too long for one event is cut after its last whole token that fits; past [`TEXT`] bytes of
/// kept text a line is kept by hash and still sent exactly when it changes.
#[test]
fn long_lines_and_full_text() {
    let long: String = (0..60).map(|i| std::format!("k{i}=\"value {i}\" ")).collect();
    let mut screen = String::new();
    for i in 0..12 {
        screen += &std::format!("row {i} {long}\n");
    }
    let (mut t, mut pc) = (Track::new(), Pc::default());
    sync(&mut t, &mut pc, &screen);
    assert!(t.used as usize <= TEXT);
    assert!(t.rec.iter().any(|r| r.len == NOT_KEPT), "some lines by hash alone");
    sync(&mut t, &mut pc, &screen.replacen("value 3\"", "value 33\"", 12));
    sync(&mut t, &mut pc, &screen.replacen("value 3\"", "v\"", 12));
}

/// LINK-47: a control byte goes as `?`; coding into too little room fails, decoding into it is cut.
#[test]
fn coding_edges() {
    let mut e = [0u8; 8];
    assert_eq!(encode(b"a\tb", &mut e), Some(3));
    assert_eq!(&e[..3], b"a?b");
    assert_eq!(encode(b"row card ", &mut [0u8; 1]), None);
    let mut d = [0u8; 3];
    assert_eq!(decode(&[1, b'x'], &mut d), 3);
    assert_eq!(&d, b"row");
}

/// The sequence number the next event carries; an output without room for a whole line gets nothing (and the
/// state still goes whole with the next event that has room).
#[test]
fn next_seq_and_no_room() {
    let mut t = Track::default();
    assert_eq!(t.next_seq(), 1);
    assert!(ev(&mut t, HOME, HEAD + 2 + LINE_ROOM - 1).is_none());
    let d = ev(&mut t, HOME, 240).unwrap();
    assert_eq!((d[0], d[1] & FULL), (1, FULL));
    assert_eq!(t.next_seq(), 2);
}

/// A TOKENS op is given up (the line goes whole) past 128 tokens, with a change past token 127, or when it
/// doesn't fit its 256 bytes.
#[test]
fn tokens_op_gives_up() {
    let mut op = [0u8; 256];
    let many: Vec<u8> = (0..130).flat_map(|_| *b"a ").collect();
    let mut changed = many.clone();
    changed[0] = b'b';
    assert_eq!(tokens_op(0, &many, &changed, &mut op), None, "130 tokens");
    let mut late = many.clone();
    late[2 * 129] = b'b';
    assert_eq!(tokens_op(0, &many, &late, &mut op), None, "token 129 changed");
    let old: Vec<u8> = (0..20).flat_map(|_| *b"x ").collect();
    let new: Vec<u8> = (0..20).flat_map(|i| std::format!("{:z<20} ", i).into_bytes()).collect();
    assert_eq!(tokens_op(0, &old, &new, &mut op), None, "20 changes of 20 bytes");
    assert!(tokens_op(0, b"a b", b"a c", &mut op).is_some());
}

/// LINK-48: an event the client can't read asks for the whole state again: too short, a line past the count,
/// an op cut short, an entry past the line's tokens, an unknown op; one that reads but doesn't match the check.
#[test]
fn apply_refuses_bad_events() {
    let full = |ops: &[u8]| [&[1, FULL, 0, 0, 1][..], ops].concat();
    for (ops, why) in [
        (&[OP_LINE << 6 | 1][..], "line past the count"),
        (&[OP_LINE << 6], "no length"),
        (&[OP_LINE << 6, 5, b'a'], "text cut short"),
        (&[OP_TOKENS << 6], "no token count"),
        (&[OP_TOKENS << 6, 2], "no entry count"),
        (&[OP_TOKENS << 6, 2, 1], "no entry"),
        (&[OP_TOKENS << 6, 2, 1, 1, 5, b'x'], "entry text cut short"),
        (&[OP_TOKENS << 6, 2, 1, 5, 1, b'x'], "entry past the tokens"),
        (&[3 << 6], "no such op"),
    ] {
        assert_eq!(Pc::default().take(&full(ops)), Err(Resync::Bad), "{why}");
    }
    assert_eq!(Pc::default().take(&[1, FULL, 0, 0]), Err(Resync::Bad), "header cut short");
    assert_eq!(Pc::default().take(&full(&[OP_LINE << 6, 1, b'x'])), Err(Resync::Check));
    let mut pc = Pc::default();
    let mut ev = full(&[OP_LINE << 6, 1, b'x']);
    ev[1] |= MORE;
    assert_eq!(pc.take(&ev), Ok(1), "more to come: not checked yet");
    assert_eq!(pc.0, [b"x".to_vec()]);
    // A token added to an empty line, by index.
    let mut pc = Pc::default();
    assert_eq!(pc.take(&[1, FULL | MORE, 0, 0, 1, OP_TOKENS << 6, 2, 1, 1, 1, b'y']), Ok(1));
    assert_eq!(pc.0, [b" y".to_vec()]);
}
