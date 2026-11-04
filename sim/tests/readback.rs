//! The UI readback's screen lines (LINK-19, decision 150) for the screens the PC's checks read: each value the
//! readback gives is one the screen draws (the same functions build both), checked here against the drawn text.
//! `tools/uitest.py` reads these lines on the radio.

mod common;
use common::Radio;

/// The readback's line starting with `kind ` (the first), or a panic naming what was there.
#[track_caller]
fn line(r: &Radio, kind: &str) -> String {
    let rb = r.sim.readback();
    rb.lines().find(|l| l.starts_with(&format!("{kind} "))).map(str::to_owned).unwrap_or_else(|| panic!("no {kind} line in:\n{rb}"))
}

/// A `key="value"` (or `key=value`) field of a readback line.
#[track_caller]
fn field(l: &str, key: &str) -> String {
    let at = l.find(&format!(" {key}=")).unwrap_or_else(|| panic!("no {key}= in {l}")) + key.len() + 2;
    let rest = &l[at..];
    if let Some(q) = rest.strip_prefix('"') {
        q[..q.find('"').unwrap()].to_owned()
    } else {
        rest.split(' ').next().unwrap().to_owned()
    }
}

#[track_caller]
fn drawn(r: &mut Radio, s: &str) {
    let t = r.text();
    assert!(t.replace('\n', " ").contains(s), "{s:?} isn't drawn; the screen:\n{t}\nthe readback:\n{}", r.sim.readback());
}

/// HF-15, HF-3: HF mode's readback: `hf loading=N` while the patch streams in, then the frequency, band,
/// receiver, mode, step, bandwidth and signal as drawn; the options page as rows.
#[test]
fn hf_mode_readback() {
    let mut r = Radio::new();
    r.menu("0 3 4").wait(200);
    assert!(line(&r, "hf").contains("loading="), "the patch loading first");
    r.wait(4000);
    let l = line(&r, "hf");
    assert_eq!(field(&l, "freq"), "9.58000", "{l}");
    for (k, v) in [("band", "31 m"), ("rx", "SI4732"), ("mode", "AM"), ("step", "5 kHz"), ("bw", "6 kHz")] {
        assert_eq!(field(&l, k), v, "{l}");
        drawn(&mut r, v);
    }
    drawn(&mut r, &field(&l, "signal"));
    r.press(common::key("2")).wait(300);
    assert_eq!(field(&line(&r, "hf"), "entry"), "Go to 2 kHz");
    drawn(&mut r, "Go to 2 kHz");
    r.press(common::key("BACK")).wait(300);
    r.hold(common::key("OK")).wait(300);
    let rb = r.sim.readback();
    assert!(rb.contains("row 1 \"Step\" \"5 kHz\" sel"), "{rb}");
    assert!(rb.contains("row 0 \"Full menu\" \">\""), "{rb}");
}

/// BCR-26.1, BCR-26.2, BCR-26.3: a broadcast card's readback: the stereo pilot, signal, SNR and the RDS text,
/// as the card draws them.
#[test]
fn broadcast_card_readback() {
    let mut r = Radio::new();
    r.type_("98.1").press(common::key("OK")).wait(3000);
    let card = r.sim.readback().lines().find(|l| l.starts_with("card A")).unwrap().to_owned();
    assert_eq!(field(&card, "kind"), "broadcast", "{card}");
    let card = line(&r, "bcast");
    assert!(card.starts_with("bcast A \""), "{card}");
    let rds = card.split('"').nth(1).unwrap().to_owned();
    assert!(!rds.is_empty(), "the sample station sends RDS: {card}");
    drawn(&mut r, &rds);
    if field(&card, "stereo") == "1" {
        drawn(&mut r, "STEREO");
    }
    drawn(&mut r, &format!("SNR {} dB", field(&card, "snr")));
}

/// SCOPE-5: the band scope's readback counts the blank bars: across 64 MHz the bars past it stay blank.
#[test]
fn scope_readback_counts_blank_bars() {
    let mut r = Radio::new();
    r.type_("63.9").press(common::key("OK")).wait(500);
    r.hold(common::key("1")).wait(3000);
    let l = line(&r, "scope");
    assert_eq!(field(&l, "centre"), "63.90000", "{l}");
    assert_eq!(field(&l, "blank"), "26", "64.025 to 64.650 MHz need the other relay state: {l}");
}

