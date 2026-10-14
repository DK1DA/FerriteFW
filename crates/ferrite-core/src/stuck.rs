//! SAFE-14's stuck PTT notice, shared by the Radio task (which watches the inputs, `ferrite_radio::ptt`) and the UI
//! (which shows them): the inputs as bits, and their names.

/// The inputs, as bits of `TxLive::ptt_stuck` and `ferrite_radio::ptt`'s masks.
pub const PTT_A: u8 = 1;
pub const PTT_B: u8 = 2;
pub const PTT_EXT: u8 = 4;

/// The stuck inputs' names for the notice and the cable log ("PTT A", "PTT B", "side PTT", joined with " + ").
pub fn names(mask: u8, out: &mut [u8]) -> usize {
    let mut n = 0;
    for (bit, name) in [(PTT_A, &b"PTT A"[..]), (PTT_B, b"PTT B"), (PTT_EXT, b"side PTT")] {
        if mask & bit == 0 {
            continue;
        }
        let part: &[u8] = if n > 0 { b" + " } else { b"" };
        for &c in part.iter().chain(name) {
            if n < out.len() {
                out[n] = c;
                n += 1;
            }
        }
    }
    n
}
