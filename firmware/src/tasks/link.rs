//! Link: the programming cable (UART4) and, later, Bluetooth. It carries the terminal (manual chapter 37) and
//! the link protocol (chapter 29): received bytes go to the UI task, which runs both next to the App and routes
//! the bytes between them (TERM-2), and what they send comes back here. This task owns the UART: the rate
//! changes `BAUD` agrees (LINK-11) happen here, after the answer has gone. KISS joins in milestone 5.

use crate::chan::{CABLE_BAUD, LINK_BAUD, LINK_HELLOS, LINK_ON, TERM_IN, TERM_OUT, TERM_WAKE};
use core::sync::atomic::Ordering::Relaxed;
use embassy_time::{Duration, Instant};
use crate::hal::uart;
use crate::report::UART_CPS;
use embassy_time::Timer;

const BAUD: u32 = 115_200;
/// Bytes sent per turn: transmit is polled (87 us a byte), so a turn stays under 3 ms and the other tasks on
/// this executor keep running (TERM-4).
const CHUNK: usize = 32;
/// Software flow control for a terminal program sending a file (the terminal's `load`, TERM-13): XOFF once this
/// many bytes wait (the receive ring holds 256, so a slow sender still fits), XON again when it has drained.
const XOFF_AT: u16 = 160;
const XON_AT: u16 = 32;
const XOFF: u8 = 0x13;
const XON: u8 = 0x11;
/// A new rate stays only if a `HELLO` arrives at it within this long (LINK-11).
const BAUD_PROOF: Duration = Duration::from_secs(1);

#[embassy_executor::task]
pub async fn task() {
    uart::init(UART_CPS, BAUD); // every boot starts at 115200 (LINK-10)
    let mut held = false;
    // A rate change on probation: its deadline and the HELLO count when it was made.
    let mut probation: Option<(Instant, u16)> = None;
    loop {
        // Only what the terminal's ring has room for: the rest waits in the receive ring, never dropped.
        let mut got = false;
        while TERM_IN.room() > 0 {
            let Some(c) = uart::getc(UART_CPS) else { break };
            TERM_IN.put(c);
            got = true;
        }
        if got {
            TERM_WAKE.signal(());
        }
        let waiting = uart::rx_count(UART_CPS) + TERM_IN.count();
        if LINK_ON.load(Relaxed) {
            // The link's binary frames can't carry XON/XOFF; one frame in flight fits the rings anyway.
            held = false;
        } else if !held && waiting >= XOFF_AT {
            uart::write(UART_CPS, &[XOFF]);
            held = true;
        } else if held && waiting <= XON_AT {
            uart::write(UART_CPS, &[XON]);
            held = false;
        }
        let mut buf = [0u8; CHUNK];
        let mut n = 0;
        while n < CHUNK {
            match TERM_OUT.get() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        if n > 0 {
            uart::write(UART_CPS, &buf[..n]);
            if TERM_OUT.count() == 0 {
                TERM_WAKE.signal(()); // room again: the terminal may have rows waiting
            }
        }
        // LINK-11: switch once everything before (the BAUD answer) has left; back to 115200 without a HELLO.
        if TERM_OUT.count() == 0 && LINK_BAUD.signaled() {
            if let Some(rate) = LINK_BAUD.try_take() {
                uart::flush(UART_CPS);
                uart::init(UART_CPS, rate);
                CABLE_BAUD.store(rate, Relaxed);
                probation = Some((Instant::now() + BAUD_PROOF, LINK_HELLOS.load(Relaxed)));
            }
        }
        if let Some((until, hellos)) = probation {
            if LINK_HELLOS.load(Relaxed) != hellos {
                probation = None;
            } else if Instant::now() >= until {
                uart::init(UART_CPS, BAUD);
                CABLE_BAUD.store(BAUD, Relaxed);
                probation = None;
            }
        }
        Timer::after_millis(if TERM_OUT.count() > 0 { 1 } else { 10 }).await;
    }
}
