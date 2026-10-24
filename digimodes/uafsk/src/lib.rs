//! # uafsk
//!
//! A 1200-baud AFSK (Bell 202) packet modem for microcontrollers: the modulation APRS and VHF packet radio use.
//! Audio in, checked HDLC frames out ([`demod::Demodulator`]); frames in, audio out
//! ([`modulate::Modulator`]). What is inside a frame (AX.25 addresses, APRS) is the protocol's business, not the
//! modem's, except that a frame repaired after a failed FCS must have a sane AX.25 address field. Part of udigimodes: `no_std`, no allocation, integer only.

#![no_std]
#![allow(clippy::needless_range_loop)] // indexed loops read better in DSP code

pub mod demod;
pub mod fsk;
pub mod hdlc;
pub mod modulate;

pub use demod::{Demodulator, Fx25Block, Fx25Frame};
pub use ufx25;
pub use modulate::{Modulator, TxConfig};
