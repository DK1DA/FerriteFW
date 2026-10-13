//! The channels between the tasks (types in ferrite_core::msg): a command channel per owner, one event bus,
//! and the snapshots the UI draws from. All static; senders wait when a channel is full, nothing is dropped.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use crate::ring::Ring;
use core::sync::atomic::{AtomicBool, AtomicI16, AtomicU16, AtomicU32, AtomicU8};
use ferrite_core::msg::{DebugAns, DebugReq, DspCmd, Event, GpsFix, GpsSky, LiveAll, RadioCmd, SameDecoded, StorageCmd};

pub static RADIO: Channel<M, RadioCmd, 8> = Channel::new();
pub static STORAGE: Channel<M, StorageCmd, 8> = Channel::new();
pub static DSP: Channel<M, DspCmd, 4> = Channel::new();
/// The terminal's BK4829 and SI4732 debug requests to the Radio task, and its answers (TERM-21): one at a time.
pub static DEBUG_REQ: Channel<M, DebugReq, 1> = Channel::new();
pub static DEBUG_ANS: Channel<M, DebugAns, 1> = Channel::new();

/// Keys and the knob, from the input sampler to the UI.
#[derive(Clone, Copy)]
pub enum Input {
    Key(keypad::Event),
    /// Detents, + clockwise.
    Knob(i8),
}
pub static INPUT: Channel<M, Input, 16> = Channel::new();

/// Everything that happened: 16 deep, 4 subscribers (UI, Link, Storage, spare), 6 publishers (one per task).
pub static EVENTS: PubSubChannel<M, Event, 16, 4, 6> = PubSubChannel::new();

/// Alerts the decoders heard (SAME headers), to the UI task: it raises the alert screen (`App::same_alert`).
/// Two deep: a second header (SAME repeats each three times) waits while the first is shown.
pub static ALERTS: Channel<M, SameDecoded, 2> = Channel::new();

/// The Radio task's snapshot of all three slots (one copy, 308 B: the UI keeps its own).
pub static LIVE: Watch<M, LiveAll, 2> = Watch::new();

/// The `tx` build: the factory calibration's transmit side, read once at boot by the UI task (which mounts the
/// SPI flash) for the Radio task: the PA curves (None when blank or implausible: transmit locked, CAL-5, CAL-6)
/// and the battery gauge (PWR-20).
#[cfg(feature = "tx")]
pub static TX_CAL: Signal<M, (Option<ferrite_core::txcal::TxCal>, ferrite_core::txcal::Gauge)> = Signal::new();
/// GPS (manual chapter 28): the latest fix for the UI and the APRS task (and one spare reader), and the
/// satellites in view for the UI's sky plot. The GPS task sends both.
pub static POSITION: Watch<M, GpsFix, 3> = Watch::new();
pub static SKY: Watch<M, GpsSky, 1> = Watch::new();
/// What the GPS should do (0 6 1, 0 6 8), from the UI when the settings change.
pub static GPS_CFG: Signal<M, ferrite_app::gpsdata::GpsCfg> = Signal::new();
/// What the radio is doing, for the GPS task (GPS-4, GPS-22): set by the Radio task with each snapshot.
pub static RF_STATE: AtomicU8 = AtomicU8::new(0);
pub const RF_BUSY_KEYED: u8 = 1;
/// The MCU's die temperature in tenths of a degree C (chapter 8, SBAR-20), smoothed; `i16::MIN` until read. The
/// Radio task reads it with its other ADC reads (the preempted group isn't shared between tasks); the UI shows it.
pub static MCU_TEMP: AtomicI16 = AtomicI16::new(i16::MIN);
/// PWR-20: the latest PA1 battery reading (12-bit, averaged), 0 until the first one; the Radio task reads it.
pub static BATTERY_RAW: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
pub const RF_BUSY_RX: u8 = 2;

/// The terminal over the cable (manual chapter 37): the Link task moves the cable's bytes into `TERM_IN` and
/// signals `TERM_WAKE`; the UI task, which owns the App the terminal reads and edits, feeds them to it and puts
/// what it draws in `TERM_OUT`, which the Link task sends. 64 B in (typed keys), 1 KB out (a few rows).
static TERM_IN_BUF: [AtomicU8; 64] = [const { AtomicU8::new(0) }; 64];
static TERM_OUT_BUF: [AtomicU8; 1024] = [const { AtomicU8::new(0) }; 1024];
pub static TERM_IN: Ring<'static> = Ring::new(&TERM_IN_BUF);
pub static TERM_OUT: Ring<'static> = Ring::new(&TERM_OUT_BUF);
pub static TERM_WAKE: Signal<M, ()> = Signal::new();

/// The link protocol on the cable (manual chapter 29), between the UI task (which runs the session next to the
/// App, as it runs the terminal) and the Link task (which owns UART4): a rate agreed by `BAUD`, to switch to once
/// the answer has gone; the `HELLO`s seen, the proof the new rate works (LINK-11); the rate now; and whether the
/// link holds the port (no XON/XOFF in its binary frames then).
pub static LINK_BAUD: Signal<M, u32> = Signal::new();
pub static LINK_HELLOS: AtomicU16 = AtomicU16::new(0);
pub static CABLE_BAUD: AtomicU32 = AtomicU32::new(115_200);
pub static LINK_ON: AtomicBool = AtomicBool::new(false);

/// Bluetooth (manual chapter 29, roadmap 3d): the Link task moves USART1's bytes into `BT_IN` and sends `BT_OUT`,
/// as it does the cable's; the UI task runs the terminal, the link or KISS on them ([`ferrite_app::ports`]).
/// 64 B in (the UART's own 256 B ring is behind it), 512 B out (a terminal page in a few turns, a link answer,
/// a KISS frame). `BT_ON` is 0 9 2 1 (the module's power, PB9), `BT_LINK_ON` the link holding Bluetooth (no
/// XON/XOFF then).
static BT_IN_BUF: [AtomicU8; 64] = [const { AtomicU8::new(0) }; 64];
static BT_OUT_BUF: [AtomicU8; 512] = [const { AtomicU8::new(0) }; 512];
pub static BT_IN: Ring<'static> = Ring::new(&BT_IN_BUF);
pub static BT_OUT: Ring<'static> = Ring::new(&BT_OUT_BUF);
pub static BT_ON: AtomicBool = AtomicBool::new(false);
pub static BT_LINK_ON: AtomicBool = AtomicBool::new(false);
