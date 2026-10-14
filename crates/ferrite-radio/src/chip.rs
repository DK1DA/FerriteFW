//! Each BK4829 set-up the Radio task writes, as one function over the driver: the register recipes are the
//! driver's (stock's values, uradio's `bk4829`), the order and the choices are the Radio task's. The firmware calls
//! these on its bit-banged bus; the tests on a recording bus.

use bk4829::{AfOut, Bandwidth, Bk4829, Bus, Demod, Fsk};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::same_rx;
use ferrite_core::settle::Settle;

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

/// Tune a chip to receive with no settle wait (stock's order): [`wake`], then [`tune_rest`]. The Radio task does
/// the two apart, with PWR-21's wait between them when the chip was asleep.
pub fn tune<B: Bus>(bk: &mut Bk4829<B>, s: &RxSetup) {
    bk.wake();
    tune_rest(bk, s);
}

/// The receive set-up after the wake (`Radio::tune_chip`): audio muted first (the audio decision unmutes the one
/// heard; a receiving chip being retuned is silent through it), the frequency, the bandwidth, FM or AM (stock's
/// modes 7 and 6; VFO-20: AM keeps the wide filter), the sub-tone detector, the receive level, then receive on.
pub fn tune_rest<B: Bus>(bk: &mut Bk4829<B>, s: &RxSetup) {
    bk.af_out(AfOut::Mute);
    bk.set_freq(s.freq * 10);
    bk.bandwidth(if s.narrow { Bandwidth::Narrow } else { Bandwidth::Wide });
    bk.demod(if s.am { Demod::Am } else { Demod::Fm });
    subtone(bk, s.rx_tone, s.rx_tail);
    bk.af_gain_rx(); // REG_48 0xB2BF, stock's receive level (RX-* / SND-2: the knob is the volume)
    bk.rx_on();
}

/// A wait that lets the rest of the firmware run (the Radio task's `Timer::after_millis`; PWR-21's waits are a few
/// ms on the highest-priority executor, too long to spin).
#[allow(async_fn_in_trait)]
pub trait Delay {
    async fn ms(&mut self, ms: u32);
}

/// PWR-21 (decision 119): tune a chip with the wake settle. `woke` is when its REG_37 supplies came on (the Radio
/// task's record), None if it is asleep: then they come on here, at `now`. Whatever is left of `settle.wake_ms`
/// since then is waited before [`tune_rest`] (nothing for a chip woken a tick early by the hop, or long awake).
/// Returns the wake time to keep.
pub async fn tune_settled<B: Bus, D: Delay>(bk: &mut Bk4829<B>, d: &mut D, s: &RxSetup, settle: Settle, woke: Option<u32>, now: u32) -> u32 {
    let (at, left) = wake_start(bk, settle, woke, now);
    if left > 0 {
        d.ms(left).await;
    }
    tune_rest(bk, s);
    at
}

/// [`tune_settled`]'s first half (the Radio task runs the halves apart, around its own timer): REG_37's supplies
/// on if `woke` is None, and returns (the wake time, the ms of settle left at `now`).
pub fn wake_start<B: Bus>(bk: &mut Bk4829<B>, settle: Settle, woke: Option<u32>, now: u32) -> (u32, u32) {
    let at = woke.unwrap_or_else(|| {
        bk.wake();
        now
    });
    (at, settle.wake_left(at, now))
}

/// PWR-21: a chip to sleep with the sleep settle: [`sleep_begin`] (muted, REG_30 off), `settle.sleep_ms`, then
/// REG_37's supplies off. With 0 it is stock's two writes back to back (after the mute).
pub async fn sleep_settled<B: Bus, D: Delay>(bk: &mut Bk4829<B>, d: &mut D, settle: Settle) {
    sleep_begin(bk);
    if settle.sleep_ms > 0 {
        d.ms(settle.sleep_ms as u32).await;
    }
    bk.ldo_sleep();
}

/// PWR-21, the sleep's first half: the AF route muted, then REG_30 off. The Radio task waits the sleep settle, then
/// [`Bk4829::ldo_sleep`]. Muting first means the chip never goes off, nor comes back, with its audio routed.
pub fn sleep_begin<B: Bus>(bk: &mut Bk4829<B>) {
    bk.af_out(AfOut::Mute);
    bk.off();
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

/// The weather channels, 162.400-162.550 MHz (WX-1). A `static`: the Radio task checks it every tick, and only named
/// statics stay in the firmware's fast flash (unnamed constants go to the slow part: firmware/memory.x).
pub static WX: core::ops::RangeInclusive<Freq10> = mhz(162) + 40_000..=mhz(162) + 55_000;

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

/// What the transmitting chip is set to before the path plan keys it (TX-23).
#[cfg(feature = "tx")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxSetup {
    /// The TX frequency, 10 Hz units.
    pub freq: Freq10,
    pub narrow: bool,
    /// The sub-tone sent: the tone word (CTCSS in 0.1 Hz, bit 15 DCS, bit 14 inverted; 0 none).
    pub tone: u16,
    /// Its deviation, REG_51[6:0] (TONE-1, TONE-16: `TxCal::tone_dev`).
    pub tone_dev: u8,
    /// REG_7D[5:0], the mic sensitivity (VOX-15: `vox::mic_reg`; a data key-up `vox::MIC_DATA`, VOX-6).
    pub mic: u8,
}

