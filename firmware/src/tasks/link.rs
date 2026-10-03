//! Link: the programming cable (UART4) and, later, Bluetooth. For now a line console: `ver` answers with the
//! version. The documented link protocol and the web app's dispatcher replace it in milestone 3.

use crate::hal::uart;
use crate::report::UART_CPS;
use embassy_time::Timer;
use ferrite_core::VERSION;

const BAUD: u32 = 115_200;

fn put(s: &[u8]) {
    uart::write(UART_CPS, s)
}

#[embassy_executor::task]
pub async fn task() {
    uart::init(UART_CPS, BAUD);
    put(b"\r\nFerrite ");
    put(VERSION.as_bytes());
    put(b"\r\n> ");
    let mut line = [0u8; 64];
    let mut n = 0;
    loop {
        while let Some(c) = uart::getc(UART_CPS) {
            match c {
                b'\r' | b'\n' => {
                    put(b"\r\n");
                    match &line[..n] {
                        b"" => {}
                        b"ver" => {
                            put(b"Ferrite ");
                            put(VERSION.as_bytes());
                            put(b"\r\n");
                        }
                        _ => put(b"? (ver)\r\n"),
                    }
                    n = 0;
                    put(b"> ");
                }
                _ if n < line.len() => {
                    line[n] = c;
                    n += 1;
                    put(&[c]);
                }
                _ => {}
            }
        }
        Timer::after_millis(10).await;
    }
}
