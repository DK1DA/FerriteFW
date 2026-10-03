//! # si4732
//!
//! A driver for the Skyworks (Silicon Labs) SI4732 broadcast receiver: FM with RDS, AM on long, medium and short
//! wave, and SSB once its patch is loaded. `no_std`, no allocation. Commands follow AN332.
//!
//! **Nothing blocks for long.** A command and its reply take well under a millisecond; those wait in place,
//! with a busy-wait bounded at 10 ms. Power-up (up to 110 ms, then 500 ms for the crystal), tuning and seeking
//! (up to seconds) are a start call and a poll: [`Si4732::power_up_start`] then [`Si4732::ready`],
//! [`Si4732::tune_start`] or [`Si4732::seek_start`] then [`Si4732::tune_poll`]. A firmware awaits a timer
//! between polls.
//!
//! **SSB needs a patch** from Silicon Labs, uploaded at every SSB power-up. It is not part of this crate. The
//! caller reads it (from its own storage) and hands it over one 8-byte record at a time
//! ([`Si4732::patch_record`]).

#![no_std]

pub mod rds;

use uradio_bus::{Delay, I2c, Nack};

/// I2C address with SEN low (the RT-950 Pro's): 0x11. With SEN high it is 0x63.
pub const ADDR: u8 = 0x11;

const POWER_UP: u8 = 0x01;
const GET_REV: u8 = 0x10;
const POWER_DOWN: u8 = 0x11;
const SET_PROPERTY: u8 = 0x12;
const GET_INT_STATUS: u8 = 0x14;
const FM_TUNE_FREQ: u8 = 0x20;
const FM_SEEK_START: u8 = 0x21;
const FM_TUNE_STATUS: u8 = 0x22;
const FM_RSQ_STATUS: u8 = 0x23;
const FM_RDS_STATUS: u8 = 0x24;
/// The AM (and SSB) commands are the FM ones + 0x20.
const AM: u8 = 0x20;

const CTS: u8 = 0x80;
const ERR: u8 = 0x40;
const STCINT: u8 = 0x01;

/// Properties used here (AN332).
pub mod prop {
    pub const GPO_IEN: u16 = 0x0001;
    pub const SSB_BFO: u16 = 0x0100;
    pub const SSB_MODE: u16 = 0x0101;
    pub const FM_DEEMPHASIS: u16 = 0x1100;
    pub const FM_SEEK_BAND_BOTTOM: u16 = 0x1400;
    pub const FM_SEEK_BAND_TOP: u16 = 0x1401;
    pub const FM_SEEK_FREQ_SPACING: u16 = 0x1402;
    pub const FM_RDS_INT_SOURCE: u16 = 0x1500;
    pub const FM_RDS_INT_FIFO_COUNT: u16 = 0x1501;
    pub const FM_RDS_CONFIG: u16 = 0x1502;
    pub const AM_CHANNEL_FILTER: u16 = 0x3102;
    pub const AM_AVC_MAX_GAIN: u16 = 0x3103;
    pub const AM_SOFT_MUTE_MAX_ATTENUATION: u16 = 0x3302;
    pub const AM_SEEK_BAND_BOTTOM: u16 = 0x3400;
    pub const AM_SEEK_BAND_TOP: u16 = 0x3401;
    pub const AM_SEEK_FREQ_SPACING: u16 = 0x3402;
    pub const RX_VOLUME: u16 = 0x4000;
    pub const RX_HARD_MUTE: u16 = 0x4001;
}

/// Receiver function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Fm,
    /// AM on long, medium and short wave.
    Am,
    /// SSB (AM function with the patch).
    Ssb,
}

/// Sideband for SSB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sideband {
    Lsb,
    Usb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The chip did not acknowledge (not powered, not present).
    Nack,
    /// No clear-to-send within the wait.
    Timeout,
    /// The chip flagged the command (bad argument, or wrong state for it).
    Rejected,
    /// Frequency outside the band for the mode.
    OutOfBand,
    /// Not powered up in a mode that has this.
    WrongMode,
}

