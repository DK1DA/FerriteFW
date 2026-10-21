//! TERM-35, LINK-33: the slots' working fields from the PC, through the keypad's paths.

use super::*;
use crate::term::cmd::{self, Done, Line, List};
use ferrite_core::freq::mhz;
use std::format;

fn run(app: &mut App, line: &str) -> Result<Done, std::string::String> {
    cmd::run(app, &store(), &Device::default(), line.as_bytes(), false).map_err(|m| std::string::String::from_utf8_lossy(m.as_bytes()).into())
}

fn said(app: &mut App, line: &str) -> std::string::String {
    match run(app, line) {
        Ok(Done::Said(m)) => std::string::String::from_utf8_lossy(m.as_bytes()).into(),
        r => panic!("{line}: {:?}", r.map(|_| ())),
    }
}

fn refused(app: &mut App, line: &str) -> std::string::String {
    run(app, line).err().unwrap_or_else(|| panic!("{line}: not refused"))
}

fn drain(app: &mut App) -> (Vec<RadioCmd>, Vec<ChanWrite>) {
    (core::iter::from_fn(|| app.pop_cmd()).collect(), core::iter::from_fn(|| app.pop_write()).collect())
}

/// TERM-35: `slot` lists the three, `slot A` shows one; neither changes anything (TERM-25's reads).
#[test]
fn slot_lists_and_shows() {
    let mut t = T::new();
    assert!(matches!(run(&mut t.app, "slot"), Ok(Done::List(List::Slots))));
    let mut l = Line::new();
    let lines: Vec<_> = (0..4)
        .map_while(|i| cmd::list_line(&t.app, &store(), &Device::default(), &ferrite_core::msg::LiveAll::default(), List::Slots, i, &mut l).then(|| std::string::String::from_utf8_lossy(l.as_bytes()).into_owned()))
        .collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("A* vfo ") && lines[1].starts_with("B  ") && lines[2].starts_with("C  "), "{lines:?}");
    assert!(said(&mut t.app, "slot b").starts_with("B  vfo "));
    assert!(!cmd::writes(b"slot") && !cmd::writes(b"slot A") && cmd::writes(b"slot A nfm") && cmd::writes(b"slot B active"));
    assert_eq!(drain(&mut t.app), (Vec::new(), Vec::new()));
}

/// TERM-35, TX-9: power on 1.25 m is set like any band's, alone or in a combined change.
#[test]
fn slot_power_on_125m() {
    let mut t = T::new();
    said(&mut t.app, "slot A 223.53");
    let p = t.app.slot_state(0).power;
    let other = ["low", "mid", "high"][(p as usize + 1) % 3];
    let s = said(&mut t.app, &format!("slot A {other}"));
    assert!(s.contains(&format!("power={other}")), "1.25 m takes the level: {s}");
    let back = ["low", "mid", "high"][p as usize];
    let s = said(&mut t.app, &format!("slot A 145.495 fm {back}"));
    assert!(s.contains(&format!("power={back}")), "{s}");
    let s = said(&mut t.app, &format!("slot A 223.53 {other}"));
    assert!(s.contains(&format!("power={other}")), "{s}");
}

/// TERM-35: a VFO slot takes a frequency, mode, power and tones as the keypad sets them (VFO entry, quick
/// settings, Tones): the radio is told, and nothing is written to the channel store.
#[test]
fn slot_sets_a_vfo_as_the_keypad() {
    let mut t = T::new();
    let s = said(&mut t.app, "slot B 146.94 nfm low tone 100.0");
    assert!(s.starts_with("B  vfo 146.94000 mode=NFM power=low tone=100.0"), "{s}");
    let g = t.app.slot_state(1);
    assert_eq!((g.freq, g.mode, g.power, g.rx_tone, g.tx_tone, g.active), (mhz(146) + 94_000, 1, 0, 1000, 1000, false));
    let (c, w) = drain(&mut t.app);
    assert!(c.contains(&RadioCmd::SlotParam { slot: 1, param: Param::Mode, value: 1 }));
    assert!(c.contains(&RadioCmd::SlotParam { slot: 1, param: Param::TxTone, value: 1000 }));
    assert!(w.is_empty(), "a VFO's change writes no channel");
    assert_eq!(t.app.active(), 0, "the active slot stays A");
    // the active slot, as SLOT does
    said(&mut t.app, "slot B active");
    assert_eq!(t.app.active(), 1);
    assert!(drain(&mut t.app).0.contains(&RadioCmd::Active(1)));
    // rxtone and txtone apart
    assert!(said(&mut t.app, "slot B rxtone D023N txtone off").contains("txtone=off rxtone=D023N"));
}

/// TERM-35.2, TONE-2.1: a tone from the terminal as a mode and a value too: `ctcss 88.5`, `dcs-n 023`, `dcs-i 754`;
/// a value that isn't the mode's is refused and nothing changes.
#[test]
fn slot_tone_as_mode_and_value() {
    let mut t = T::new();
    assert!(said(&mut t.app, "slot B txtone ctcss 88.5 rxtone dcs-i 754").contains("txtone=88.5 rxtone=D754I"));
    assert!(said(&mut t.app, "slot B tone DCS-N 023").contains("tone=D023N"));
    assert!(said(&mut t.app, "slot B rxtone off").contains("rxtone=off"));
    let before = t.app.slot_state(1);
    assert!(refused(&mut t.app, "slot B txtone ctcss D023N").contains("CTCSS takes a tone"));
    assert!(refused(&mut t.app, "slot B txtone dcs-n 100.0").contains("DCS-N takes an octal code"));
    assert!(refused(&mut t.app, "slot B txtone dcs-i 089").contains("DCS-I takes an octal code"));
    assert_eq!(t.app.slot_state(1), before, "a refusal changes nothing");
}

