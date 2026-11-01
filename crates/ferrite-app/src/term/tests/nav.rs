//! The terminal's keys and pages: escape sequences, the PIN prompt, the lists' movement keys and each page's
//! help.

use super::*;

/// TERM-16: VT100 and VT220 keys: CSI and SS3 arrows, Home and End by both forms, PgUp and PgDn, PF1 to PF4; ESC
/// then another byte is Esc, then that byte.
#[test]
fn escape_sequences() {
    let mut r = Rig::main();
    r.send(b"\x1b[A"); // up from the first wraps to the last
    assert_eq!(r.term.main_sel as usize, MAIN.len() - 1);
    r.send(b"\x1bOB"); // SS3 down wraps back
    assert_eq!(r.term.main_sel, 0);
    r.send(b"\x1bOA");
    assert_eq!(r.term.main_sel as usize, MAIN.len() - 1);
    r.send(b"\x1b[B");
    assert_eq!(r.term.main_sel, 0);
    // PF1 (both forms) opens help, Esc or PF1 closes it
    r.send(b"\x1bOP");
    assert!(r.term.help);
    r.send(b"\x1bOP");
    assert!(!r.term.help);
    r.send(b"\x1b[11~");
    assert!(r.term.help);
    r.send(b"\r");
    assert!(!r.term.help);
    // unknown finals and numbers are dropped
    r.send(b"\x1b[9~\x1b[Z\x1bOZ");
    assert_eq!(r.term.page(), Page::Main);
    assert_eq!(r.term.main_sel, 0);
    // SS3 Enter opens the selection
    r.send(b"\x1bOM");
    assert_eq!(r.term.page(), Page::Channels);
    // ESC then a letter: Esc (back to the menu), then the letter
    r.send(b"\x1bz");
    assert_eq!(r.term.page(), Page::Main);
    // Ctrl-L redraws everything
    r.send(b"\x0c");
    assert!(r.vt.has("Ferrite terminal"));
    // bytes that are no key are ignored
    r.send(&[0x01, 0x80]);
    assert_eq!(r.term.page(), Page::Main);
}

/// TERM-14: the channel list's movement keys: Home and End (both forms), PgUp and PgDn by a page, `z` through the
/// zones, then no zone, then every channel; Esc back to where it was opened from.
#[test]
fn channel_list_keys() {
    let mut r = Rig::main();
    r.send(b"1");
    let n = list_len(ALL, &r.store);
    let pg = r.term.page_rows();
    r.send(b"\x1b[F");
    assert_eq!(r.term.chan_sel, n - 1);
    r.send(b"\x1b[H");
    assert_eq!(r.term.chan_sel, 0);
    r.send(b"\x1b[4~");
    assert_eq!(r.term.chan_sel, n - 1);
    r.send(b"\x1b[1~");
    assert_eq!(r.term.chan_sel, 0);
    r.send(b"\x1bOF");
    assert_eq!(r.term.chan_sel, n - 1);
    r.send(b"\x1bOH");
    assert_eq!(r.term.chan_sel, 0);
    r.send(b"\x1b[6~");
    assert_eq!(r.term.chan_sel, pg.min(n - 1));
    r.send(b"\x1b[5~");
    assert_eq!(r.term.chan_sel, 0);
    r.send(b"\x1b[B\x1b[B");
    assert_eq!(r.term.chan_sel, 2);
    r.send(b"\x1b[A");
    assert_eq!(r.term.chan_sel, 1);
    // z: zone 0, 1, ... the last, no zone, every channel
    let z = r.store.zones();
    let mut seen = Vec::new();
    for _ in 0..z + 2 {
        r.send(b"z");
        seen.push(r.term.zone);
    }
    assert_eq!(seen[0], 0);
    assert_eq!(seen[z as usize - 1], z - 1);
    assert_eq!(seen[z as usize], NO_ZONE);
    assert_eq!(seen[z as usize + 1], ALL);
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!(r.term.page(), Page::Main, "opened from the menu: back to it");
}

/// TERM-14: with no zones `z` goes from every channel to no zone and back; an empty list says so.
#[test]
fn channel_list_without_zones() {
    let mut r = Rig::main();
    r.store.request(crate::data::Op::Wipe);
    r.send(b"1");
    assert!(r.vt.has("No channels here."), "{}", r.vt.text());
    r.send(b"z");
    assert_eq!(r.term.zone, NO_ZONE);
    r.send(b"z");
    assert_eq!(r.term.zone, ALL);
}

