//! # uaprs
//!
//! APRS for microcontrollers: what goes inside the frames `uafsk` carries.
//! Part of udigimodes: `no_std`, no allocation, integer only; parsing borrows from the frame instead of copying.
//!
//! - [`ax25`]: UI frames: addresses, digipeater paths, building frames, TNC2 text (`SRC>DEST,PATH:info`).
//! - [`parse`]: APRS packets: positions (plain, compressed, Mic-E, NMEA), messages and acks, objects, items,
//!   status, weather, third-party traffic.
//! - [`build`]: position reports, messages and acks to send.
//! - [`kiss`]: the KISS TNC protocol, for a PC or phone over serial or Bluetooth.
//! - [`beacon`]: smart beaconing; [`geo`]: distance and bearing between positions.

#![no_std]
#![allow(clippy::needless_range_loop)] // indexed loops read better for fixed-layout fields

pub mod ax25;
pub mod beacon;
pub mod build;
pub mod geo;
pub mod kiss;
pub mod parse;

pub use parse::{parse, Packet};
