//! Each BK4829 set-up the Radio task writes, as one function over the driver: the register recipes are the
//! driver's (stock's values, uradio's `bk4829`), the order and the choices are the Radio task's. The firmware calls
//! these on its bit-banged bus; the tests on a recording bus.

use bk4829::{AfOut, Bandwidth, Bk4829, Bus, Demod, Fsk};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::same_rx;

/// What a chip needs to receive a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxSetup {
    /// Where the chip tunes (the clarifier already applied), 10 Hz units.
    pub freq: Freq10,
    pub narrow: bool,
    /// AM (receive only, TX-20).
    pub am: bool,
    /// The receive tone word (CHAN-3): CTCSS in 0.1 Hz, bit 15 DCS, bit 14 inverted; 0 none.
    pub rx_tone: u16,
    /// Tail elimination on receive (TONE-6): with no receive tone, the 55 Hz tail mutes.
    pub rx_tail: bool,
}

/// Tune a chip to receive (`Radio::tune_chip`): awake, the frequency, the bandwidth, FM or AM (stock's modes 7 and
/// 6; VFO-20: AM keeps the wide filter), the sub-tone detector, audio muted (the audio decision unmutes the one
/// heard), then receive on.
pub fn tune<B: Bus>(bk: &mut Bk4829<B>, s: &RxSetup) {
    bk.wake();
    bk.set_freq(s.freq * 10);
    bk.bandwidth(if s.narrow { Bandwidth::Narrow } else { Bandwidth::Wide });
    bk.demod(if s.am { Demod::Am } else { Demod::Fm });
    subtone(bk, s.rx_tone, s.rx_tail);
    bk.af_out(AfOut::Mute);
    bk.af_gain_rx(); // REG_48 0xB2BF, stock's receive level (RX-* / SND-2: the knob is the volume)
    bk.rx_on();
}

/// The slot's own sub-tone detector (CHAN-3, TONE-6): set by [`tune`], and put back after a tone hint's probe.
pub fn subtone<B: Bus>(bk: &mut Bk4829<B>, rx_tone: u16, rx_tail: bool) {
    match rx_tone {
        // TONE-6: no tone to lose, so the detector listens for the 55 Hz tail instead (stock: REG_07 0x21CD).
        0 if rx_tail => {
            bk.ctcss_rx(550);
            bk.tail_detect();
        }
        0 => bk.subtone_off(),
        t if t & 0x8000 != 0 => bk.dcs_rx(t & 0x3FFF, t & 0x4000 != 0),
        t => bk.ctcss_rx(t),
    }
}

/// The squelch thresholds (SQL-1): REG_78 (RSSI open/close) and REG_4F (noise), from
/// [`ferrite_core::sqcal::SqCal::regs`], written as stock's 0x0801E64C does. The glitch thresholds stay as the
/// chip's start-up left them, as stock's do.
pub fn squelch<B: Bus>(bk: &mut Bk4829<B>, regs: (u16, u16)) {
    bk.write(bk4829::reg::SQ_RSSI, regs.0);
    bk.write(bk4829::reg::SQ_NOISE, regs.1);
}

/// The tone hint's probe (TSRCH-12): the detectors measure whatever is on air; audio stays as it is (muted).
pub fn tone_probe<B: Bus>(bk: &mut Bk4829<B>) {
    bk.subtone_scan_on();
}

