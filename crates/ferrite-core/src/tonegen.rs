//! What the BK4829's tone generator plays, worked out on the host: end tones (TONE-10), CW ID (VOX-12), DTMF
//! symbols (DTMF-7, DTMF-11), local beeps (SND-11), and the frequency counter's and the mic meter's readings.
//! The Radio task times the tones; the bk4829 driver writes stock's registers.

use crate::freq::Freq10;

/// One step of a tone sequence: Hz (0 = silence) and how long, ms.
pub type Step = (u16, u16);

/// The end tone choices (TONE-10), index as 0 8 5 4's list: 0 off.
pub const END_TONES: [&[Step]; 6] = [
    &[],
    &[(1000, 150)],                           // Beep
    &[(800, 100), (1200, 100)],               // Two-tone up
    &[(1200, 100), (800, 100)],               // Two-tone down
    &[(1000, 60), (1250, 60), (1500, 60)],    // Chirp
    &[(2475, 250)],                           // Quindar
];

/// CW ID (VOX-12): 700 Hz at 20 wpm.
pub const CW_HZ: u16 = 700;
pub const CW_WPM: u16 = 20;

/// One dit at `wpm` (PARIS: 1200 / wpm ms).
pub const fn dit_ms(wpm: u16) -> u16 {
    1200 / if wpm == 0 { 1 } else { wpm }
}

/// Morse for A-Z, 0-9 and / (the callsign's characters), as dits (1) and dahs (3) packed from the low bits: each
/// element 2 bits (1 dit, 3 dah, 0 end).
fn morse(c: u8) -> u16 {
    const LETTERS: [&[u8]; 26] = [
        b".-", b"-...", b"-.-.", b"-..", b".", b"..-.", b"--.", b"....", b"..", b".---", b"-.-", b".-..", b"--",
        b"-.", b"---", b".--.", b"--.-", b".-.", b"...", b"-", b"..-", b"...-", b".--", b"-..-", b"-.--", b"--..",
    ];
    const DIGITS: [&[u8]; 10] =
        [b"-----", b".----", b"..---", b"...--", b"....-", b".....", b"-....", b"--...", b"---..", b"----."];
    let pat: &[u8] = match c.to_ascii_uppercase() {
        b'A'..=b'Z' => LETTERS[(c.to_ascii_uppercase() - b'A') as usize],
        b'0'..=b'9' => DIGITS[(c - b'0') as usize],
        b'/' => b"-..-.",
        _ => return 0,
    };
    pat.iter().rev().fold(0, |acc, &e| acc << 2 | if e == b'-' { 3 } else { 1 })
}

/// The CW keying of `text`: (key down?, ms) steps, gaps included, at `wpm`. Characters CW can't send are
/// skipped; a space is a word gap.
pub struct Cw<'a> {
    text: &'a [u8],
    i: usize,
    /// The current character's remaining elements, and whether the next step is the gap after an element.
    left: u16,
    gap: bool,
    dit: u16,
}

impl<'a> Cw<'a> {
    pub fn new(text: &'a [u8], wpm: u16) -> Self {
        Cw { text, i: 0, left: 0, gap: false, dit: dit_ms(wpm) }
    }
}

impl Iterator for Cw<'_> {
    type Item = (bool, u16);
    fn next(&mut self) -> Option<(bool, u16)> {
        loop {
            if self.gap {
                self.gap = false;
                // After an element: 1 dit inside a character, 3 between characters (letter gap).
                return Some((false, if self.left == 0 { 3 * self.dit } else { self.dit }));
            }
            if self.left != 0 {
                let e = self.left & 3;
                self.left >>= 2;
                // No gap after the last element of the last character.
                self.gap = self.left != 0 || self.i < self.text.len();
                return Some((true, e * self.dit));
            }
            let c = *self.text.get(self.i)?;
            self.i += 1;
            if c == b' ' {
                return Some((false, 4 * self.dit)); // with the letter gap before it: 7 dits
            }
            self.left = morse(c);
        }
    }
}

