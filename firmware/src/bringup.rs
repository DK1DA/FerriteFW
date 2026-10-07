//! The bring-up build's cable log (feature `bringup`, roadmap 1c): plain lines on the programming cable at
//! 115200 baud, so a first boot on a new radio can be followed without trusting the screen or the key map. The
//! boot report, every raw key code, the knob, the receiver once a second, and a fault's address. The first byte
//! from the PC ends the log and starts the terminal (chapter 37). Normal builds have none of it.

use crate::chan::{TERM_OUT, TERM_WAKE};
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use ferrite_ui::text::Buf;

/// The log is on (until the PC sends a byte).
pub static ON: AtomicBool = AtomicBool::new(cfg!(feature = "bringup"));

/// One line to the cable (dropped bytes if the ring is full: the log never waits).
pub fn line(s: &[u8]) {
    if !ON.load(Relaxed) {
        return;
    }
    for &c in s.iter().chain(b"\r\n") {
        TERM_OUT.put(c);
    }
    TERM_WAKE.signal(());
}

/// A line built with ufmt.
pub fn linef(f: impl FnOnce(&mut Buf<96>)) {
    if ON.load(Relaxed) {
        let mut b = Buf::<96>::new();
        f(&mut b);
        line(b.as_bytes());
    }
}

/// `0x` and eight hex digits.
pub fn hex(b: &mut Buf<96>, v: u32) {
    b.push(b"0x");
    for i in (0..8).rev() {
        b.push(&[b"0123456789ABCDEF"[(v >> (i * 4)) as usize & 15]]);
    }
}

/// From the fault path (interrupts off, safe state done): straight to the UART, polled; nothing if the cable's
/// UART was never started. Plain byte arrays only: the fault path stays in fast flash (tools/slowcheck.py).
pub fn fault(pc: Option<u32>) {
    if !cfg!(feature = "bringup") {
        return;
    }
    let uart = crate::report::UART_CPS;
    crate::hal::uart::write(uart, b"\r\n*** FAULT: safe state, red LED, stopped");
    if let Some(pc) = pc {
        let mut h = *b"; pc 0x00000000";
        for i in 0..8 {
            h[7 + i] = b"0123456789ABCDEF"[(pc >> ((7 - i) * 4)) as usize & 15];
        }
        crate::hal::uart::write(uart, &h);
    }
    crate::hal::uart::write(uart, b"\r\n");
}