/// TERM-35, LINK-33: the offset from the PC, MENU 5's path: a VFO's -0.6 MHz shift cleared with `offset off`
/// (the Radio task told, no channel written), set to any amount with a sign, and a split's TX frequency; a shift
/// that needs the other band relay (TX-37), 100 MHz or more, or a split no chip tunes is refused, nothing changed.
/// On a channel slot it changes the channel and saves it (EDIT-4).
#[test]
fn slot_offset_and_split() {
    let mut t = T::new();
    let s = said(&mut t.app, "slot B 146.94 offset -0.6");
    assert!(s.contains("offset=-0.60000"), "{s}");
    let (c, w) = drain(&mut t.app);
    assert!(c.contains(&RadioCmd::SlotParam { slot: 1, param: Param::Offset, value: -60_000 }), "{c:?}");
    assert!(w.is_empty());
    let s = said(&mut t.app, "slot B offset off");
    assert!(!s.contains("offset="), "simplex: {s}");
    assert_eq!(t.app.slot_state(1).offset, 0);
    assert!(drain(&mut t.app).0.contains(&RadioCmd::SlotParam { slot: 1, param: Param::Offset, value: 0 }));
    said(&mut t.app, "slot B offset +1.6");
    assert_eq!((t.app.slot_state(1).offset, t.app.slot_state(1).split), (160_000, false));
    let s = said(&mut t.app, "slot B split 147.5");
    assert!(s.contains("tx=147.50000"), "{s}");
    assert!(drain(&mut t.app).0.contains(&RadioCmd::SlotParam { slot: 1, param: Param::Split, value: (mhz(147) + 50_000) as i32 }));
    // a shift back ends the split
    said(&mut t.app, "slot B offset -0.6");
    let g = t.app.slot_state(1);
    assert_eq!((g.offset, g.split), (-60_000, false));
    assert!(drain(&mut t.app).0.contains(&RadioCmd::SlotParam { slot: 1, param: Param::Split, value: 0 }));
    for bad in ["slot B split 52.0", "slot B offset +300", "slot B split 5.0", "slot B offset +100"] {
        let r = refused(&mut t.app, bad);
        assert!(!r.is_empty(), "{bad}");
        let g = t.app.slot_state(1);
        assert_eq!((g.offset, g.split), (-60_000, false), "{bad}: unchanged");
    }
    // a channel slot: the channel saved
    t.app.set_channel(0, 0, 0, &store());
    drain(&mut t.app);
    said(&mut t.app, "slot A offset +0.6");
    let (_, w) = drain(&mut t.app);
    assert!(matches!(w[..], [ChanWrite::Put { ch, .. }] if ch.cfg.offset == 60_000 && !ch.cfg.split), "{w:?}");
}

/// TERM-35: a channel slot isn't retuned: a frequency needs `vfo` (V/M first); its mode, power and tones change
/// the channel and save it, as the quick settings do on the radio (EDIT-4).
#[test]
fn slot_on_a_channel() {
    let mut t = T::new();
    t.app.set_channel(0, 0, 0, &store());
    drain(&mut t.app);
    let r = refused(&mut t.app, "slot A 146.52");
    assert!(r.contains("on a channel: add vfo"), "{r}");
    assert!(t.app.slot_state(0).channel.is_some(), "unchanged");
    let (c, w) = drain(&mut t.app);
    assert!(c.is_empty() && w.is_empty());
    said(&mut t.app, "slot A high");
    let (_, w) = drain(&mut t.app);
    assert!(matches!(w[..], [ChanWrite::Put { ch, .. }] if ch.cfg.power == 2), "the channel saved, as the keypad does");
    let s = said(&mut t.app, "slot A vfo 147.00");
    assert!(s.starts_with("A* vfo 147.00000"), "{s}");
    assert!(drain(&mut t.app).1.is_empty(), "V/M and the VFO write no channel");
}

/// TERM-35: refusals change nothing and say why: frequencies no chip tunes, bad words, the APRS slot,
/// channel-only mode, transmitting.
#[test]
fn slot_refusals() {
    let mut t = T::new();
    assert!(refused(&mut t.app, "slot A 7.1").contains("HF mode"));
    assert!(refused(&mut t.app, "slot A 700").contains("Can't tune"));
    assert!(refused(&mut t.app, "slot D").starts_with("slot A|B|C"));
    assert!(refused(&mut t.app, "slot A usb").contains("not a slot word"));
    assert!(refused(&mut t.app, "slot A tone 300.0").contains("outside 60.0"));
    t.app.set.aprs_mode = true;
    assert!(refused(&mut t.app, "slot C nfm").contains("APRS"));
    said(&mut t.app, "slot C"); // reading it is fine
    t.app.set.aprs_mode = false;
    t.app.set.chan_only = true;
    assert!(refused(&mut t.app, "slot A 146.52").contains("channel-only"));
    said(&mut t.app, "slot B active"); // only the active slot changes there
    t.app.set.chan_only = false;
    t.app.radio.keyed = Some(0);
    assert!(refused(&mut t.app, "slot B nfm").contains("transmitting"));
    t.app.radio.keyed = None;
    assert!(drain(&mut t.app).1.is_empty());
}
