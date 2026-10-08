//! # ferrite-core
//!
//! Everything in Ferrite that is plain logic: what the tasks say to each other ([`msg`]), frequencies ([`freq`]),
//! keys ([`keys`]), the SPI flash map with its protected ranges ([`layout`]), the one decoder slot all decoders
//! share ([`decoders`]), and chip identification ([`chipid`]). No registers, no async runtime: it all builds and
//! tests on the PC.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod chipid;
pub mod decoders;
pub mod freq;
pub mod keys;
pub mod layout;
pub mod link;
pub mod msg;
pub mod path;
pub mod rxsleep;
pub mod same;
pub mod scan;
pub mod tonegen;
pub mod txcal;
pub mod txplan;
pub mod txseq;
pub mod watch;

/// Shown on the boot screen and in the link's hello.
pub const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
