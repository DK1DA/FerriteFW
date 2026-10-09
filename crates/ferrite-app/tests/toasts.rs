//! HOME-9, INBOX-1: a toast is one line in the hint bar, with nothing cut; HOME-8: a shortcut's popup fits its box.
//! Every toast and popup the App can show with fixed text is found in the sources (`show_toast`, `show_popup`,
//! `notify` calls) and measured with the real fonts, as `ferrite_ui::widgets::toast` lays it out: every head the
//! call can show against every body it can show. Text built at run time (names, frequencies) is not checked here.

use std::path::Path;

use ferrite_ui::widgets::{popup_fits, toast_fits};

/// The App's toast buffers (`Toast` in lib.rs): longer text is cut before it is drawn.
const HEAD_MAX: usize = 20;
const BODY_MAX: usize = 28;

#[derive(Debug)]
struct Call {
    file: String,
    line: usize,
    popup: bool,
    heads: Vec<String>,
    bodies: Vec<String>,
    icon: bool,
}

/// The string literals in one argument: `b"..."`, `"..."` and `slow!("...")` all count.
fn literals(arg: &str) -> Vec<String> {
    let b = arg.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let mut s = String::new();
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                    match b[i] {
                        b'n' => s.push('\n'),
                        b't' => s.push('\t'),
                        b'x' if i + 2 < b.len() => {
                            s.push(u8::from_str_radix(&arg[i + 1..i + 3], 16).unwrap_or(b'?') as char);
                            i += 2;
                        }
                        c => s.push(c as char),
                    }
                } else {
                    // UTF-8 bytes stay as they are
                    let start = i;
                    let mut end = i + 1;
                    while end < b.len() && b[end] & 0xC0 == 0x80 {
                        end += 1;
                    }
                    s.push_str(&arg[start..end]);
                    i = end - 1;
                }
                i += 1;
            }
            out.push(s);
        }
        i += 1;
    }
    out
}

/// The call's arguments at the top level, from just after its `(`.
fn args(src: &str) -> Vec<String> {
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut out = Vec::new();
    let mut in_str = false;
    let mut prev = '\0';
    for c in src.chars() {
        if in_str {
            cur.push(c);
            if c == '"' && prev != '\\' {
                in_str = false;
            }
            prev = if prev == '\\' && c == '\\' { '\0' } else { c };
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' if depth == 0 => {
                out.push(cur.trim().to_string());
                return out;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
        prev = c;
    }
    out
}

fn scan(dir: &Path, calls: &mut Vec<Call>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name == "tests" || name.starts_with("tests") {
            continue; // the App's own tests use sample text, not the firmware's toasts
        }
        if p.is_dir() {
            scan(&p, calls);
            continue;
        }
        if p.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        for (name, popup, fixed_icon) in [("show_toast(", false, None), ("show_popup(", true, Some(false)), ("notify(", false, Some(true))] {
            let mut from = 0;
            while let Some(k) = text[from..].find(name) {
                let at = from + k;
                from = at + name.len();
                // definitions, and other functions ending in the same name
                let prev = text[..at].chars().next_back().unwrap_or(' ');
                if text[..at].ends_with("fn ") || prev.is_alphanumeric() || prev == '_' {
                    continue;
                }
                let a = args(&text[from..]);
                if a.len() < 2 {
                    continue;
                }
                let icon = fixed_icon.unwrap_or_else(|| a.get(2).is_none_or(|v| v != "false"));
                let line = text[..at].matches('\n').count() + 1;
                calls.push(Call {
                    file: p.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display().to_string(),
                    line,
                    popup,
                    heads: literals(&a[0]),
                    bodies: if a[1].trim() == "b\"\"" { vec![String::new()] } else { literals(&a[1]) },
                    icon,
                });
            }
        }
    }
}

/// Every toast and popup with fixed text fits: one line, nothing cut, nothing past the buffers.
#[test]
fn every_toast_fits_one_line() {
    let mut calls = Vec::new();
    scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut calls);
    assert!(calls.len() > 100, "found only {} toast calls: is the scan still finding them?", calls.len());
    let mut bad = Vec::new();
    let mut checked = 0;
    for c in &calls {
        if c.heads.is_empty() {
            continue; // a head built at run time
        }
        let bodies: Vec<&str> = if c.bodies.is_empty() { vec![""] } else { c.bodies.iter().map(|s| s.as_str()).collect() };
        for h in &c.heads {
            for b in &bodies {
                checked += 1;
                let fits = if c.popup { popup_fits(h.as_bytes(), b.as_bytes()) } else { toast_fits(h.as_bytes(), b.as_bytes(), c.icon) };
                if !fits || h.len() > HEAD_MAX || b.len() > BODY_MAX {
                    bad.push(format!("{}:{}: \"{}\" / \"{}\"", c.file, c.line, h, b));
                }
            }
        }
    }
    assert!(checked > 100, "only {checked} toast texts checked");
    assert!(bad.is_empty(), "{} toasts don't fit one line (HOME-9) or their buffer:\n{}", bad.len(), bad.join("\n"));
}

/// The measuring itself: the sample toast fits, a long one doesn't, and the envelope costs room.
#[test]
fn toast_measure() {
    assert!(toast_fits(b"VE3KPW-9", b"On my way", true));
    assert!(!toast_fits(b"Channel-only mode", b"hold # to leave it, then PIN", false));
    let long = b"Squelch level 9 now set";
    assert!(toast_fits(long, b"", false) || !toast_fits(long, b"", true));
}
