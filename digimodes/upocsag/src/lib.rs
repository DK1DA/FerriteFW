//! # upocsag
//!
//! POCSAG pager messages for microcontrollers: the paging format of hospitals, fire services and the amateur DAPNET
//! network. Part of udigimodes: `no_std`, no allocation, integer only.
//!
//! - [`Receiver`]: discriminator audio in, messages out, at 512, 1200 and 2400 baud at once.
//! - [`Decoder`]: codewords in, messages out, for a source that already delivers codewords (a hardware modem).
//! - [`bch`]: the BCH(31,21) code with even parity; corrects up to two bit errors per codeword.
//! - [`encode`]: a message to codewords, for transmitting and testing.

#![no_std]

pub mod bch;
pub mod decode;
pub mod demod;
pub mod encode;

pub use decode::{Decoder, Message};
pub use demod::Receiver;

/// Starts every batch of 16 codewords.
pub const SYNC: u32 = 0x7CD2_15D8;
/// Fills codeword slots with nothing to send; also ends a message.
pub const IDLE: u32 = 0x7A89_C197;
/// Symbol rates POCSAG uses.
pub const BAUDS: [u16; 3] = [512, 1200, 2400];