impl From<Nack> for Error {
    fn from(_: Nack) -> Self {
        Error::Nack
    }
}

/// A completed tune or seek.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tune {
    /// FM: 10 kHz units; AM and SSB: kHz.
    pub freq: u16,
    /// dBuV.
    pub rssi: u8,
    /// dB.
    pub snr: u8,
    /// A valid station (seek's criteria).
    pub valid: bool,
}

/// Signal quality now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rsq {
    pub rssi: u8,
    pub snr: u8,
    pub valid: bool,
}

/// SSB settings (SSB_MODE property).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SsbConfig {
    /// Audio bandwidth: 0 = 1.2 kHz, 1 = 2.2, 2 = 3.0, 3 = 4.0, 4 = 500 Hz, 5 = 1.0 kHz.
    pub audio_bw: u8,
    /// Sideband cut-off filter: 0 = band-pass, 1 = low-pass.
    pub sideband_cutoff: u8,
    /// AVC divider (0 for SSB, 3 for sync AM).
    pub avc_divider: u8,
    pub avc: bool,
    /// Soft mute on RSSI (false) or SNR (true).
    pub soft_mute_snr: bool,
    /// AFC off (normal for SSB).
    pub afc_off: bool,
}

impl SsbConfig {
    /// The property value: audio BW in bits 3:0, cut-off 7:4, AVC divider 11:8, AVC enable 12, soft-mute select
    /// 13, AFC disable 15.
    pub fn bits(&self) -> u16 {
        (self.audio_bw as u16 & 15)
            | (self.sideband_cutoff as u16 & 15) << 4
            | (self.avc_divider as u16 & 15) << 8
            | (self.avc as u16) << 12
            | (self.soft_mute_snr as u16) << 13
            | (self.afc_off as u16) << 15
    }
}

/// One RDS group: blocks A-D and the block error levels (2 bits each, A in bits 7:6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub blocks: [u16; 4],
    pub errors: u8,
}

/// The receiver.
pub struct Si4732<B: I2c> {
    bus: B,
    addr: u8,
    mode: Option<Mode>,
}

impl<B: I2c> Si4732<B> {
    /// Sends nothing; the chip is assumed powered down (call [`Si4732::power_down`] after an MCU reset if unsure).
    pub fn new(bus: B, addr: u8) -> Self {
        Si4732 { bus, addr, mode: None }
    }

    pub fn mode(&self) -> Option<Mode> {
        self.mode
    }

    /// Status byte: CTS and ERR.
    fn status(&mut self) -> Result<u8, Error> {
        let mut s = [0u8];
        self.bus.read(self.addr, &mut s)?;
        Ok(s[0])
    }

    /// Waits for clear-to-send, polling every 20 us, at most 10 ms.
    fn wait_cts(&mut self, delay: &mut impl Delay) -> Result<(), Error> {
        for _ in 0..500 {
            let s = self.status()?;
            if s & CTS != 0 {
                return if s & ERR != 0 { Err(Error::Rejected) } else { Ok(()) };
            }
            delay.delay_us(20);
        }
        Err(Error::Timeout)
    }

    /// A command: send, wait for CTS, read the reply (`reply[0]` is the status byte).
    pub fn command(&mut self, cmd: &[u8], reply: &mut [u8], delay: &mut impl Delay) -> Result<(), Error> {
        self.bus.write(self.addr, cmd)?;
        self.wait_cts(delay)?;
        if !reply.is_empty() {
            self.bus.read(self.addr, reply)?;
        }
        Ok(())
    }

    pub fn set_property(&mut self, prop: u16, value: u16, delay: &mut impl Delay) -> Result<(), Error> {
        let c = [SET_PROPERTY, 0, (prop >> 8) as u8, prop as u8, (value >> 8) as u8, value as u8];
        self.command(&c, &mut [], delay)
    }