/// What the scan detectors heard as a tone word (CHAN-3's form; TSRCH-13), from [`Bk4829::subtone_scan`]: CTCSS
/// snapped to the nearest of `ctcss` within 1 Hz (else to 0.1 Hz, 60-260 Hz only); DCS matched against `dcs` in
/// any of its 23 rotations, normal before inverted (the complement). None if nothing usable.
#[inline(never)] // in .slow_text (memory.x): once per probe
pub fn heard_tone(ctcss_dhz: Option<u16>, dcs_word: Option<u32>, ctcss: &[u16], dcs: &[u16]) -> Option<u16> {
    if let Some(w) = dcs_word {
        for inverted in [false, true] {
            let mut x = if inverted { !w & 0x7F_FFFF } else { w & 0x7F_FFFF };
            for _ in 0..23 {
                // The word carries its code: data bits 0-8, then the fixed 100 in bits 9-11. Only a rotation
                // with that marker is checked, with one codeword: at most 46 a read, not one per code.
                let c = (x & 0x1FF) as u16;
                if x & 0xE00 == 0x800 && bk4829::dcs_codeword(c, false) == x && dcs.contains(&c) {
                    return Some(0x8000 | if inverted { 0x4000 } else { 0 } | c);
                }
                x = (x >> 1) | (x & 1) << 22;
            }
        }
    }
    let d = ctcss_dhz.filter(|d| (600..=2600).contains(d))?;
    let near = ctcss.iter().copied().min_by_key(|&t| t.abs_diff(d));
    Some(near.filter(|t| t.abs_diff(d) <= 10).unwrap_or(d))
}

/// A local beep's start on a chip (SND-11, stock 0x0801EB8C): woken first if it was asleep, tone 1 at `tone_gain`
/// (the beep volume, SND-3: `tonegen::beep_gain`; stock's is 0x17). Returns REG_48 for [`beep_end`].
pub fn beep_start<B: Bus>(bk: &mut Bk4829<B>, hz: u32, tone_gain: u8, asleep: bool) -> u16 {
    if asleep {
        bk.wake();
    }
    bk.beep_on_gain(hz, tone_gain)
}

/// The beep's end: tone off, REG_48 back, then the chip asleep again or back to receive.
pub fn beep_end<B: Bus>(bk: &mut Bk4829<B>, gain: u16, asleep: bool) {
    bk.beep_off(gain);
    if asleep {
        bk.sleep();
    } else {
        bk.rx_on();
    }
}

/// The weather channels, 162.400-162.550 MHz (WX-1).
pub const WX: core::ops::RangeInclusive<Freq10> = mhz(162) + 40_000..=mhz(162) + 55_000;

/// The modem in its NOAA SAME receive mode (DEC-19; BK4819 application note, nobody's firmware uses it yet, so
/// every value is bench-confirm, roadmap 6b): REG_58 receive mode 000 with bandwidth 010 and receive gain 3 (bits
/// 7:6 set as in both of stock's modem modes), the rate in Tone 2's word (520.83 baud x 10.32444), the four sync
/// bytes `ZCZC`, no CRC (stock's REG_5C with bit 6 clear), and the longest header's length.
pub const SAME_FSK: Fsk = Fsk {
    mode: 0x03C4,
    tone_word: ((same_rx::BAUD_MILLI as u64 * 1_032_444 + 50_000_000) / 100_000_000) as u16,
    tone_gain: 0,
    preamble: 16,
    sync: same_rx::SYNC,
    sync4: true,
    crc: 0xA730 & !(1 << 6),
    scramble: false,
    len: same_rx::MAX_LEN as u16,
};

/// The SAME modem listening for a burst's sync word.
pub fn same_arm<B: Bus>(bk: &mut Bk4829<B>) {
    bk.fsk_setup(&SAME_FSK);
    bk.fsk_rx(&SAME_FSK);
}

/// Where each chip sits while counting (TSRCH-10): its band's middle, for the front end the path selects.
pub const COUNTER_TUNE: [Freq10; 2] = [mhz(146), mhz(446)];

/// The SI4732's input path for its mode, as stock (BCR-20, `analysis/audio_path.md` §2): FM broadcast PA7 = PC5 = 0,
/// AM (and SW, SSB, CW) PA7 = PC5 = 1, the LNA path. The pins' levels, PA7 first.
pub fn si_input(am: bool) -> [bool; 2] {
    [am, am]
}

