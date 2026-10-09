//! Helpers for the code-review rule checks in `tests/rules.rs`: the manual's rules that are about how the code is
//! built rather than what the radio does (one owner per chip, nothing writes MCU flash, English-only fonts...).
//! They read the workspace's sources and assets, so they run on the PC with plain `cargo test` in well under a
//! second; the one check that needs the firmware built is `#[ignore]`d (run it with `-- --ignored`).

use std::path::{Path, PathBuf};

/// The ferrite workspace root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("workspace root")
}

/// Every `.rs` file under `dir` (relative to the root), sorted.
pub fn rs_files(dir: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(&root().join(dir), &mut |p| {
        if p.extension().is_some_and(|e| e == "rs") {
            out.push(p.to_path_buf());
        }
    });
    out.sort();
    out
}

/// Every file under `dir`, skipping build output and VCS folders.
pub fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if name != "target" && !name.starts_with('.') && name != "node_modules" {
                walk(&p, f);
            }
        } else {
            f(&p);
        }
    }
}

/// A file's code with `//` comments removed (a plain scan: good enough for names and literals, which is what
/// the checks look for).
pub fn code(p: &Path) -> String {
    let src = std::fs::read_to_string(p).unwrap_or_default();
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let mut in_str = false;
        let mut cut = line.len();
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'"' if i == 0 || b[i - 1] != b'\\' => in_str = !in_str,
                b'/' if !in_str && b.get(i + 1) == Some(&b'/') => {
                    cut = i;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        out.push_str(&line[..cut]);
        out.push('\n');
    }
    out
}

/// Files under `dir` whose code (comments removed) contains `needle`, as paths relative to the root (this
/// crate's own checks, which name what they look for, are left out).
pub fn files_using(dir: &str, needle: &str) -> Vec<String> {
    let r = root();
    let me = r.join("crates/ferrite-checks");
    rs_files(dir)
        .into_iter()
        .filter(|p| !p.starts_with(&me) && code(p).contains(needle))
        .map(|p| p.strip_prefix(&r).unwrap().to_string_lossy().into_owned())
        .collect()
}

/// Every hex literal (`0x…`, underscores allowed) in a piece of code.
pub fn hex_literals(code: &str) -> Vec<u64> {
    let b = code.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'0' && (b[i + 1] == b'x' || b[i + 1] == b'X') && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let mut j = i + 2;
            let mut v: u64 = 0;
            let mut digits = 0;
            while j < b.len() && (b[j].is_ascii_hexdigit() || b[j] == b'_') {
                if b[j] != b'_' {
                    v = v.wrapping_shl(4) | (b[j] as char).to_digit(16).unwrap() as u64;
                    digits += 1;
                }
                j += 1;
            }
            if digits > 0 && digits <= 16 {
                out.push(v);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}
