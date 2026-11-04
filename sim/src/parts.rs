//! The SPI parts in the simulator (decision 335, UPD-20 – UPD-23). By default none is set up: the host build's own
//! parts are read, as on a radio with its update installed. `--parts DIR` loads the part files `ferrite-mkparts`
//! wrote (as the radio reads them from its SPI flash, each checked by its header, build ID and CRC);
//! `--no-parts` gives a chip without them, so every user of a part shows its fallback.

use ferrite_core::part::{self, Kind};
use std::sync::OnceLock;

/// Every part's body as built into this crate's host build, in `part::KINDS` order.
pub fn bodies() -> Vec<(Kind, Vec<u8>)> {
    part::KINDS
        .iter()
        .map(|k| {
            let body = match &k.magic {
                b"FTXT" => ferrite_app::text::BODY.to_vec(),
                b"FLGO" => ferrite_ui::logo::body(),
                b"FVTP" => ferrite_app::term::template_pack().to_vec(),
                m => panic!("no body for part {m:?}"),
            };
            (k.clone(), body)
        })
        .collect()
}

/// A part's file: its header for `build`, then its body.
pub fn file(k: &Kind, build: &str, body: &[u8]) -> Vec<u8> {
    let mut v = part::header(k, build, body).to_vec();
    v.extend_from_slice(body);
    v
}

/// Writes every part's file into `dir` for `build` (`ferrite-mkparts`, the tests).
#[cfg(not(target_arch = "wasm32"))]
pub fn write_all(dir: &std::path::Path, build: &str) -> Vec<(Kind, usize, usize)> {
    std::fs::create_dir_all(dir).unwrap();
    bodies()
        .iter()
        .map(|(k, body)| {
            let v = file(k, build, body);
            assert!(v.len() <= k.slot.len(), "{}: {} bytes, the slot holds {}", k.file, v.len(), k.slot.len());
            std::fs::write(dir.join(k.file), &v).unwrap();
            (k.clone(), v.len(), body.len())
        })
        .collect()
}

/// The chip's parts: each part's file at its address.
static CHIP: OnceLock<Vec<(u32, Vec<u8>)>> = OnceLock::new();

fn read(addr: u32, buf: &mut [u8]) -> bool {
    buf.fill(0xFF); // erased, where no part was loaded
    for (at, b) in CHIP.get().map_or(&[][..], |c| &c[..]) {
        let (s, e) = (*at as u64, *at as u64 + b.len() as u64);
        for (i, d) in buf.iter_mut().enumerate() {
            let a = addr as u64 + i as u64;
            if a >= s && a < e {
                *d = b[(a - s) as usize];
            }
        }
    }
    true
}

/// Loads the part files in `dir` (missing ones read as erased): which are good for this build, as
/// `part::mount`'s bits.
pub fn load(dir: &std::path::Path) -> u8 {
    let chip = part::KINDS.iter().filter_map(|k| std::fs::read(dir.join(k.file)).ok().map(|b| (k.slot.start, b))).collect();
    let _ = CHIP.set(chip);
    part::set_reader(read);
    part::mount()
}

/// No parts: an erased chip where they go.
pub fn none() -> u8 {
    let _ = CHIP.set(Vec::new());
    part::set_reader(read);
    part::mount()
}

/// `--parts DIR` or `--no-parts`, if given.
pub fn from_args(args: &[String]) {
    if let Some(dir) = args.iter().position(|a| a == "--parts").and_then(|i| args.get(i + 1)) {
        let ok = load(std::path::Path::new(dir));
        eprintln!("parts from {dir}: {} of {} good for this build", ok.count_ones(), part::KINDS.len());
    } else if args.iter().any(|a| a == "--no-parts") {
        none();
    }
}