/// REG_30 read back after a key-up with the microphone (TX-7): stock's transmit value.
#[cfg(feature = "tx")]
pub const REG30_TX_MIC: u16 = 0xC1FE;

/// What the keyed chip must hold before the PA is driven (TX-7): the TX frequency word Ferrite wrote (10 Hz units,
/// REG_38/39) and REG_3B (the crystal register Ferrite never writes: the value read after the chip's start-up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxExpect {
    pub freq: Freq10,
    pub reg3b: u16,
}

/// The register a TX-7 read-back found wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadBack {
    /// REG_38/39: the synthesized frequency.
    Freq,
    /// REG_30: not keyed with the microphone.
    Power,
    /// REG_3B: the crystal register changed.
    Xtal,
    /// REG_36: the PA bias didn't take.
    Bias,
}

/// `Act::ChipTx` (TX-7, TX-23): the chip already holds the TX frequency and tone; key it with the microphone, then
/// read back REG_30, the frequency (REG_38/39) and REG_3B, write stock's REG_36 for the level ({0x7F, 0x57,
/// 0x57}[level], `analysis/calibration.md`) and read it back too, all before the PA is driven. Stock reads nothing
/// back; this is Ferrite's (decision 97). Err names the first register that differs: the caller goes to the safe
/// state with the PA at zero.
#[cfg(feature = "tx")]
pub fn key_mic<B: Bus>(bk: &mut Bk4829<B>, high: bool, want: &TxExpect) -> Result<(), ReadBack> {
    bk.tx_on(bk4829::tx::Source::Mic);
    if bk.read(0x30) != REG30_TX_MIC {
        return Err(ReadBack::Power);
    }
    let word = bk.read(0x38) as u32 | (bk.read(0x39) as u32) << 16;
    if word != want.freq {
        return Err(ReadBack::Freq);
    }
    if bk.read(0x3B) != want.reg3b {
        return Err(ReadBack::Xtal);
    }
    let bias = if high { 0x7F } else { 0x57 };
    bk.write(0x36, bias);
    if bk.read(0x36) != bias {
        return Err(ReadBack::Bias);
    }
    Ok(())
}

/// While keyed (TONE-13, DTMF-7, DTMF-11): the tone burst or a DTMF symbol on the keyed chip, as stock sends any
/// internal tone: the generator set, REG_30 to tone transmit (the microphone off) and the sidetone as asked; both
/// None goes back to the microphone. Returns (a tone is sending, the microphone read-back is good).
#[cfg(feature = "tx")]
pub fn keyed_tone<B: Bus>(bk: &mut Bk4829<B>, hz: Option<u16>, dtmf: Option<char>, heard: bool) -> (bool, bool) {
    use bk4829::tx::Source;
    let sending = match (hz, dtmf) {
        (Some(hz), _) => {
            bk.tone_tx(hz as u32);
            true
        }
        (None, Some(d)) => bk.dtmf_tx(d),
        (None, None) => false,
    };
    if sending {
        bk.tx_on(Source::Tones);
        bk.sidetone(heard);
        return (true, true);
    }
    bk.tone_tx(0);
    bk.sidetone(false);
    bk.tx_on(Source::Mic);
    (false, bk.read(0x30) == REG30_TX_MIC)
}

/// The tail sent at the unkey (TONE-6, TONE-9): the 55 Hz tone at the driver's sub-tone gain (74, as its CTCSS)
/// where stock takes the calibration's byte (bench-confirm), or REG_52's phase shift (the DCS turn-off code on DCS).
#[cfg(feature = "tx")]
pub fn tail(tx_tone: u16, phase: u8) -> bk4829::tx::Tail {
    use bk4829::tx::Tail;
    match ferrite_core::tonegen::tail_kind(tx_tone, phase) {
        ferrite_core::tonegen::TailKind::Tone55 => Tail::Tone55 { dev: 74 },
        ferrite_core::tonegen::TailKind::Shift(p) => Tail::Shift { phase: p },
    }
}