/// TERM-14: the zones page: arrows wrap, Home and End, Enter opens that zone's channels, Esc from them comes back
/// to the zones, Esc again to the menu.
#[test]
fn zones_page_keys() {
    let mut r = Rig::main();
    r.send(b"2");
    assert_eq!(r.term.page(), Page::Zones);
    let n = r.store.zones() + 2;
    r.send(b"\x1b[A");
    assert_eq!(r.term.zone_sel, n - 1);
    r.send(b"\x1b[B");
    assert_eq!(r.term.zone_sel, 0);
    r.send(b"\x1b[F");
    assert_eq!(r.term.zone_sel, n - 1);
    r.send(b"\x1b[H");
    assert_eq!(r.term.zone_sel, 0);
    r.send(b"\x1b[B\r"); // the first zone (every channel is the row above it)
    assert_eq!(r.term.page(), Page::Channels);
    assert_eq!(r.term.zone, 0);
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!(r.term.page(), Page::Zones);
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!(r.term.page(), Page::Main);
}

/// TERM-6: the channel editor's fields: arrows wrap, Home and End; ◀▶ on the name and on TX without split say
/// how to type them; # on Home toggles it with its note.
#[test]
fn editor_fields_and_notes() {
    let mut r = Rig::main();
    r.send(b"1\r");
    assert_eq!(r.term.page(), Page::Edit);
    let n = FIELDS.len() as u8;
    r.send(b"\x1b[A");
    assert_eq!(r.term.field, n - 1);
    r.send(b"\x1b[B");
    assert_eq!(r.term.field, 0);
    r.send(b"\x1b[F");
    assert_eq!(r.term.field, n - 1);
    r.send(b"\x1b[H");
    assert_eq!(r.term.field, 0);
    let at = |f: F| FIELDS.iter().position(|x| *x == f).unwrap() as u8;
    r.term.field = at(F::Name);
    r.send(b"\x1b[C");
    assert_eq!(r.term.note.as_bytes(), b"Enter types the name (16 characters)");
    r.term.field = at(F::Tx);
    r.send(b"\x1b[C");
    assert_eq!(r.term.note.as_bytes(), b"TX: Enter types a TX frequency (split)");
    if let Some(h) = FIELDS.iter().position(|x| *x == F::Home) {
        r.term.field = h as u8;
        r.send(b"\r");
        assert_eq!(r.term.note.as_bytes(), b"Home channel: hold 0 on home jumps here");
        r.send(b"\x1b");
        r.now += 100;
        r.poll();
        assert!(r.vt.has("\u{b7} home channel") || r.raw().contains("home channel"), "the list's foot says so: {}", r.vt.text());
        r.send(b"\r");
        r.term.field = h as u8;
        r.send(b"\r");
        assert_eq!(r.term.note.as_bytes(), b"No longer the home channel");
    }
}

/// TERM-6, TERM-17: the settings page: Home and End, a digit opens that row, Esc goes up a level at a time; a
/// setting the radio has highlighted is changed there.
#[test]
fn settings_page_keys() {
    let mut r = Rig::main();
    r.send(b"3");
    assert_eq!(r.term.page(), Page::Settings);
    r.send(b"\x1b[F");
    assert_eq!(r.term.set_sel as usize, GROUPS.len() - 1);
    r.send(b"\x1b[H");
    assert_eq!(r.term.set_sel, 0);
    r.send(b"7"); // Display, sound, power
    assert_eq!((r.term.g, r.term.set_sel), (7, 0));
    r.send(b"\x1b[F");
    let last = r.term.set_sel;
    assert!(last > 0);
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!((r.term.g, r.term.set_sel), (0, 6), "up to the groups, on the one it was in");
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!(r.term.page(), Page::Main);
}