    /// Starts power-up in `mode` (crystal oscillator, analog audio out); returns at once. Poll [`Si4732::ready`]
    /// (up to 110 ms), then, for SSB, send the patch; then wait 500 ms for the crystal before tuning.
    pub fn power_up_start(&mut self, mode: Mode) -> Result<(), Error> {
        // ARG1: PATCH (5) | XOSCEN (4) | FUNC (3:0: 0 FM, 1 AM); ARG2: 0x05 analog audio out.
        let arg1 = match mode {
            Mode::Fm => 0x10,
            Mode::Am => 0x11,
            Mode::Ssb => 0x31,
        };
        self.bus.write(self.addr, &[POWER_UP, arg1, 0x05])?;
        self.mode = Some(mode);
        Ok(())
    }

    /// True once the chip is ready for the next command; an error if it rejected the last one.
    pub fn ready(&mut self) -> Result<bool, Error> {
        let s = self.status()?;
        if s & CTS == 0 {
            return Ok(false);
        }
        if s & ERR != 0 {
            return Err(Error::Rejected);
        }
        Ok(true)
    }

    /// One 8-byte record of the SSB patch, after an SSB power-up and before anything else.
    pub fn patch_record(&mut self, record: &[u8; 8], delay: &mut impl Delay) -> Result<(), Error> {
        if self.mode != Some(Mode::Ssb) {
            return Err(Error::WrongMode);
        }
        self.command(record, &mut [], delay)
    }

    pub fn power_down(&mut self, delay: &mut impl Delay) -> Result<(), Error> {
        self.mode = None;
        self.command(&[POWER_DOWN], &mut [], delay)
    }

    /// Part number (32 for an SI4732), firmware major and minor, chip revision.
    pub fn revision(&mut self, delay: &mut impl Delay) -> Result<(u8, u8, u8, u8), Error> {
        let mut r = [0u8; 9];
        self.command(&[GET_REV], &mut r, delay)?;
        Ok((r[1], r[2], r[3], r[8]))
    }

    /// Volume 0-63.
    pub fn volume(&mut self, v: u8, delay: &mut impl Delay) -> Result<(), Error> {
        self.set_property(prop::RX_VOLUME, v.min(63) as u16, delay)
    }

    pub fn mute(&mut self, on: bool, delay: &mut impl Delay) -> Result<(), Error> {
        self.set_property(prop::RX_HARD_MUTE, if on { 3 } else { 0 }, delay)
    }

    /// SSB settings and the fine tuning offset (Hz, -16383..16383): after the patch.
    pub fn ssb(&mut self, cfg: SsbConfig, bfo_hz: i16, delay: &mut impl Delay) -> Result<(), Error> {
        if self.mode != Some(Mode::Ssb) {
            return Err(Error::WrongMode);
        }
        self.set_property(prop::SSB_MODE, cfg.bits(), delay)?;
        self.bfo(bfo_hz, delay)
    }

    /// SSB fine tuning, Hz.
    pub fn bfo(&mut self, hz: i16, delay: &mut impl Delay) -> Result<(), Error> {
        self.set_property(prop::SSB_BFO, hz as u16, delay)
    }

    /// FM: RDS on (interrupt per group, up to 5 corrected errors per block).
    pub fn rds_on(&mut self, delay: &mut impl Delay) -> Result<(), Error> {
        if self.mode != Some(Mode::Fm) {
            return Err(Error::WrongMode);
        }
        self.set_property(prop::FM_RDS_INT_SOURCE, 0x0001, delay)?;
        self.set_property(prop::FM_RDS_INT_FIFO_COUNT, 0x0001, delay)?;
        self.set_property(prop::FM_RDS_CONFIG, 0xAA01, delay)
    }

    fn op(&self, fm: u8) -> Result<u8, Error> {
        match self.mode {
            Some(Mode::Fm) => Ok(fm),
            Some(_) => Ok(fm + AM),
            None => Err(Error::WrongMode),
        }
    }

