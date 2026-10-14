//! Transmit: only with the `tx` feature.
//!
//! The firmware decides whether and where it may transmit (band edges, licence, the PA path on the RF board); this
//! module only does what it is told. [`Bk4829::tx_on`] keys the chip on the frequency already set;
//! [`Bk4829::tx_off`] returns to receive.

use crate::{int, reg, tone_word, Bk4829, Bus, Fsk};

/// REG_30 values stock uses for transmit: VCO calibration, PLL, PA gain, transmit DSP, with the microphone ADC
/// (voice) or with the AF DAC (tones and data, microphone off).
const POWER_TX_MIC: u16 = 0xC1FE;
const POWER_TX_TONE: u16 = 0xC3FA;
const POWER_RX: u16 = 0xBFF1;

/// Tone 1's gain for a transmitted single tone (tone burst, end tone, CW ID): stock V0.29's REG_70 0x9700
/// (0x0801F33C: `(R & 0x7F) | 0x9700`, with REG_30 mode 4). V0.27 sent 0xBC00 (gain 0x3C, 0x0801EB8C), about 8 dB
/// more; V0.29 lowered it, as it did the local beep ([`crate::BEEP_GAIN`]).
pub const TONE_TX_GAIN: u8 = 0x17;
/// Both generators' gain for a DTMF pair: stock's REG_70 0xC5C5 (0x0801DFA8).
pub const DTMF_GAIN: u8 = 0x45;
/// REG_47 for the sidetone while tones are sent: stock's AF route 3 (the generator to the speaker).
const AF_OUT_SIDETONE: u16 = 0x6342;

/// Tail elimination's signal (TONE-5), as stock sends it at unkey (0x0801D078).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tail {
    /// No sub-tone on the transmission: the 55 Hz tail tone from the CTCSS generator, at deviation `dev`
    /// (stock: REG_51 0x9000 | calibration byte 1, REG_07 0x0471).
    Tone55 { dev: u8 },
    /// A CTCSS or DCS transmission: REG_52 bit 15, with bits 14:13 the CTCSS phase shift (1 = 120°, 2 = 180°,
    /// 3 = 240°) or 0, the 134.4 Hz turn-off code a DCS transmission needs.
    Shift { phase: u8 },
}

/// What modulates the carrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Mic,
    /// The tone generators or the FSK modem; microphone off.
    Tones,
}

impl<B: Bus> Bk4829<B> {
    /// Key the transmitter (stock: REG_30 0, then the transmit value; the 0-to-1 of bit 15 recalibrates the VCO).
    pub fn tx_on(&mut self, src: Source) {
        self.write(reg::POWER, 0);
        self.write(reg::POWER, if src == Source::Mic { POWER_TX_MIC } else { POWER_TX_TONE });
    }

    /// Unkey: PA control off, then back to receive.
    pub fn tx_off(&mut self) {
        let pa = self.read(reg::PA);
        self.write(reg::PA, pa & !0x80);
        self.write(reg::POWER, 0x0200);
        self.write(reg::POWER, POWER_RX);
    }

    /// PA control output: `bias` 0-255 (0-3.2 V on PACTL), gains 0-7, `on` enables the output.
    pub fn pa(&mut self, bias: u8, gain1: u8, gain2: u8, on: bool) {
        self.write(reg::PA, (bias as u16) << 8 | (on as u16) << 7 | (gain1 as u16 & 7) << 3 | (gain2 as u16 & 7));
    }

    /// Deviation, 0-0xFFF (both voice and sub-tone).
    pub fn deviation(&mut self, dev: u16) {
        let v = self.read(reg::DEVIATION);
        self.write(reg::DEVIATION, (v & 0xF000) | 1 << 12 | (dev & 0x0FFF));
    }

    /// Microphone sensitivity 0-63 (0.5 dB steps).
    pub fn mic_gain(&mut self, g: u8) {
        let v = self.read(reg::MIC);
        self.write(reg::MIC, (v & !0x3F) | (g as u16 & 0x3F));
    }

    /// Send a CTCSS tone (tenths of a Hz) with the transmission.
    pub fn ctcss_tx(&mut self, dhz: u16) {
        self.ctcss_rx(dhz);
        let v = self.read(reg::SUBTONE);
        self.write(reg::SUBTONE, v | 0x8000);
    }

    /// Send a DCS code (octal digits' value) with the transmission.
    pub fn dcs_tx(&mut self, code: u16, inverted: bool) {
        self.dcs_rx(code, inverted);
        let v = self.read(reg::SUBTONE);
        self.write(reg::SUBTONE, v | 0x8000);
    }

