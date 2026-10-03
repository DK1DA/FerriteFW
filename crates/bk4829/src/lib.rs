//! # bk4829
//!
//! A driver for the Beken BK4829 FM transceiver (two in the RT-950 Pro). `no_std`, no allocation. Register meanings
//! are from Beken's register table (DRT01-230606-C01); the start-up sequence and the values for each feature are
//! the ones Radtel's stock firmware writes, so the chip starts in a state known to work on this radio.
//!
//! **Transmit only with the `tx` feature.** Keying, PA control, sending sub-tones, DTMF and FSK exist only in
//! builds that enable it ([`tx`]); a receive-only build cannot contain them. Nothing here can key a transmitter
//! without it: receive never sets REG_30's transmit bits or REG_51's transmit sub-tone enable.
//!
//! The driver talks through [`Bus`]: two calls, a register write and a register read. [`wire::ThreeWire`] is the
//! bit-banged bus with both chips on it. The driver keeps no state of its own (read-modify-writes read the chip),
//! so it can be made for a call and dropped.
//!
//! Units: frequencies in Hz; sub-tones in tenths of a Hz (88.5 Hz = 885); RSSI in half dB.

#![no_std]

pub mod wire;

#[cfg(feature = "tx")]
pub mod tx;

/// Register access.
pub trait Bus {
    fn write(&mut self, reg: u8, value: u16);
    fn read(&mut self, reg: u8) -> u16;
}

impl<B: Bus> Bus for &mut B {
    fn write(&mut self, reg: u8, value: u16) {
        (**self).write(reg, value)
    }
    fn read(&mut self, reg: u8) -> u16 {
        (**self).read(reg)
    }
}

/// Registers used by name.
pub mod reg {
    pub const ID: u8 = 0x00;
    pub const INT: u8 = 0x02;
    pub const CTC: u8 = 0x07;
    pub const CDCSS: u8 = 0x08;
    pub const DTMF_COEF: u8 = 0x09;
    pub const DTMF_CODE: u8 = 0x0B;
    pub const STATUS: u8 = 0x0C;
    pub const DTMF: u8 = 0x24;
    pub const AF_FILTERS: u8 = 0x2B;
    pub const POWER: u8 = 0x30;
    pub const FUNC: u8 = 0x31;
    pub const PA: u8 = 0x36;
    pub const LDO: u8 = 0x37;
    pub const FREQ_LO: u8 = 0x38;
    pub const FREQ_HI: u8 = 0x39;
    pub const INT_EN: u8 = 0x3F;
    pub const DEVIATION: u8 = 0x40;
    pub const RF_FILTER: u8 = 0x43;
    pub const AF_OUT: u8 = 0x47;
    pub const AF_GAIN: u8 = 0x48;
    pub const SQ_GLITCH_CLOSE: u8 = 0x4D;
    pub const SQ_GLITCH_OPEN: u8 = 0x4E;
    pub const SQ_NOISE: u8 = 0x4F;
    pub const SUBTONE: u8 = 0x51;
    pub const CTCSS_THRESH: u8 = 0x52;
    pub const FSK_MODE: u8 = 0x58;
    pub const FSK_CTRL: u8 = 0x59;
    pub const FSK_SYNC01: u8 = 0x5A;
    pub const FSK_SYNC23: u8 = 0x5B;
    pub const FSK_CRC: u8 = 0x5C;
    pub const FSK_LEN: u8 = 0x5D;
    pub const FSK_FIFO: u8 = 0x5F;
    pub const GLITCH: u8 = 0x63;
    pub const NOISE: u8 = 0x65;
    pub const RSSI: u8 = 0x67;
    pub const CDCSS_HI: u8 = 0x69;
    pub const CDCSS_LO: u8 = 0x6A;
    pub const TONE: u8 = 0x70;
    pub const TONE1_FREQ: u8 = 0x71;
    pub const TONE2_FREQ: u8 = 0x72;
    pub const SQ_RSSI: u8 = 0x78;
    pub const MIC: u8 = 0x7D;
}

/// Interrupt flags (REG_02, enables in REG_3F at the same bits).
pub mod int {
    pub const FSK_TX_DONE: u16 = 1 << 15;
    pub const FSK_FIFO_EMPTY: u16 = 1 << 14;
    pub const FSK_RX_DONE: u16 = 1 << 13;
    pub const FSK_FIFO_FULL: u16 = 1 << 12;
    pub const DTMF: u16 = 1 << 11;
    pub const TAIL: u16 = 1 << 10;
    pub const CDCSS_FOUND: u16 = 1 << 9;
    pub const CDCSS_LOST: u16 = 1 << 8;
    pub const CTCSS_FOUND: u16 = 1 << 7;
    pub const CTCSS_LOST: u16 = 1 << 6;
    pub const VOX_FOUND: u16 = 1 << 5;
    pub const VOX_LOST: u16 = 1 << 4;
    pub const SQ_OPEN: u16 = 1 << 3;
    pub const SQ_CLOSE: u16 = 1 << 2;
    pub const FSK_SYNC: u16 = 1 << 1;
}

