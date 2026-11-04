//! Writes the SPI parts (decision 335, UPD-20) for a firmware build: the text pack, the boot logo and the terminal's
//! templates, each with its header (`ferrite_core::part`), and `parts.json` naming each part's place and maximum
//! size, for `tools/ffw.py` to pack into the `.ffw`.
//!
//!     cargo run -q -p ferrite-sim --bin ferrite-mkparts -- --build ID OUT_DIR
//!
//! `--build`: the firmware image's build ID (tools/mkbtf.sh reads it from the image); without it, this binary's
//! own (the simulator's: `ferrite-sim --parts OUT_DIR` then loads them).

use std::fmt::Write as _;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut build, mut out) = (ferrite_core::BUILD.to_string(), None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--build" => build = args.next().expect("--build ID"),
            _ => out = Some(a),
        }
    }
    let out = std::path::PathBuf::from(out.expect("usage: ferrite-mkparts [--build ID] OUT_DIR"));
    assert!(build.len() <= 16, "a build ID is 16 bytes at most");
    let mut json = String::from("[\n");
    let written = ferrite_sim::parts::write_all(&out, &build);
    for (i, (k, n, body)) in written.iter().enumerate() {
        let magic = std::str::from_utf8(&k.magic).unwrap();
        let _ = write!(json, "  {{\"file\": \"{}\", \"magic\": \"{magic}\", \"addr\": {}, \"max\": {}}}", k.file, k.slot.start, k.slot.len());
        json.push_str(if i + 1 < written.len() { ",\n" } else { "\n" });
        println!("{}: {n} bytes (body {body}), at 0x{:06X}, {} KB slot", k.file, k.slot.start, k.slot.len() >> 10);
    }
    json.push_str("]\n");
    std::fs::write(out.join("parts.json"), json).unwrap();
}