    /// Tail elimination while still keyed (stock 0x0801D078), held until `None` ends it; stock keeps the carrier
    /// up 400 ms with it.
    pub fn tail_tx(&mut self, t: Option<Tail>) {
        let low = self.read(reg::CTCSS_THRESH) & 0x1FFF;
        match t {
            Some(Tail::Tone55 { dev }) => {
                self.write(reg::SUBTONE, 0x9000 | (dev as u16 & 0x7F));
                self.write(reg::CTC, 0x0471);
                self.write(reg::CTCSS_THRESH, low);
            }
            Some(Tail::Shift { phase }) => self.write(reg::CTCSS_THRESH, low | 0x8000 | (phase as u16 & 3) << 13),
            None => {
                self.write(reg::SUBTONE, 0);
                self.write(reg::CTCSS_THRESH, low);
            }
        }
    }

    /// One or two tones (Hz; 0 = off) from the tone generators, with gains 0-127: a 1750 Hz burst, a DTMF pair.
    /// Send with [`Source::Tones`].
    pub fn tones(&mut self, f1: u32, g1: u8, f2: u32, g2: u8) {
        self.write(reg::TONE1_FREQ, tone_word(f1));
        self.write(reg::TONE2_FREQ, tone_word(f2));
        let t = ((f1 != 0) as u16) << 15 | (g1 as u16 & 0x7F) << 8 | ((f2 != 0) as u16) << 7 | (g2 as u16 & 0x7F);
        self.write(reg::TONE, t);
    }

    /// A single tone at [`TONE_TX_GAIN`] (`hz` 0: tones off). While keyed, switch with [`Bk4829::tx_on`] to
    /// [`Source::Tones`] first, as stock moves REG_30 to its mode 4 for any internal tone.
    pub fn tone_tx(&mut self, hz: u32) {
        if hz == 0 {
            self.write(reg::TONE, 0); // stock's "tones off" (0x0801EB4C)
        } else {
            self.tones(hz, TONE_TX_GAIN, 0, 0);
        }
    }

    /// A DTMF symbol's pair at [`DTMF_GAIN`] each (0-9, A-D, *, #); false for anything else, nothing changed.
    pub fn dtmf_tx(&mut self, symbol: char) -> bool {
        let Some((row, col)) = dtmf_pair(symbol) else { return false };
        self.tones(row, DTMF_GAIN, col, DTMF_GAIN);
        true
    }

    /// Hear the tones being sent (stock's mode 4 sidetone, AF route 3), or not (AF muted).
    pub fn sidetone(&mut self, on: bool) {
        if on {
            self.write(reg::AF_OUT, AF_OUT_SIDETONE);
        } else {
            self.af_out(crate::AfOut::Mute);
        }
    }

    /// Load a packet (words, first sent first) and send it: FIFO cleared, the done interrupt enabled, transmit
    /// started. The transmitter must be on ([`Source::Tones`]); wait for [`int::FSK_TX_DONE`], then
    /// [`Bk4829::fsk_tx_end`]. Up to 128 words fit the FIFO at once.
    pub fn fsk_send(&mut self, f: &Fsk, words: &[u16]) {
        let c = self.fsk_ctrl(f);
        self.write(reg::INT_EN, int::FSK_TX_DONE);
        self.write(reg::FSK_CTRL, c | 1 << 15);
        self.write(reg::FSK_CTRL, c);
        for &w in words {
            self.write(reg::FSK_FIFO, w);
        }
        self.write(reg::FSK_CTRL, c | 1 << 11);
    }

    /// After the packet: interrupts cleared and off, transmit off in the modem.
    pub fn fsk_tx_end(&mut self, f: &Fsk) {
        self.write(reg::INT, 0);
        self.write(reg::INT_EN, 0);
        let c = self.fsk_ctrl(f);
        self.write(reg::FSK_CTRL, c);
    }
}

/// The DTMF pair (row Hz, column Hz) for a symbol: 0-9, A-D, *, #.
pub fn dtmf_pair(symbol: char) -> Option<(u32, u32)> {
    const ROW: [u32; 4] = [697, 770, 852, 941];
    const COL: [u32; 4] = [1209, 1336, 1477, 1633];
    let i = b"123A456B789C*0#D".iter().position(|&c| c as char == symbol.to_ascii_uppercase())?;
    Some((ROW[i / 4], COL[i % 4]))
}
