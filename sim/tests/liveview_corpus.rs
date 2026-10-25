//! Live View completeness (LINK-19.4, decision 289): for every screen the simulator reaches (the jump list, home in
//! each slot and holder, broadcast, scan, entry with digits, toasts and alerts, About's pages), the texts the firmware
//! drew and the UI readback it would send, written to `target/liveview/corpus.json`. Ferrite Console's
//! `liveview-complete` test draws each readback with its Live View and fails on any drawn text it doesn't show, so a
//! screen the radio changes and Console doesn't follow fails there. Run by Console's test, or by hand:
//!
//! ```text
//! cargo test -p ferrite-sim --test liveview_corpus
//! ```

mod common;

use common::Radio;
use ferrite_sim::simcore::jump_names;
use std::fmt::Write as _;
use std::path::Path;

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// One screen: its name, what the firmware drew there (top to bottom) and the readback.
fn entry(name: &str, r: &mut Radio) -> String {
    let texts: Vec<String> = r.sim.screen_texts().into_iter().map(|t| json_str(&t.s)).collect();
    format!("{{\"name\":{},\"texts\":[{}],\"readback\":{}}}", json_str(name), texts.join(","), json_str(&r.sim.readback()))
}

/// The states beyond the jump list: a fresh sample radio, then a script (`common::Radio::run`).
const SCRIPTS: &[(&str, &str)] = &[
    ("home, B active", "press SLOT; wait 1s"),
    ("home, C active", "press SLOT; press SLOT; wait 1s"),
    ("home, layout 1", "event layout; home; wait 1s"),
    ("home, layout 2", "event layout; event layout; home; wait 1s"),
    ("APRS mode, A active", "menu 0 5 3; press OK; home; wait 2s"),
    ("APRS mode, C active", "menu 0 5 3; press OK; home; press SLOT; press SLOT; wait 2s"),
    ("APRS mode, C active, layout 1", "menu 0 5 3; press OK; event layout; home; press SLOT; press SLOT; wait 2s"),
    ("APRS mode, APRS list", "menu 0 5 3; press OK; home; press SLOT; press SLOT; press OK; wait 1s"),
    ("broadcast FM on A", "event fm; wait 3s"),
    ("broadcast FM, quick settings", "event fm; wait 3s; press MENU; wait 1s"),
    ("scan on A", "press SIDE2; wait 2s"),
    ("frequency entry, 1 digit", "type 1; wait 1s"),
    ("frequency entry, 146.52", "type 14652; wait 1s"),
    ("frequency entry, HF", "type 7; wait 1s"),
    ("quick menu", "press MENU; wait 1s"),
    ("message toast", "event message; wait 500ms"),
    ("alert", "event alert; wait 1s"),
    ("crossband running", "event crossband; wait 2s"),
    ("tone hint", "event tonehint; wait 1s"),
    ("low battery", "event battery; wait 1s"),
    ("monitor held", "down SIDE1; wait 1s"),
];

/// LINK-19.4: the corpus Console's Live View completeness check reads (every screen's drawn texts and readback).
#[test]
fn liveview_corpus() {
    let mut out = Vec::new();
    for (i, n) in jump_names().enumerate() {
        let mut r = Radio::new();
        r.sim.jump(i);
        r.wait(1000);
        out.push(entry(&format!("{i:03} {n}"), &mut r));
    }
    for (name, script) in SCRIPTS {
        let mut r = Radio::new();
        r.home();
        r.run(script);
        out.push(entry(name, &mut r));
    }
    // About's pages, ▶ by ▶ (HELP-1 to HELP-5).
    let about = jump_names().position(|n| n.ends_with("About")).expect("About in the jump list");
    let mut r = Radio::new();
    r.sim.jump(about);
    r.wait(1000);
    for p in 1..=5 {
        r.run("press RIGHT; wait 500ms");
        out.push(entry(&format!("About, page {}", p + 1), &mut r));
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/liveview");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("corpus.json"), format!("[\n{}\n]\n", out.join(",\n"))).unwrap();
    assert!(out.len() > 80, "{} screens", out.len());
}
