//! Link: the programming cable (UART4) and Bluetooth (USART1 to the BT513, roadmap 3d). Each port carries the
//! terminal (manual chapter 37), the link protocol (chapter 29) or KISS (chapter 26): received bytes go to the
//! UI task, which runs them next to the App ([`ferrite_app::ports`] decides which), and what they send comes
//! back here. This task owns both UARTs and the module's power: the cable's rate changes `BAUD` agrees (LINK-11)
//! happen here, after the answer has gone; Bluetooth stays at 115200 (LINK-25).

use crate::board::BT_PWR;
use ferrite_radio::bt::BtPower;
use crate::chan::{BT_IN, BT_LINK_ON, BT_ON, BT_OUT, CABLE_BAUD, LINK_BAUD, LINK_HELLOS, LINK_ON, RF_BUSY_KEYED, RF_STATE, TERM_IN, TERM_OUT, TERM_WAKE};
use crate::hal::uart;
use crate::report::{UART_BT, UART_CPS};
use crate::ring::Ring;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use embassy_time::{Duration, Instant, Timer};

const BAUD: u32 = 115_200;
/// Bytes sent per turn: transmit is polled (87 us a byte), so a turn stays under 3 ms and the other tasks on
/// this executor keep running (TERM-4). With both ports sending, they take turns.
const CHUNK: usize = 32;
/// Software flow control for a terminal program sending a file (the terminal's `load`, TERM-13): XOFF once this
/// many bytes wait (each UART's receive ring holds 256, so a slow sender still fits), XON again when it has
/// drained.
const XOFF_AT: u16 = 160;
const XON_AT: u16 = 32;
const XOFF: u8 = 0x13;
const XON: u8 = 0x11;
/// A new rate stays only if a `HELLO` arrives at it within this long (LINK-11).
const BAUD_PROOF: Duration = Duration::from_secs(1);
/// The module's power settles before its UART is used (stock powers PB9 and starts USART1 together; a short
/// wait costs nothing).
const BT_POWER_MS: u64 = 20;

/// One port's bytes between its UART and the UI task's rings.
struct Port {
    uart: usize,
    rx: &'static Ring<'static>,
    tx: &'static Ring<'static>,
    /// The link holds the port: its binary frames can't carry XON/XOFF.
    link: &'static AtomicBool,
    held: bool,
}

impl Port {
    /// Received bytes into the UI's ring (only what it has room for: the rest waits in the UART's ring, never
    /// dropped), and flow control (TERM-13). True if any arrived.
    fn receive(&mut self) -> bool {
        let mut got = false;
        while self.rx.room() > 0 {
            let Some(c) = uart::getc(self.uart) else { break };
            self.rx.put(c);
            got = true;
        }
        let waiting = uart::rx_count(self.uart) + self.rx.count();
        if self.link.load(Relaxed) {
            self.held = false;
        } else if !self.held && waiting >= XOFF_AT {
            uart::write(self.uart, &[XOFF]);
            self.held = true;
        } else if self.held && waiting <= XON_AT {
            uart::write(self.uart, &[XON]);
            self.held = false;
        }
        got
    }

    /// Up to [`CHUNK`] bytes out. True if any went.
    fn send(&mut self) -> bool {
        let mut buf = [0u8; CHUNK];
        let mut n = 0;
        while n < CHUNK {
            match self.tx.get() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        if n > 0 {
            uart::write(self.uart, &buf[..n]);
            if self.tx.count() == 0 {
                TERM_WAKE.signal(()); // room again: the terminal may have rows waiting
            }
        }
        n > 0
    }
}

/// 0 9 2 1: the module powered (PB9, active high) and USART1 at 115200, or both off with the TX pin let go so a
/// powered-down module isn't fed (LINK-25: the MCU never configures the module; the rate is fixed).
fn bt_power(on: bool) {
    if on {
        BT_PWR.write(true);
        uart::init(UART_BT, BAUD);
    } else {
        uart::deinit(UART_BT);
        BT_PWR.write(false);
        BT_IN.clear();
        BT_OUT.clear();
    }
}

#[embassy_executor::task]
pub async fn task() {
    uart::init(UART_CPS, BAUD); // every boot starts at 115200 (LINK-10)
    let mut cable = Port { uart: UART_CPS, rx: &TERM_IN, tx: &TERM_OUT, link: &LINK_ON, held: false };
    let mut bt = Port { uart: UART_BT, rx: &BT_IN, tx: &BT_OUT, link: &BT_LINK_ON, held: false };
    // 0 9 2 1 and PB9 (`ferrite_radio::bt`, host-tested).
    let mut bt_pw = BtPower::default();
    // Whose turn it is to send when both ports have bytes waiting.
    let mut bt_turn = false;
    // A rate change on probation: its deadline and the HELLO count when it was made.
    let mut probation: Option<(Instant, u16)> = None;
    loop {
        // 0 9 2 1 as the UI last set it.
        let out = bt_pw.step(BT_ON.load(Relaxed), RF_STATE.load(Relaxed) & RF_BUSY_KEYED != 0);
        if let Some(want) = out.power {
            if want {
                BT_PWR.write(true);
                Timer::after_millis(BT_POWER_MS).await;
            }
            bt_power(want);
        }
        // Decision 76: stock sets PB9 again after each transmission while Bluetooth is on.
        if out.reassert {
            BT_PWR.write(true);
        }
        let bt_on = bt_pw.on;
        let mut got = cable.receive();
        if bt_on {
            got |= bt.receive();
        }
        if got {
            TERM_WAKE.signal(());
        }
        // One port's chunk per turn, alternating when both have bytes waiting.
        if bt_on && bt_turn {
            if !bt.send() {
                cable.send();
            }
        } else if !cable.send() && bt_on {
            bt.send();
        }
        bt_turn = !bt_turn;
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
        let busy = TERM_OUT.count() > 0 || bt_on && BT_OUT.count() > 0;
        Timer::after_millis(if busy { 1 } else { 10 }).await;
    }
}
