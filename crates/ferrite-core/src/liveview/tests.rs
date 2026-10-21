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
card B active=0 head=\"012 · Repeaters\" state=\"idle\" freq=443.47500 name=\"Club repeater\" mode=\"FM-N\" tone=\"T 100.0\" shift=\"+5M\" power=\"LOW\" s=\"S0\" meter=1 rx=0 scan=0 kind=slot
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