    /// Starts tuning; poll [`Si4732::tune_poll`]. FM: 10 kHz units, 64.00-108.00 MHz (6400-10800); AM and SSB:
    /// kHz, 149-30000. `sideband` only matters for SSB.
    pub fn tune_start(&mut self, freq: u16, sideband: Sideband, delay: &mut impl Delay) -> Result<(), Error> {
        let mode = self.mode.ok_or(Error::WrongMode)?;
        let ok = match mode {
            Mode::Fm => (6400..=10800).contains(&freq),
            _ => (149..=30000).contains(&freq),
        };
        if !ok {
            return Err(Error::OutOfBand);
        }
        let (hi, lo) = ((freq >> 8) as u8, freq as u8);
        match mode {
            Mode::Fm => self.command(&[FM_TUNE_FREQ, 0, hi, lo, 0], &mut [], delay),
            Mode::Am => self.command(&[FM_TUNE_FREQ + AM, 0, hi, lo, 0, 0], &mut [], delay),
            Mode::Ssb => {
                // ARG1 bits 7:6: 1 = LSB, 2 = USB.
                let sb = match sideband {
                    Sideband::Lsb => 0x40,
                    Sideband::Usb => 0x80,
                };
                self.command(&[FM_TUNE_FREQ + AM, sb, hi, lo, 0, 0], &mut [], delay)
            }
        }
    }

    /// Starts a seek up or down (wrapping at the band edges); poll [`Si4732::tune_poll`]. FM and AM only.
    pub fn seek_start(&mut self, up: bool, delay: &mut impl Delay) -> Result<(), Error> {
        let arg = if up { 0x0C } else { 0x04 }; // SEEKUP (3) | WRAP (2)
        match self.mode {
            Some(Mode::Fm) => self.command(&[FM_SEEK_START, arg], &mut [], delay),
            Some(Mode::Am) => self.command(&[FM_SEEK_START + AM, arg, 0, 0, 0, 0], &mut [], delay),
            _ => Err(Error::WrongMode),
        }
    }

    /// The tune or seek's result once it has completed (the seek-tune-complete interrupt), else None.
    pub fn tune_poll(&mut self, delay: &mut impl Delay) -> Result<Option<Tune>, Error> {
        // STCINT only updates when GET_INT_STATUS runs (AN332).
        let mut s = [0u8];
        self.command(&[GET_INT_STATUS], &mut s, delay)?;
        if s[0] & STCINT == 0 {
            return Ok(None);
        }
        let mut r = [0u8; 8];
        let op = self.op(FM_TUNE_STATUS)?;
        self.command(&[op, 0x01], &mut r, delay)?; // INTACK
        Ok(Some(Tune { freq: (r[2] as u16) << 8 | r[3] as u16, rssi: r[4], snr: r[5], valid: r[1] & 1 != 0 }))
    }

    /// Signal quality now.
    pub fn rsq(&mut self, delay: &mut impl Delay) -> Result<Rsq, Error> {
        let mut r = [0u8; 6];
        let op = self.op(FM_RSQ_STATUS)?;
        self.command(&[op, 0x00], &mut r, delay)?;
        Ok(Rsq { valid: r[2] & 1 != 0, rssi: r[4], snr: r[5] })
    }

    /// One RDS group from the receiver's FIFO, if it has one.
    pub fn rds_read(&mut self, delay: &mut impl Delay) -> Result<Option<Group>, Error> {
        if self.mode != Some(Mode::Fm) {
            return Err(Error::WrongMode);
        }
        // Ask without popping (STATUSONLY), then pop one group only if the FIFO has one.
        let mut r = [0u8; 13];
        self.command(&[FM_RDS_STATUS, 0x04], &mut r[..4], delay)?;
        if r[3] == 0 {
            return Ok(None);
        }
        self.command(&[FM_RDS_STATUS, 0x01], &mut r, delay)?;
        Ok(Some(Group { blocks: core::array::from_fn(|i| (r[4 + 2 * i] as u16) << 8 | r[5 + 2 * i] as u16), errors: r[12] }))
    }

    /// Empties the RDS FIFO (after retuning: the old station's groups).
    pub fn rds_flush(&mut self, delay: &mut impl Delay) -> Result<(), Error> {
        let mut r = [0u8; 13];
        self.command(&[FM_RDS_STATUS, 0x03], &mut r, delay)
    }

    pub fn release(self) -> B {
        self.bus
    }
}
