//! # udsc
//!
//! Marine VHF DSC (Digital Selective Calling, ITU-R M.493) for microcontrollers: the distress alerts and calls on
//! channel 70 (156.525 MHz). Part of udigimodes: `no_std`, no allocation, integer only.
//!
//! - [`Receiver`]: audio in, calls out. The front end is uafsk's (1300 Hz and 2100 Hz at 1200 baud), then
//!   [`framer`] per slicer: phasing, 10-bit codewords, the two copies of every symbol (DX and RX), the ECC.
//! - [`message`]: a call's symbols to fields: format, addresses (MMSI), category, telecommands, working channel,
//!   and for distress the nature, position and time.

#![no_std]

pub mod framer;
pub mod message;
pub mod receiver;

pub use framer::Call;
pub use message::Message;
pub use receiver::Receiver;

/// End-of-sequence symbols: acknowledgement requested, acknowledgement, end of sequence (anything else).
pub const EOS: [u8; 3] = [117, 122, 127];
