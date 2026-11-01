//! uaprs::parse on any information field, with any destination (Mic-E reads its latitude from the destination).
//! Input: one byte for the destination's length (low 3 bits, 0 to 7), the destination, then the information field.
//! Every packet type is reached through the type byte; third-party packets are parsed again, a few levels deep.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::fmt::Write;
use uaprs::parse::{parse, Packet};

fuzz_target!(|data: &[u8]| {
    let Some((&n, rest)) = data.split_first() else { return };
    let n = ((n & 7) as usize).min(rest.len());
    let (mut dest, mut info) = rest.split_at(n);
    let mut sink = String::new();
    for _ in 0..4 {
        let Ok(p) = parse(dest, info) else { return };
        let _ = write!(sink, "{p:?}");
        match p {
            Packet::Position { comment, .. } | Packet::Object { comment, .. } | Packet::Item { comment, .. } => {
                let v: Vec<u8> = comment.bytes().collect();
                assert_eq!(v.len(), comment.len());
                assert_eq!(v.is_empty(), comment.is_empty());
                assert!(v.len() <= comment.raw().len());
                if let Packet::Position { pos, .. } = p {
                    let _ = write!(sink, "{:?}", pos);
                }
                return;
            }
            Packet::ThirdParty(t) => {
                dest = t.dest;
                info = t.info;
            }
            _ => return,
        }
    }
});