/// TERM-6, TERM-7, TERM-17: ◀▶ on every settings row answers: changed, not settable from here, left to the
/// radio (the PIN, the region, Dry TX), greyed with the radio's reason, or open on the radio's menu.
#[test]
fn settings_rows_answer() {
    use crate::screens::groups;
    let mut r = Rig::main();
    r.send(b"3");
    let mut seen = std::collections::BTreeSet::new();
    for g in 1..=GROUPS.len() as u8 {
        let n = groups::items(g, 0).len() as u8;
        for i in 1..=n {
            let subs = groups::SUBS.iter().find(|(q, _)| *q == (g, i)).map(|(_, e)| e.len() as u8);
            for k in 0..=subs.unwrap_or(0) {
                if subs.is_some() && k == 0 {
                    continue;
                }
                (r.term.g, r.term.sub, r.term.set_sel) = (g, if k == 0 { 0 } else { i }, if k == 0 { i - 1 } else { k - 1 });
                r.term.note.clear();
                r.term.settings_change(&mut r.app, 1);
                let note = String::from_utf8_lossy(r.term.note.as_bytes()).into_owned();
                assert!(!note.is_empty(), "0 {g} {i} {k}: no answer");
                seen.insert(note);
            }
            // put the screen back: some rows open one or change the mode
            r.app.go_home();
            r.app.set_lpm_mode(0);
        }
    }
    for want in ["Changed", "Not settable yet", "Change this one on the radio"] {
        assert!(seen.contains(want), "{want}: {seen:?}");
    }
    // TERM-17: the row the radio's menu has highlighted
    let mut r = Rig::main();
    r.send(b"3");
    r.app.open(crate::screens::nav::Dest::Group(7));
    let crate::Screen::Group(gr) = r.app.screen() else { panic!("not a group") };
    (r.term.g, r.term.sub, r.term.set_sel) = (7, 0, gr.row);
    r.term.settings_change(&mut r.app, 1);
    assert_eq!(r.term.note.as_bytes(), b"Open on the radio's menu: change it there");
    // a greyed row says why (WX-3: SAME outside the US and Canada)
    r.app.go_home();
    r.app.set.region = 5;
    if groups::greyed(&r.app, &[4, 5, 1]).is_some() {
        (r.term.g, r.term.sub, r.term.set_sel) = (4, 5, 0);
        r.term.settings_change(&mut r.app, 1);
        assert_eq!(r.term.note.as_bytes(), b"US/Canada only");
    }
}

/// TERM-6: Enter on a settings row changes it as ▶ does; Esc from a third-level list goes back to its row.
#[test]
fn settings_enter_and_up_from_a_list() {
    let mut r = Rig::main();
    r.send(b"3");
    let (g, i) = crate::screens::groups::SUBS[0].0;
    r.send(&[b'0' + g]);
    r.term.set_sel = i - 1;
    r.send(b"\r");
    assert_eq!((r.term.g, r.term.sub), (g, i), "into the list");
    r.send(b"\r");
    assert!(!r.term.note.is_empty(), "Enter on a row answers as ▶ does");
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!((r.term.g, r.term.sub, r.term.set_sel), (g, 0, i - 1), "back on its row");
}

/// TERM-12: the command line: Backspace takes a character back; Up on an empty line brings the last command back;
/// Esc leaves the line; a load's Ctrl-C stops it.
#[test]
fn command_line_editing() {
    let mut r = Rig::main();
    r.send(b":home 3x\x7f\r");
    assert_eq!(r.app.set.home_ch, Some(2), "the x taken back");
    r.app.set.home_ch = None;
    r.send(b":\x1b[A\r");
    for _ in 0..3 {
        r.poll();
    }
    assert_eq!(r.app.set.home_ch, Some(2), "Up brought it back");
    r.send(b":abc\x1b");
    r.now += 100;
    r.poll();
    assert!(!r.term.cmd, "Esc left the line");
    r.cmd("load");
    r.send(b"\x03");
    for _ in 0..4 {
        r.poll();
    }
    assert!(matches!(r.term.con, Con::Done | Con::Off), "{:?}", r.term.con);
}

/// TERM-16: each page's help (`?`) is its own; Esc closes it.
#[test]
fn help_per_page() {
    for (key, page, want) in [(b'1', Page::Channels, HELP_CHANNELS), (b'3', Page::Settings, HELP_SETTINGS), (b'4', Page::Monitor, HELP_MONITOR), (b'7', Page::About, HELP_OTHER)] {
        let mut r = Rig::main();
        r.send(&[key]);
        assert_eq!(r.term.page(), page);
        r.send(b"?");
        let first = want.lines().next().unwrap();
        assert!(r.vt.has(first.trim_end()), "{page:?}: {}", r.vt.text());
        r.send(b"\x1b");
        r.now += 100;
        r.poll();
        assert!(!r.term.help);
    }
    let mut r = Rig::main();
    r.send(b"1\r?");
    assert!(r.vt.has(HELP_EDIT.lines().next().unwrap().trim_end()), "{}", r.vt.text());
}

