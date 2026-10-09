//! The firmware tasks' hardware steps, apart from the MCU, so they run on the PC too.
//!
//! The Radio, GPS and link tasks (`firmware/src/tasks`) decide with the host-tested engines of `ferrite-core`;
//! what they then do to the hardware lives here, written against small traits the firmware implements on its
//! pins and its BK4829 bus:
//! - [`run`]: a path plan ([`ferrite_core::path::Steps`]) carried out on the pins, the PA drive and the chips,
//!   in order, waits included (TX-23, TX-26, SAFE-20 – SAFE-23, decision 95), through [`Hw`];
//! - [`chip`]: each BK4829 set-up the Radio task writes (tuning, AM, tones, the beep, the counter, the SAME
//!   modem, keying with its read-back), through the bk4829 driver's [`bk4829::Bus`];
//! - [`gps`] and [`bt`]: the GPS module's power and PB10, and the Bluetooth module's PB9, as decisions.
//!
//! The tests (`src/tests`) drive real plans from `ferrite-core` through a recording mock of every pin, DAC value,
//! register write and wait, and check what the radio is commanded to do. The radio's own behaviour (RF, audio,
//! timing on the glass) is still the bench's (the test plan's "radio" half).

#![no_std]

pub mod bt;
pub mod chip;
pub mod gps;
mod run;

pub use run::{run, Hw, Pin};

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