/// Stock's start-up writes, in its order (two halves around a read-modify-write of REG_40). REG_00 0x8000 then 0
/// is a soft reset; REG_10-14 the AGC table; REG_3F the interrupts stock uses.
const INIT_A: &[(u8, u16)] = &[
    (0x00, 0x8000), (0x00, 0x0000), (0x37, 0x9D1F), (0x13, 0x03DF), (0x12, 0x03DB), (0x11, 0x033A), (0x10, 0x0318),
    (0x14, 0x0210), (0x49, 0x2AB2), (0x7B, 0x73DC), (0x1C, 0x07C0), (0x1D, 0xE555), (0x1E, 0x4C58), (0x1F, 0x865A),
    (0x3E, 0x94C6), (0x3F, 0x07FE), (0x25, 0xC1BA), (0x3A, 0x9A7C), (0x19, 0x1041), (0x28, 0x0B40), (0x29, 0xAA00),
    (0x2A, 0x6600), (0x2C, 0x0022), (0x2F, 0x9890), (0x53, 0x2028), (0x7E, 0x303E), (0x46, 0x6050), (0x4A, 0x5430),
    (0x48, 0xB3BF), (0x49, 0x2AB2), (0x4A, 0x5430), (0x4D, 0xA004), (0x4E, 0x3815), (0x4F, 0x3F3B), (0x77, 0xCCEF),
    (0x7E, 0x303E),
];
/// REG_40 keeps bits 15:12 and takes this deviation.
const INIT_DEVIATION: u16 = 0x4D2;
const INIT_B: &[(u8, u16)] = &[
    (0x7D, 0xE912), (0x48, 0xB3FF), (0x74, 0xDF22), (0x44, 0x91C1), (0x45, 0x3040), (0x75, 0xE61C), (0x54, 0x91C1),
    (0x55, 0x3040),
];

/// Stock's DTMF detector coefficients (REG_09, index in bits 15:12).
const DTMF_COEFS: [u16; 16] = [
    0x006F, 0x106B, 0x2067, 0x3062, 0x4050, 0x5047, 0x603A, 0x702C, 0x8041, 0x9037, 0xA025, 0xB017, 0xC0E4, 0xD0CB,
    0xE0B5, 0xF09F,
];

/// REG_30 values stock uses: receive (VCO calibration, receive chain, AF DAC, PLL, receive DSP).
const POWER_RX: u16 = 0xBFF1;
const POWER_AF_ONLY: u16 = 0x0200;

/// Channel bandwidth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bandwidth {
    /// 25 kHz channels (REG_43 0x3028, stock's).
    Wide,
    /// 12.5 kHz channels (REG_43 0x4008, stock's).
    Narrow,
}

/// What the AF output carries (REG_47 bits 11:8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfOut {
    Mute,
    /// Demodulated audio.
    Normal,
    /// The tone generator (tone 1 enabled), heard on receive.
    Tone,
    /// Any other value of the field (the table calls the rest reserved).
    Raw(u8),
}

/// Squelch thresholds: open when all three pass, close when any fails (hysteresis between the pairs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Squelch {
    /// RSSI to open and to close, half dB on REG_67's scale (dBm = value / 2 - 160).
    pub rssi_open: u8,
    pub rssi_close: u8,
    /// Out-of-band noise to open and to close (REG_65's scale, dB): lower is quieter.
    pub noise_open: u8,
    pub noise_close: u8,
    /// Glitches per 10 ms to open and to close.
    pub glitch_open: u8,
    pub glitch_close: u8,
}

/// The receive status (REG_0C).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub squelch_open: bool,
    pub vox: bool,
    pub ctcss: bool,
    /// The 55 Hz tail tone.
    pub tail: bool,
    pub cdcss: bool,
    pub cdcss_inverted: bool,
    /// An interrupt flag is waiting in REG_02.
    pub interrupt: bool,
}