/// The transmitting chip's set-up before the path plan keys it (TX-23), in stock's order (TX start 0x0801EAF8,
/// TX audit F1, F2, F7, F14): out of receive (REG_30 0, stock's 50 us), REG_47 0x6142 (stock's AF route while
/// keyed; nicFW writes 0x6042), the TX frequency, REG_43 with it (0x3028 wide, 0x4008 narrow; 0x0801DE38-0x0801DE52:
/// with one chip awake at a time, decision 111, a cross-band split's TX chip was last set for another slot, M7),
/// the sub-tone with the calibration's deviation, the mic gain (REG_7D), then stock's REG_40 deviation
/// (`(R & 0xF000) | 0x50A`). The chip keeps its last receive mode bits (REG_31 scrambler off).
#[cfg(feature = "tx")]
pub fn tx_setup<B: Bus>(bk: &mut Bk4829<B>, s: &TxSetup) {
    bk.tx_start();
    bk.af_out(AfOut::Normal);
    bk.set_freq(s.freq * 10);
    bk.bandwidth(if s.narrow { Bandwidth::Narrow } else { Bandwidth::Wide });
    match s.tone {
        0 => bk.subtone_off(),
        t if t & 0x8000 != 0 => bk.dcs_tx(t & 0x3FFF, t & 0x4000 != 0),
        t => bk.ctcss_tx(t),
    }
    if s.tone != 0 {
        bk.subtone_gain(s.tone_dev);
    }
    bk.mic_gain(s.mic);
    bk.tx_deviation();
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
    /// The keyed slot's bandwidth: stock keys through REG_30 0x0200 on wide, 0 on narrow (F3).
    pub narrow: bool,
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
    bk.tx_on(bk4829::tx::Source::Mic, !want.narrow);
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
/// None goes back to the microphone with the key-up's AF route (REG_47 0x6142, F14). `narrow`: the keyed slot's
/// bandwidth, for stock's REG_30 step (F3). Returns (a tone is sending, the microphone read-back is good).
#[cfg(feature = "tx")]
pub fn keyed_tone<B: Bus>(bk: &mut Bk4829<B>, hz: Option<u16>, dtmf: Option<char>, heard: bool, narrow: bool) -> (bool, bool) {
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
        bk.tx_on(Source::Tones, !narrow);
        bk.sidetone(heard);
        return (true, true);
    }
    bk.tone_tx(0);
    bk.af_out(AfOut::Normal);
    bk.tx_on(Source::Mic, !narrow);
    (false, bk.read(0x30) == REG30_TX_MIC)
}

/// The tail sent at the unkey (TONE-5, TONE-6, TONE-9): the 55 Hz tone at the calibration's CTCSS byte (`dev`,
/// `TxCal::tail_dev`; stock 0x0801D078, TX audit F7), or REG_52's phase shift (the DCS turn-off code on DCS).
#[cfg(feature = "tx")]
pub fn tail(tx_tone: u16, phase: u8, dev: u8) -> bk4829::tx::Tail {
    use bk4829::tx::Tail;
    match ferrite_core::tonegen::tail_kind(tx_tone, phase) {
        ferrite_core::tonegen::TailKind::Tone55 => Tail::Tone55 { dev },
        ferrite_core::tonegen::TailKind::Shift(p) => Tail::Shift { phase: p },
    }
}

/// TERM-33's `flat` test key-up (decision 132): the four registers as they were, for [`tx_test_restore`].
#[cfg(feature = "tx")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestRegs {
    pub r2b: u16,
    pub r19: u16,
    pub r7d: u16,
    pub r50: u16,
}

/// REG_2B[2:0]: the TX HPF300, LPF1 and pre-emphasis, each 1 = off (the register table).
#[cfg(feature = "tx")]
pub const TX_FILTERS_OFF: u16 = 0b111;
/// REG_19[15]: MIC AGC, 1 = off (the register table; the BK4819 app note turns it off for DMR).
#[cfg(feature = "tx")]
pub const MIC_AGC_OFF: u16 = 1 << 15;
/// REG_7D[6]: the AF level controller (ALC), 1 = off. The BK4829's table has it here; the BK4819 app note's
/// REG_4B[5] is the older chip's place for it, and REG_4B is left alone.
#[cfg(feature = "tx")]
pub const ALC_OFF: u16 = 1 << 6;
/// REG_50[15]: AF TX mute. Kept clear: DAC1 reaches the modulator through the mic input, so this mute would silence
/// the test signal with the mic (V0.29 sets it only around its internal FSK burst).
#[cfg(feature = "tx")]
pub const TX_MUTE: u16 = 1 << 15;

/// TERM-33 `flat`, after [`tx_setup`] and before the key-up: the TX audio filters, MIC AGC and the ALC off and the
/// TX mute clear, so DAC1's test signal reaches the modulator as it is. REG_7D[5:0] is the data key-up's
/// [`ferrite_core::vox::MIC_DATA`] from [`tx_setup`]. Returns what was there.
#[cfg(feature = "tx")]
pub fn tx_test_flat<B: Bus>(bk: &mut Bk4829<B>) -> TestRegs {
    let was = TestRegs { r2b: bk.read(bk4829::reg::AF_FILTERS), r19: bk.read(0x19), r7d: bk.read(bk4829::reg::MIC), r50: bk.read(0x50) };
    bk.write(bk4829::reg::AF_FILTERS, was.r2b | TX_FILTERS_OFF);
    bk.write(0x19, was.r19 | MIC_AGC_OFF);
    bk.write(bk4829::reg::MIC, was.r7d | ALC_OFF);
    if was.r50 & TX_MUTE != 0 {
        bk.write(0x50, was.r50 & !TX_MUTE);
    }
    was
}

/// At the unkey of a `flat` test: the four registers as [`tx_test_flat`] found them.
#[cfg(feature = "tx")]
pub fn tx_test_restore<B: Bus>(bk: &mut Bk4829<B>, r: &TestRegs) {
    bk.write(bk4829::reg::AF_FILTERS, r.r2b);
    bk.write(0x19, r.r19);
    bk.write(bk4829::reg::MIC, r.r7d);
    bk.write(0x50, r.r50);
}
