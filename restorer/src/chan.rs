//! What the shared HAL (firmware/src/hal) names of the firmware's channels: only `clock::for_audio`'s wake, which
//! the reader never calls. Nothing else of the firmware's tasks is here.
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::signal::Signal;

pub static TERM_WAKE: Signal<M, ()> = Signal::new();