/// The total length of a CW ID, ms (for the timeout's count: TONE-11 counts the end tone and ID).
pub fn cw_ms(text: &[u8], wpm: u16) -> u32 {
    Cw::new(text, wpm).map(|(_, ms)| ms as u32).sum()
}

/// The DTMF symbol for the UI's key byte (RadioCmd::Dtmf): digits, A-D (DTMF-11: MENU, ▲, ▼, BACK), * and #.
pub fn dtmf_symbol(key: u8) -> Option<char> {
    match key {
        b'0'..=b'9' | b'A'..=b'D' | b'*' | b'#' => Some(key as char),
        b'a'..=b'd' => Some(key.to_ascii_uppercase() as char),
        _ => None,
    }
}

/// Tail elimination (TONE-5): how long the carrier stays up with it, as stock (0x08024DCC keeps the TX 400 ms).
pub const TAIL_MS: u16 = 400;

/// Which tail elimination a transmission ends with (TONE-5): stock's 55 Hz tone with no sub-tone (0x0801D078), the
/// CTCSS phase shift on a CTCSS transmission when 0 8 5 2 picks one, the 134.4 Hz turn-off code on DCS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TailKind {
    Tone55,
    /// REG_52's bits 14:13: 0 the DCS turn-off code, 1-3 the CTCSS phase shift of 120°, 180°, 240°.
    Shift(u8),
}

/// The tail for a transmission with this tone word (CHAN-3: bit 15 DCS) and 0 8 5 2's choice (0 55 Hz, 1-3 the
/// phase shifts): the phase is for CTCSS only.
pub fn tail_kind(tx_tone: u16, phase: u8) -> TailKind {
    match tx_tone {
        0 => TailKind::Tone55,
        t if t & 0x8000 != 0 => TailKind::Shift(0),
        _ if phase == 0 || phase > 3 => TailKind::Tone55,
        _ => TailKind::Shift(phase),
    }
}

/// DTMF-3: a sequence's length, ms: the first digit delay, then each digit on for `ms` and off for the same.
pub fn dtmf_seq_ms(digits: usize, ms: u16, delay_ms: u16) -> u32 {
    delay_ms as u32 + digits as u32 * 2 * ms as u32
}

/// A local beep's length, clamped: the Radio task waits through it.
pub const BEEP_MAX_MS: u16 = 1_000;

/// The counter (TSRCH-10): stock (0x08021BC4) takes two frequency-scan readings in a row and keeps them only when
/// they agree within 500 Hz, then the mean. 10 Hz units.
pub fn counter_agree(a: Freq10, b: Freq10) -> Option<Freq10> {
    (a.abs_diff(b) < 50).then(|| ((a as u64 + b as u64) / 2) as Freq10)
}

/// The chip that counts `f`'s band (TSRCH-10): chip 0 for 64-300 MHz, chip 1 from 300 MHz to 1300 MHz outside
/// 630-756 MHz; None outside both (18-64 MHz would need the relay: never counted, SAFE-20).
pub fn counter_chip(f: Freq10) -> Option<u8> {
    use crate::freq::mhz;
    match f {
        f if (mhz(64)..mhz(300)).contains(&f) => Some(0),
        f if (mhz(300)..mhz(630)).contains(&f) || (mhz(756)..=mhz(1300)).contains(&f) => Some(1),
        _ => None,
    }
}

/// The mic meter (TX-12) from PA0, the mic-level tap stock's VOX reads (decision 76): stock uses the reading's top
/// 8 bits; this many counts is shown as full deviation (to confirm on the bench, with the VOX thresholds' scale:
/// level 1 opens at 101).
pub const MIC_FULL: u16 = 160;