/// FSK modem settings (REG_58-5E).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fsk {
    /// REG_58: mode, receive gain, preamble type, receive bandwidth (bits 15:1; bit 0, enable, is set by the
    /// driver). Use [`Fsk::FSK2400`], [`Fsk::FFSK1200`] or the table's values.
    pub mode: u16,
    /// Tone 2 (the FSK tone generator) control word and gain, for sending.
    pub tone_word: u16,
    pub tone_gain: u8,
    /// Preamble bytes, 1-16.
    pub preamble: u8,
    /// Sync bytes 0-3; with `sync4` false only 0 and 1 are used.
    pub sync: [u8; 4],
    pub sync4: bool,
    /// REG_5C (CRC enable is bit 6; stock's other bits kept as they are).
    pub crc: u16,
    pub scramble: bool,
    /// Packet length in bytes, 1-2048.
    pub len: u16,
}

impl Fsk {
    /// Stock's 2400 baud FSK: REG_58 0x00C8.
    pub const FSK2400: u16 = 0x00C8;
    /// Stock's FFSK 1200/1800 (transmit and receive), receive gain 3: REG_58 0x3FC2.
    pub const FFSK1200: u16 = 0x3FC2;
}

/// The chip's interrupt flags.
pub type Interrupts = u16;

/// The transceiver, through a bus.
pub struct Bk4829<B: Bus> {
    bus: B,
}

/// Tone 1 / tone 2 frequency word: Hz x 10.32444 (26 MHz crystal family, which the RT-950's chips use: REG_3B/3C
/// reset to 26 MHz and stock's sub-tone words fit it).
pub const fn tone_word(hz: u32) -> u16 {
    ((hz as u64 * 1_032_444 + 50_000) / 100_000) as u16
}

/// CTCSS detector word, tenths of a Hz x 2.064888.
pub const fn ctcss_word(dhz: u16) -> u16 {
    ((dhz as u64 * 2_064_888 + 500_000) / 1_000_000) as u16
}

/// The 23-bit DCS codeword for an octal code written as its digits' value (023 = 0o23 = 19): code in bits 8:0, then
/// the fixed 100, then 11 Golay (23,12) parity bits. Inverted DCS sends the complement.
pub const fn dcs_codeword(code: u16, inverted: bool) -> u32 {
    let data = 0x800 | (code as u32 & 0x1FF);
    let mut r = data << 11;
    let mut i = 22;
    while i >= 11 {
        if r >> i & 1 != 0 {
            r ^= 0xC75 << (i - 11);
        }
        i -= 1;
    }
    let w = data | r << 12;
    if inverted {
        !w & 0x7F_FFFF
    } else {
        w
    }
}

impl<B: Bus> Bk4829<B> {
    pub fn new(bus: B) -> Self {
        Bk4829 { bus }
    }

    pub fn release(self) -> B {
        self.bus
    }

    pub fn write(&mut self, reg: u8, value: u16) {
        self.bus.write(reg, value)
    }

    pub fn read(&mut self, reg: u8) -> u16 {
        self.bus.read(reg)
    }

    fn modify(&mut self, reg: u8, mask: u16, value: u16) {
        let v = self.bus.read(reg);
        self.bus.write(reg, (v & !mask) | (value & mask));
    }

    /// REG_00: 0x4829.
    pub fn id(&mut self) -> u16 {
        self.read(reg::ID)
    }

    /// Soft reset and stock's start-up. The chip is then on, not receiving: call [`Bk4829::rx_on`].
    pub fn init(&mut self) {
        for &(r, v) in INIT_A {
            self.write(r, v);
        }
        self.modify(reg::DEVIATION, 0x0FFF, INIT_DEVIATION);
        for &(r, v) in INIT_B {
            self.write(r, v);
        }
    }

    /// Frequency, Hz (10 Hz steps). Takes effect at the next [`Bk4829::rx_on`] (which recalibrates the VCO).
    pub fn set_freq(&mut self, hz: u32) {
        let n = hz / 10;
        self.write(reg::FREQ_LO, n as u16);
        self.write(reg::FREQ_HI, (n >> 16) as u16);
    }

    pub fn freq(&mut self) -> u32 {
        let lo = self.read(reg::FREQ_LO) as u32;
        let hi = self.read(reg::FREQ_HI) as u32;
        (hi << 16 | lo) * 10
    }

    /// Receive (stock: REG_30 0x0200 then 0xBFF1; the 0-to-1 of bit 15 recalibrates the VCO).
    pub fn rx_on(&mut self) {
        self.write(reg::POWER, POWER_AF_ONLY);
        self.write(reg::POWER, POWER_RX);
    }

    /// Everything in REG_30 off: no receive, no transmit.
    pub fn off(&mut self) {
        self.write(reg::POWER, 0);
    }