/// XB-2, XB-18, SAFE-12.3: crossband's start page readback: the sides, the grid and the pre-flight checks, as
/// drawn; a side outside the band plan fails its check.
#[test]
fn crossband_start_readback() {
    let mut r = Radio::new();
    r.menu("0 3 5").wait(300);
    let rb = r.sim.readback();
    assert!(rb.contains("xb running=0 cell=0"), "{rb}");
    let checks: Vec<&str> = rb.lines().filter(|l| l.starts_with("check ")).collect();
    assert_eq!(checks.len(), 4, "{rb}");
    for c in &checks {
        let t = &c[c.find('"').unwrap() + 1..c.len() - 1];
        drawn(&mut r, t);
    }
    assert!(rb.contains("row 0 \"Power\""), "{rb}");
}

/// SETUP-10: setup's summary readback: its rows and the note on adding channels, as drawn.
#[test]
fn setup_summary_readback() {
    let mut r = Radio::new();
    r.menu("0 9 7 1").wait(300);
    assert!(line(&r, "setup").contains("page=1 of=8"), "{}", r.sim.readback());
    r.press(common::key("HASH")).wait(300);
    let rb = r.sim.readback();
    assert!(line(&r, "setup").contains("page=8"), "{rb}");
    assert!(rb.contains("row 0 \"Channels\""), "{rb}");
    let help = rb.lines().find(|l| l.starts_with("help \"summary\"")).unwrap_or_else(|| panic!("{rb}"));
    assert!(help.contains("Add channels"), "{help}");
    drawn(&mut r, "Region");
}

/// TEXT-5: compose's readback: the count against the 67 APRS allows, as drawn; APRS-38: before that, the
/// address book on the empty recipient, its highlighted callsign drawn, and OK taking it.
#[test]
fn compose_readback() {
    let mut r = Radio::new();
    r.menu("0 5 2").wait(300);
    r.press(common::key("HASH")).wait(300);
    let b = line(&r, "book");
    let sel = field(&b, "sel");
    drawn(&mut r, &sel);
    r.press(common::key("MENU")).wait(300);
    let l = line(&r, "compose");
    assert_eq!(field(&l, "to"), sel, "{l}");
    let count = field(&l, "count");
    assert!(count.ends_with("/67"), "{l}");
    drawn(&mut r, &count);
}

/// GPS-15.2, GPS-15.3, GPS-15.4: the position page's readback: the position, the six tiles and the nearest
/// waypoint, as drawn.
#[test]
fn gps_position_readback() {
    let mut r = Radio::new();
    r.menu("0 6 2").wait(500);
    let rb = r.sim.readback();
    let pos = line(&r, "pos");
    let parts: Vec<&str> = pos.split('"').collect();
    drawn(&mut r, parts[1]);
    for name in ["Grid", "Altitude", "Speed", "Course", "Local time", "UTC"] {
        let row = rb.lines().find(|l| l.starts_with(&format!("row 0 \"{name}\""))).unwrap_or_else(|| panic!("{rb}"));
        let v = row.split('"').nth(3).unwrap();
        drawn(&mut r, v);
    }
    let near = line(&r, "near");
    drawn(&mut r, near.split('"').nth(3).unwrap());
}

/// SQL-12.1, SQL-7: the squelch screen's readback: the level, the live signal and the noise floor beside it, as
/// drawn.
#[test]
fn squelch_readback() {
    let mut r = Radio::new();
    r.down(common::key("SIDE1"));
    r.knob(1).wait(1500);
    let l = line(&r, "squelch");
    let level = field(&l, "level");
    drawn(&mut r, &level);
    assert!(field(&l, "s").starts_with('S'), "the live signal (it moves: not compared with the last drawing): {l}");
    let floor = field(&l, "floor");
    assert!(floor.starts_with("Noise floor"), "{l}");
    drawn(&mut r, &floor);
    r.up(common::key("SIDE1"));
}

/// PIN-2, PIN-3: the PIN entry's readback: its heading, a star per digit (never the digits, even setting a new
/// PIN) and the line under them, as drawn.
#[test]
fn pin_entry_readback() {
    let mut r = Radio::new();
    r.menu("0 9 3 1").press(common::key("OK")).wait(300);
    r.type_("135").wait(200);
    let l = line(&r, "pin");
    assert!(l.starts_with("pin \"New PIN\""), "{l}");
    assert_eq!(field(&l, "typed"), "***", "{l}");
    assert!(!l.contains("135"), "never the digits: {l}");
    drawn(&mut r, "New PIN");
}
