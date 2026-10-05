//! Link: the programming cable (UART4) and, later, Bluetooth. For now it carries the terminal (manual chapter
//! 37): received bytes go to the UI task, which runs the terminal next to the App, and what the terminal draws
//! comes back here to be sent. The documented link protocol and KISS join it in milestone 3 (TERM-2: the cable
//! port setting and the first bytes decide).

use crate::chan::{TERM_IN, TERM_OUT, TERM_WAKE};
use crate::hal::uart;
use crate::report::UART_CPS;
use embassy_time::Timer;

const BAUD: u32 = 115_200;
/// Bytes sent per turn: transmit is polled (87 us a byte), so a turn stays under 3 ms and the other tasks on
/// this executor keep running (TERM-4).
const CHUNK: usize = 32;

#[embassy_executor::task]
pub async fn task() {
    uart::init(UART_CPS, BAUD);
    loop {
        let mut got = false;
        while let Some(c) = uart::getc(UART_CPS) {
            TERM_IN.put(c);
            got = true;
        }
        if got {
            TERM_WAKE.signal(());
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
        Timer::after_millis(if TERM_OUT.count() > 0 { 1 } else { 10 }).await;
    }
}