    pub fn bandwidth(&mut self, bw: Bandwidth) {
        self.write(reg::RF_FILTER, if bw == Bandwidth::Wide { 0x3028 } else { 0x4008 });
    }

    pub fn af_out(&mut self, out: AfOut) {
        let v = match out {
            AfOut::Mute => 0,
            AfOut::Normal => 1,
            AfOut::Tone => 2,
            AfOut::Raw(v) => v & 15,
        };
        self.modify(reg::AF_OUT, 0x0F00, (v as u16) << 8);
    }

    /// Receive audio level: `gain2` 0-63 (-28 to +3.5 dB, 0.5 dB steps), `dac` 0-15 (about 2 dB steps).
    pub fn af_gain(&mut self, gain2: u8, dac: u8) {
        self.modify(reg::AF_GAIN, 0x03FF, ((gain2 as u16 & 63) << 4) | (dac as u16 & 15));
    }

    /// The AF filters: receive high-pass 300 Hz, low-pass 3 kHz, de-emphasis (off for data modes, which want flat
    /// audio).
    pub fn rx_filters(&mut self, hpf: bool, lpf: bool, deemphasis: bool) {
        let off = (!hpf as u16) << 10 | (!lpf as u16) << 9 | (!deemphasis as u16) << 8;
        self.modify(reg::AF_FILTERS, 0x0700, off);
    }

    pub fn squelch(&mut self, sq: &Squelch) {
        self.write(reg::SQ_RSSI, (sq.rssi_open as u16) << 8 | sq.rssi_close as u16);
        self.write(reg::SQ_NOISE, (sq.noise_close as u16 & 0x7F) << 8 | (sq.noise_open as u16 & 0x7F));
        self.modify(reg::SQ_GLITCH_CLOSE, 0x00FF, sq.glitch_close as u16);
        self.modify(reg::SQ_GLITCH_OPEN, 0x00FF, sq.glitch_open as u16);
    }

    /// RSSI, half dB: dBm = value / 2 - 160.
    pub fn rssi(&mut self) -> u16 {
        self.read(reg::RSSI) & 0x1FF
    }

    /// RSSI in dBm.
    pub fn rssi_dbm(&mut self) -> i16 {
        self.rssi() as i16 / 2 - 160
    }

    /// Out-of-band (above 10 kHz) noise, dB.
    pub fn noise(&mut self) -> u8 {
        (self.read(reg::NOISE) & 0x7F) as u8
    }

    /// Glitches in the last 10 ms.
    pub fn glitch(&mut self) -> u8 {
        self.read(reg::GLITCH) as u8
    }

    pub fn status(&mut self) -> Status {
        let s = self.read(reg::STATUS);
        Status {
            squelch_open: s & 2 != 0,
            vox: s & 4 != 0,
            ctcss: s & 1 << 10 != 0,
            tail: s & 1 << 11 != 0,
            cdcss: s & 3 << 14 != 0,
            cdcss_inverted: s & 1 << 15 != 0,
            interrupt: s & 1 != 0,
        }
    }

    /// Which interrupts to raise (the [`int`] flags).
    pub fn interrupts_enable(&mut self, mask: Interrupts) {
        self.write(reg::INT_EN, mask);
    }

    /// The waiting interrupt flags, cleared; 0 if none (REG_0C bit 0, then REG_02 cleared and read, as stock).
    pub fn interrupts(&mut self) -> Interrupts {
        if self.read(reg::STATUS) & 1 == 0 {
            return 0;
        }
        self.write(reg::INT, 0);
        self.read(reg::INT)
    }

    /// Receive sub-tone: none.
    pub fn subtone_off(&mut self) {
        self.write(reg::SUBTONE, 0);
    }

    /// Detect a CTCSS tone (tenths of a Hz) on receive. The same detector finds the 55 Hz tail instead after
    /// [`Bk4829::tail_detect`].
    pub fn ctcss_rx(&mut self, dhz: u16) {
        // CTCSS mode, auto bandwidths, gain 74 (BK4819 practice); transmit enable (bit 15) left off.
        self.write(reg::SUBTONE, 1 << 12 | 74);
        self.write(reg::CTC, ctcss_word(dhz) & 0x1FFF);
        self.write(reg::CTCSS_THRESH, 0x0292);
    }

    /// Detect a DCS code on receive (octal digits' value: 023 is 0o23).
    pub fn dcs_rx(&mut self, code: u16, inverted: bool) {
        self.write(reg::SUBTONE, 51);
        self.cdcss_word(dcs_codeword(code, inverted));
    }