/// PA0's 12-bit reading as % of full deviation, 0-100.
pub fn mic_percent(adc: u16) -> u8 {
    ((adc >> 4).min(MIC_FULL) as u32 * 100 / MIC_FULL as u32) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;
    extern crate std;
    use std::vec::Vec;

    #[test]
    fn end_tones_are_tone_10() {
        let ms = |i: usize| END_TONES[i].iter().map(|s| s.1 as u32).sum::<u32>();
        assert!(END_TONES[0].is_empty());
        assert_eq!((ms(1), ms(2), ms(3), ms(4), ms(5)), (150, 200, 200, 180, 250));
        assert_eq!(END_TONES[5], &[(2475, 250)], "Quindar");
        assert!(END_TONES.iter().flat_map(|t| t.iter()).all(|s| (300..=3000).contains(&s.0)), "inside the voice band");
    }

    #[test]
    fn cw_at_20_wpm() {
        assert_eq!(dit_ms(CW_WPM), 60);
        // "E" is one dit; "T" one dah; "ET": dit, letter gap, dah.
        assert_eq!(Cw::new(b"E", 20).collect::<Vec<_>>(), [(true, 60)]);
        assert_eq!(Cw::new(b"ET", 20).collect::<Vec<_>>(), [(true, 60), (false, 180), (true, 180)]);
        // "A": dit, element gap, dah.
        assert_eq!(Cw::new(b"A", 20).collect::<Vec<_>>(), [(true, 60), (false, 60), (true, 180)]);
        // A word gap is 7 dits: the letter gap's 3 plus 4.
        assert_eq!(Cw::new(b"E E", 20).collect::<Vec<_>>(), [(true, 60), (false, 180), (false, 240), (true, 60)]);
        // PARIS is 50 dits including its word gap: 43 without.
        assert_eq!(cw_ms(b"PARIS", 20), 43 * 60);
        // A callsign: its characters only, lower case too, unknown ones skipped.
        assert_eq!(cw_ms(b"va7xyz/p", 20), cw_ms(b"VA7XYZ/P", 20));
        assert_eq!(cw_ms(b"E#E", 20), cw_ms(b"EE", 20));
    }

    #[test]
    fn dtmf_symbols() {
        assert_eq!(dtmf_symbol(b'5'), Some('5'));
        assert_eq!(dtmf_symbol(b'D'), Some('D'), "DTMF-11: BACK sends D");
        assert_eq!(dtmf_symbol(b'#'), Some('#'));
        assert_eq!(dtmf_symbol(b'E'), None);
    }

    #[test]
    fn counter() {
        assert_eq!(counter_agree(mhz(146) + 52_000, mhz(146) + 52_040), Some(mhz(146) + 52_020));
        assert_eq!(counter_agree(mhz(146) + 52_000, mhz(146) + 52_050), None, "500 Hz apart: not the same carrier");
        assert_eq!(counter_chip(mhz(146)), Some(0));
        assert_eq!(counter_chip(mhz(446)), Some(1));
        assert_eq!(counter_chip(mhz(700)), None, "the chip's gap");
        assert_eq!(counter_chip(mhz(1296)), Some(1));
        assert_eq!(counter_chip(mhz(28)), None, "HF needs the relay");
    }

    #[test]
    fn mic_meter() {
        assert_eq!(mic_percent(0), 0);
        assert_eq!(mic_percent(80 << 4), 50);
        assert_eq!(mic_percent(0xFFF), 100);
    }

    #[test]
    fn tails_and_dtmf_timing() {
        assert_eq!(tail_kind(0, 2), TailKind::Tone55, "no sub-tone: 55 Hz whatever the phase");
        assert_eq!(tail_kind(885, 0), TailKind::Tone55);
        assert_eq!(tail_kind(885, 2), TailKind::Shift(2), "CTCSS: the phase shift");
        assert_eq!(tail_kind(0x8000 | 0o23, 2), TailKind::Shift(0), "DCS: the turn-off code, phase or not");
        assert_eq!(dtmf_seq_ms(4, 100, 300), 1100);
    }
}
