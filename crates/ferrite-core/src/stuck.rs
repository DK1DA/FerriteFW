//! SAFE-14's stuck PTT notice, shared by the Radio task (which watches the inputs, `ferrite_radio::ptt`) and the UI
//! (which shows them): the inputs as bits, and their names.

/// The inputs, as bits of `TxLive::ptt_stuck` (its low nibble: stuck now) and `ferrite_radio::ptt`'s masks.
pub const PTT_A: u8 = 1;
pub const PTT_B: u8 = 2;
pub const PTT_EXT: u8 = 4;

/// `TxLive::ptt_stuck`'s low nibble: the inputs stuck now.
pub const NOW: u8 = 0x0F;
/// Its high nibble: the inputs stuck at some time since boot (kept after the release), so the UI can name them even
/// when they were let go before it first looked.
pub const EVER_SHIFT: u8 = 4;

/// The next `TxLive::ptt_stuck` from the last and the inputs stuck now.
pub const fn latch(prev: u8, now: u8) -> u8 {
    (now & NOW) | ((prev | now << EVER_SHIFT) & !NOW)
}

/// The inputs and their names (the text in the table itself, padded, and its length). A named `static`: the Radio
/// task reads it, and only named statics stay in the firmware's fast flash (firmware/memory.x); short literals
/// land among the slow part's tables, which `slowcheck` can't tell from a table read from interrupt context.
static NAMES: [(u8, [u8; 8], u8); 3] = [(PTT_A, *b"PTT A   ", 5), (PTT_B, *b"PTT B   ", 5), (PTT_EXT, *b"side PTT", 8)];
/// Between two names; a named `static` for the same reason.
static SEP: [u8; 3] = *b" + ";

/// The stuck inputs' names for the notice and the cable log ("PTT A", "PTT B", "side PTT", joined with " + ").
pub fn names(mask: u8, out: &mut [u8]) -> usize {
    let mut n = 0;
    for (bit, name, len) in NAMES.iter() {
        let (bit, name) = (*bit, &name[..*len as usize]);
        if mask & bit == 0 {
            continue;
        }
        let part: &[u8] = if n > 0 { &SEP } else { &[] };
        for &c in part.iter().chain(name) {
            if n < out.len() {
                out[n] = c;
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SAFE-14: what was stuck since boot stays in the high nibble after the release; the low nibble follows now.
    #[test]
    fn latch_keeps_what_was_stuck() {
        let v = latch(0, PTT_A);
        assert_eq!((v & NOW, v >> EVER_SHIFT), (PTT_A, PTT_A));
        let v = latch(v, 0);
        assert_eq!((v & NOW, v >> EVER_SHIFT), (0, PTT_A));
        let v = latch(v, PTT_EXT);
        assert_eq!((v & NOW, v >> EVER_SHIFT), (PTT_EXT, PTT_A | PTT_EXT));
        let mut b = [0u8; 28];
        let n = names(v >> EVER_SHIFT, &mut b);
        assert_eq!(&b[..n], b"PTT A + side PTT");
    }
}