    fn cdcss_word(&mut self, w: u32) {
        // The 134.4 Hz bit rate as a CTC1 word (134.4 x 20.64888 = 0x0AD7), as stock and the BK4819 drivers write.
        self.write(reg::CTC, 0x0AD7);
        self.write(reg::CDCSS, (w & 0xFFF) as u16);
        self.write(reg::CDCSS, 0x8000 | (w >> 12 & 0xFFF) as u16);
    }

    /// Detect the 55 Hz tail (end-of-transmission tone) with the CTCSS detector (stock: REG_07 0x21CD).
    pub fn tail_detect(&mut self) {
        self.write(reg::CTC, 0x2000 | (25391 / 55));
    }

    /// The CTCSS frequency or DCS code being received, from the scan detectors (REG_68, REG_69/6A): tenths of a Hz,
    /// or the 23-bit word. None while scanning.
    pub fn subtone_scan(&mut self) -> (Option<u16>, Option<u32>) {
        let c = self.read(0x68);
        let ctcss = (c & 0x8000 == 0).then(|| ((c as u32 & 0x1FFF) * 1_000_000 / 2_064_888) as u16);
        let hi = self.read(reg::CDCSS_HI);
        let dcs = if hi & 0x8000 == 0 {
            let lo = self.read(reg::CDCSS_LO);
            Some((hi as u32 & 0xFFF) << 12 | (lo as u32 & 0xFFF))
        } else {
            None
        };
        (ctcss, dcs)
    }

    /// DTMF detection on or off (stock's coefficients and settings; the DTMF interrupt is enabled with it).
    pub fn dtmf_rx(&mut self, on: bool) {
        if on {
            for c in DTMF_COEFS {
                self.write(reg::DTMF_COEF, c);
            }
            self.write(reg::DTMF, 0x87FF);
            self.write(reg::TONE, 0xC5C5);
            self.write(reg::INT_EN, int::DTMF);
        } else {
            self.modify(reg::DTMF, 0x0020, 0);
            self.write(reg::TONE, 0);
        }
    }

    /// The last DTMF symbol received (after the DTMF interrupt).
    pub fn dtmf_code(&mut self) -> char {
        b"0123456789ABCD*#"[(self.read(reg::DTMF_CODE) >> 8 & 15) as usize] as char
    }

    /// Set up the FSK modem (not yet receiving or sending).
    pub fn fsk_setup(&mut self, f: &Fsk) {
        self.write(reg::FSK_MODE, f.mode | 1);
        self.write(reg::TONE, 1 << 7 | (f.tone_gain as u16 & 0x7F));
        self.write(reg::TONE2_FREQ, f.tone_word);
        self.write(reg::FSK_SYNC01, (f.sync[0] as u16) << 8 | f.sync[1] as u16);
        self.write(reg::FSK_SYNC23, (f.sync[2] as u16) << 8 | f.sync[3] as u16);
        self.write(reg::FSK_CRC, f.crc);
        let n = f.len.clamp(1, 2048) - 1;
        self.write(reg::FSK_LEN, (n & 0xFF) << 8 | (n >> 8 & 7) << 5);
        self.write(reg::FSK_CTRL, self.fsk_ctrl(f));
    }

    fn fsk_ctrl(&self, f: &Fsk) -> u16 {
        (f.scramble as u16) << 13 | ((f.preamble.clamp(1, 16) - 1) as u16) << 4 | (f.sync4 as u16) << 3
    }

    /// Receive packets: FIFO cleared, receive on, the receive interrupts enabled (sync, FIFO almost full, done).
    pub fn fsk_rx(&mut self, f: &Fsk) {
        let c = self.fsk_ctrl(f);
        self.write(reg::FSK_CTRL, c | 1 << 14);
        self.write(reg::FSK_CTRL, c | 1 << 12);
        self.write(reg::INT_EN, int::FSK_RX_DONE | int::FSK_FIFO_FULL | int::FSK_SYNC);
    }

    /// One word from the receive FIFO (on FIFO almost full: REG_5E bits 2:0 words; on done: the rest).
    pub fn fsk_read(&mut self) -> u16 {
        self.read(reg::FSK_FIFO)
    }

    /// The received packet's CRC passed (after done).
    pub fn fsk_crc_ok(&mut self) -> bool {
        self.read(reg::DTMF_CODE) & 0x10 != 0
    }

    /// Modem off (stock: REG_70 then REG_58 to 0).
    pub fn fsk_off(&mut self) {
        self.write(reg::TONE, 0);
        self.write(reg::FSK_MODE, 0);
    }
}
