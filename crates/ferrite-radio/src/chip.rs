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
    match s.rx_tone {
        // TONE-6: no tone to lose, so the detector listens for the 55 Hz tail instead (stock: REG_07 0x21CD).
        0 if s.rx_tail => {
            bk.ctcss_rx(550);
            bk.tail_detect();
        }
        0 => bk.subtone_off(),
        t if t & 0x8000 != 0 => bk.dcs_rx(t & 0x3FFF, t & 0x4000 != 0),
        t => bk.ctcss_rx(t),
    }
    bk.af_out(AfOut::Mute);
    bk.rx_on();
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
