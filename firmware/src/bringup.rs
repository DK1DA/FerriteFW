//! The cable log (roadmap 1c; TERM-28): plain lines on the programming cable at 115200 baud, so a boot can be
//! followed without trusting the screen or the key map. The boot report, every raw key code, the knob, the
//! receiver once a second, beeps, redraw times and a fault's address. The first byte from the PC ends the log and
//! starts the terminal or link (chapter 37). Every build prints the boot report; after it the bring-up build keeps
//! logging, the normal build only with Cable log on (0 9 9 7). A fault's report prints in every build.

use crate::chan::{TERM_OUT, TERM_WAKE};
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use ferrite_ui::text::Buf;

/// The log is on (until the PC sends a byte). On at power on in every build, for the boot report.
pub static ON: AtomicBool = AtomicBool::new(true);

/// One line to the cable (dropped bytes if the ring is full: the log never waits).
pub fn line(s: &[u8]) {
    if !ON.load(Relaxed) {
        return;
    }
    // Tasks on every executor write lines into the one single-producer ring: a line goes in whole, with interrupts
    // off, so two never interleave (the cable log showed "ddraw 14 ms" halves, Oct 20, 2026). At most ~100 bytes.
    critical_section::with(|_| {
        for &c in s.iter().chain(b"\r\n") {
            TERM_OUT.put(c);
        }
    });
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
/// UART was never started. Plain byte arrays and fast code only (tools/slowcheck.py); the text itself may sit in
/// the slow flash (memory.x), which is only slower to read.
pub fn fault(pc: Option<u32>) {
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