/// Mail and TNC (manual chapter 38) are placeholders that say so; Esc goes back.
#[test]
fn mail_and_tnc_pages() {
    for (key, name) in [(b'5', "Mail"), (b'6', "TNC")] {
        let mut r = Rig::main();
        r.send(&[key]);
        assert!(r.vt.has(name), "{}", r.vt.text());
        assert!(r.vt.has("Not built yet (manual chapter 38)."));
        r.send(b"\x1b");
        r.now += 100;
        r.poll();
        assert_eq!(r.term.page(), Page::Main);
    }
}

/// GPS-26: the main menu's radio line says the GPS state and the power manager's phase.
#[test]
fn main_menu_gps_line() {
    use ferrite_core::gpspower::GpsPhase;
    for (gps, phase, want) in [
        (GpsState::Searching, GpsPhase::Waking, "GPS searching, waking"),
        (GpsState::Fix, GpsPhase::Running, "GPS fix, awake"),
        (GpsState::Fix, GpsPhase::Standby(12), "GPS fix, standby 12 s"),
        (GpsState::Off, GpsPhase::Standby(12), "GPS off   Bluetooth"),
    ] {
        let mut r = Rig::new();
        r.dev.gps = gps;
        r.dev.gps_phase = phase;
        r.send(b"\r\r");
        assert!(r.vt.has(want), "{want}: {}", r.vt.text());
    }
}

/// LINK-30, LINK-31: over Bluetooth with a PIN set, the terminal asks for it first: digits show as stars,
/// Backspace takes one back, a wrong one says so, Ctrl-C leaves; the right one opens the menu.
#[test]
fn pin_prompt() {
    let mut r = Rig::new();
    r.app.set.pin = 1234;
    r.app.set.pin_len = 4;
    r.term.set_pin_needed(true);
    r.send(b"\r");
    assert!(r.vt.has("This radio has a PIN lock. Type its PIN, then Enter."), "{}", r.vt.text());
    assert!(r.vt.has("Enter Check   Ctrl-C Leave"));
    r.send(b"12");
    assert!(r.vt.has("PIN: **"), "{}", r.vt.text());
    r.send(b"\x7f");
    assert!(r.vt.has("PIN: *") && !r.vt.has("PIN: **"));
    r.send(b"x\r"); // a letter is no digit; Enter checks the one digit
    assert!(r.vt.has("Wrong PIN"), "{}", r.vt.text());
    r.send(b"\r"); // nothing typed: nothing checked
    r.send(b"1234\r");
    assert!(!r.term.pin_gate());
    r.send(b"\r");
    assert_eq!(r.term.page(), Page::Main);
    // Ctrl-C at the prompt leaves
    let mut r = Rig::new();
    r.app.set.pin = 1234;
    r.app.set.pin_len = 4;
    r.term.set_pin_needed(true);
    r.send(b"\r\x03");
    assert_eq!(r.term.page(), Page::Off);
}

/// TERM-6: the editor's foot says where TX goes: from the offset, split, or simplex.
#[test]
fn editor_foot_says_where_tx_goes() {
    let mut r = Rig::main();
    let (c, _) = r.store.by_num(9).unwrap(); // North hill: -600 kHz
    r.term.edit = Some(c);
    r.term.go(Page::Edit);
    r.term.redraw();
    r.poll();
    let tx = mhz5((c.rx as i64 + c.cfg.offset as i64) as Freq10);
    assert!(r.vt.has(&std::format!("TX {} MHz from the offset. Saved as you go.", core::str::from_utf8(tx.as_bytes()).unwrap())), "{}", r.vt.text());
    let mut s = c;
    s.cfg.split = true;
    s.cfg.offset = 14_600_000;
    r.term.edit = Some(s);
    r.term.redraw();
    r.poll();
    assert!(r.vt.has("Split: TX 146.00000 MHz, whatever RX is."), "{}", r.vt.text());
    let mut x = c;
    x.cfg.offset = 0;
    r.term.edit = Some(x);
    r.term.redraw();
    r.poll();
    assert!(r.vt.has("Simplex. Saved as you go."), "{}", r.vt.text());
}

