//! The channels between the tasks (types in ferrite_core::msg): a command channel per owner, one event bus,
//! and the snapshots the UI draws from. All static; senders wait when a channel is full, nothing is dropped.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use crate::ring::Ring;
use core::sync::atomic::AtomicU8;
use ferrite_core::msg::{DebugAns, DebugReq, DspCmd, Event, Live, Position, RadioCmd, SameDecoded, StorageCmd};

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

pub static LIVE: Watch<M, Live, 2> = Watch::new();
pub static POSITION: Watch<M, Position, 2> = Watch::new();

/// The terminal over the cable (manual chapter 37): the Link task moves the cable's bytes into `TERM_IN` and
/// signals `TERM_WAKE`; the UI task, which owns the App the terminal reads and edits, feeds them to it and puts
/// what it draws in `TERM_OUT`, which the Link task sends. 64 B in (typed keys), 1 KB out (a few rows).
static TERM_IN_BUF: [AtomicU8; 64] = [const { AtomicU8::new(0) }; 64];
static TERM_OUT_BUF: [AtomicU8; 1024] = [const { AtomicU8::new(0) }; 1024];
pub static TERM_IN: Ring<'static> = Ring::new(&TERM_IN_BUF);
pub static TERM_OUT: Ring<'static> = Ring::new(&TERM_OUT_BUF);
pub static TERM_WAKE: Signal<M, ()> = Signal::new();
