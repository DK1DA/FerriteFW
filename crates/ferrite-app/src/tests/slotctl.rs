//! TERM-35, LINK-33: the slots' working fields from the PC, through the keypad's paths.

use super::*;
use crate::term::cmd::{self, Done, Line, List};
use ferrite_core::freq::mhz;

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
        .map_while(|i| cmd::list_line(&t.app, &store(), &Device::default(), List::Slots, i, &mut l).then(|| std::string::String::from_utf8_lossy(l.as_bytes()).into_owned()))
        .collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("A* vfo ") && lines[1].starts_with("B  ") && lines[2].starts_with("C  "), "{lines:?}");
    assert!(said(&mut t.app, "slot b").starts_with("B  vfo "));
    assert!(!cmd::writes(b"slot") && !cmd::writes(b"slot A") && cmd::writes(b"slot A nfm") && cmd::writes(b"slot B active"));
    assert_eq!(drain(&mut t.app), (Vec::new(), Vec::new()));
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
/// channel-only mode, transmitting, 1.25 m power.
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
    said(&mut t.app, "slot B 223.5");
    assert!(refused(&mut t.app, "slot B high").contains("0.25 W"), "TX-10");
    assert!(drain(&mut t.app).1.is_empty());
}