/// TERM-14: the list's Flags column: home, skip, APRS, RX only, hidden, priority, space-separated.
#[test]
fn channel_list_flags() {
    use crate::chan::{FLAG_APRS, FLAG_HIDDEN, FLAG_PRIORITY, FLAG_RX_ONLY, FLAG_SKIP};
    let mut r = Rig::main();
    for (num, flags) in [(0, FLAG_SKIP), (1, FLAG_APRS | FLAG_RX_ONLY), (2, FLAG_HIDDEN), (3, FLAG_PRIORITY)] {
        let (mut c, _) = r.store.by_num(num).unwrap();
        c.flags = flags;
        r.store.apply(ChanWrite::Put { ch: c, from: None });
    }
    r.app.set.home_ch = Some(0);
    r.send(b"1");
    let flags_of = |r: &Rig, name: &str| -> String { r.vt.line(r.vt.row_of(name).unwrap())[71..].into() };
    assert_eq!(flags_of(&r, "Calling 2 m"), "home skip");
    assert_eq!(flags_of(&r, "Calling 70 cm"), "APRS RX");
    assert_eq!(flags_of(&r, "Simplex B"), "hidden");
    assert_eq!(flags_of(&r, "Simplex C"), "prio");
}

/// TERM-5: Monitor with nothing logged says so; a slot receiving shows as `now` with its signal.
#[test]
fn monitor_nothing_yet_and_now() {
    let mut r = Rig::main();
    r.store.request(crate::data::Op::ClearLog);
    r.send(b"4");
    assert!(r.vt.has("Nothing yet."), "{}", r.vt.text());
    r.live.slots[1].squelch_open = true;
    r.live.slots[1].rssi = -73;
    r.term.redraw();
    r.poll();
    let row = (0..r.vt.rows).find(|&i| r.vt.line(i).starts_with(" now")).expect("a now line");
    assert!(r.vt.line(row).contains("RX"), "{}", r.vt.line(row));
}

/// The main menu's slot lines: a channel by number and name, a slot receiving with its signal, one
/// scanning, the active one marked.
#[test]
fn main_menu_slot_lines() {
    let mut r = Rig::new();
    let c9 = r.store.by_num(9).unwrap().0;
    r.app.set_channel_rec(0, c9);
    r.live.slots[1].squelch_open = true;
    r.live.slots[1].rssi = -73;
    r.live.slots[2].scanning = true;
    r.send(b"\r\r");
    let a = r.vt.line(r.vt.row_of("  A  ").unwrap());
    assert!(a.contains("CH 010 North hill"), "{a}");
    assert!(a.contains("active"), "{a}");
    let b = r.vt.line(r.vt.row_of("  B  ").unwrap());
    assert!(b.contains("RX "), "{b}");
    let c = r.vt.line(r.vt.row_of("  C  ").unwrap());
    assert!(c.contains("scanning"), "{c}");
}

/// TERM-18: with colour on, a refusal on the bottom line is yellow on the reverse bar.
#[test]
fn colour_warning_on_the_bar() {
    let mut r = Rig::main();
    r.app.set.term_colour = true;
    r.cmd("frobnicate");
    assert!(r.raw().contains("\x1b[1;93;44m"), "the warning in colour on the bar");
}

/// The cable port setting's names, for the radio's menu too (0 9 2 3).
#[test]
fn cable_port_names() {
    assert_eq!(port_name(0), CABLE_PORTS[0]);
    assert_eq!(port_name(CABLE_PORTS.len() as u8), CABLE_PORTS[0], "wraps");
}

/// INBOX-6.1: an age the radio doesn't know is "earlier"; seconds, minutes and hours otherwise.
#[test]
fn ages() {
    for (s, want) in [(crate::data::AGO_UNKNOWN, "earlier"), (12, "12 s ago"), (125, "2 min ago"), (7300, "2 h ago")] {
        let mut b = Buf::<16>::new();
        ago(s, &mut b);
        assert_eq!(b.as_bytes(), want.as_bytes());
    }
}

/// PIN-3, LINK-31: three misses start the wait; the prompt says how long.
#[test]
fn pin_prompt_waits_after_misses() {
    let mut r = Rig::new();
    r.app.set.pin = 1234;
    r.app.set.pin_len = 4;
    r.term.set_pin_needed(true);
    r.send(b"\r");
    for _ in 0..3 {
        r.send(b"9999\r");
    }
    r.send(b"1234\r");
    assert!(r.vt.has("Too many tries: wait"), "{}", r.vt.text());
    assert!(r.term.pin_gate());
}
