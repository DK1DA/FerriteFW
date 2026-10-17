//! Link: the programming cable (UART4) and Bluetooth (USART1 to the BT513, roadmap 3d). Each port carries the
//! terminal (manual chapter 37), the link protocol (chapter 29) or KISS (chapter 26): received bytes go to the
//! UI task, which runs them next to the App ([`ferrite_app::ports`] decides which), and what they send comes
//! back here. This task owns both UARTs and the module's power: the cable's rate changes `BAUD` agrees (LINK-11)
//! happen here, after the answer has gone; Bluetooth runs at the module's resting 9600 (LINK-25), its bytes sent
//! only when USART1 can take one without waiting (about 1 ms each), in bursts with gaps the module can forward over
//! BLE (LINK-36).

use crate::board::BT_PWR;
use ferrite_radio::bt::BtPower;
use ferrite_core::link::{BtPace, BtPacer};
use crate::chan::{BT_IN, BT_LINK_ON, BT_ON, BT_OUT, BT_PACE, BT_RATE, CABLE_BAUD, DRY_TX, LINK_BAUD, LINK_HELLOS, LINK_ON, RF_BUSY_KEYED, RF_STATE, TERM_IN, TERM_OUT, TERM_WAKE};
use crate::hal::uart;
use crate::report::{UART_BT, UART_CPS};
use crate::ring::Ring;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use crate::report::UartStats;
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
/// The module's power settles before its UART is used (stock starts USART1, then powers PB9 with no wait; a short
/// wait costs nothing).
const BT_POWER_MS: u64 = 20;
/// The cable log's Bluetooth counters (TERM-28): a line this often while bytes move.
const BT_LOG_MS: u64 = 5_000;

/// One port's bytes between its UART and the UI task's rings.
struct Port {
    uart: usize,
    rx: &'static Ring<'static>,
    tx: &'static Ring<'static>,
    /// The link holds the port: its binary frames can't carry XON/XOFF.
    link: &'static AtomicBool,
    held: bool,
    /// The cable: sends nothing while keyed (TX-43). Bluetooth: only what USART1 takes without waiting, paced.
    cable: bool,
    /// Bluetooth's bursts and gaps (LINK-36); the cable's is never asked.
    pacer: BtPacer,
}

impl Port {
    /// Whether a byte may go now (TX-43: never on the cable while keyed, but in the `tx-dry` build; SND-14: never
    /// on the cable with a speaker mic connected).
    fn may_send(&self) -> bool {
        !self.cable || ferrite_radio::cable::may_send(RF_STATE.load(Relaxed) & RF_BUSY_KEYED != 0, cfg!(feature = "tx-dry") || DRY_TX.load(Relaxed), spkmic())
    }

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
        } else if !self.may_send() {
            // TX-43: flow control waits too; the receive rings still fill as before.
        } else if !self.held && waiting >= XOFF_AT {
            uart::write(self.uart, &[XOFF]);
            self.held = true;
        } else if self.held && waiting <= XON_AT {
            uart::write(self.uart, &[XON]);
            self.held = false;
        }
        got
    }

    /// Up to [`CHUNK`] bytes out. True if any went. The cable asks before each byte: nothing goes while
    /// keyed (TX-43), and the rest waits in the ring. Bluetooth sends no more than its burst has left, and nothing
    /// in a gap (LINK-36): the gap is waited out by the loop's 1 ms turns, never in here.
    fn send(&mut self) -> bool {
        let (pace, now) = (BtPace::unpack(BT_PACE.load(Relaxed)), Instant::now().as_millis());
        let most = if self.cable { CHUNK } else { self.pacer.allow(pace, now).min(CHUNK) };
        let mut n = 0;
        while n < most {
            if !self.may_send() || !self.cable && !uart::tx_free(self.uart) {
                break;
            }
            let Some(b) = self.tx.get() else { break };
            uart::write(self.uart, &[b]);
            n += 1;
        }
        if !self.cable {
            self.pacer.sent(pace, n, now);
        }
        if n > 0 && self.tx.count() == 0 {
            TERM_WAKE.signal(()); // room again: the terminal may have rows waiting
        }
        n > 0
    }
}

/// A speaker mic is in the side connector (SND-12, the input task's debounced state), or PC15 reads low (connected)
/// while that is still being decided (the boot check, a change): the cable goes quiet at the first sign of one.
fn spkmic() -> bool {
    use ferrite_core::acc::{CHANGING, ON};
    let a = crate::chan::ACCESSORY.load(Relaxed);
    a & ON != 0 || a & CHANGING != 0 && !crate::board::ACC_DET.read()
}

/// SND-14 (decision 146): with a speaker mic, PC10 (UART4 TX) held high as a GPIO, as stock; back on the UART
/// without. Whatever was being sent has left first; what waits for the cable is dropped.
fn cable_quiet(quiet: bool) {
    if quiet {
        uart::flush(UART_CPS);
        uart::tx_pin_quiet(UART_CPS);
        TERM_OUT.clear();
    } else {
        uart::tx_pin_live(UART_CPS);
        crate::bringup::line(b"cable: live (speaker mic removed)");
    }
}

/// 0 9 2 1: the module powered (PB9, active high) and USART1 at its rate (9600, LINK-25), or both off with the TX
/// pin let go so a powered-down module isn't fed (the MCU never configures the module).
fn bt_power(on: bool) {
    if on {
        BT_PWR.write(true);
        uart::init(UART_BT, BT_RATE.load(Relaxed));
    } else {
        uart::deinit(UART_BT);
        BT_PWR.write(false);
        BT_IN.clear();
        BT_OUT.clear();
    }
}

/// TERM-28: "bt: in 12 out 340 (+12/+340), framing 0, overrun 0, 9600 baud, module on" (the change in the last 5 s
/// in brackets).
fn bt_log_line(was: &UartStats, now: &UartStats) {
    let (fe, ov) = uart::errors(UART_BT);
    crate::bringup::linef(|b| {
        let _ = ufmt::uwrite!(
            b,
            "bt: in {} out {} (+{}/+{}), framing {}, overrun {}, {} baud, module {}",
            now.rx_bytes,
            now.tx_bytes,
            now.rx_bytes.wrapping_sub(was.rx_bytes),
            now.tx_bytes.wrapping_sub(was.tx_bytes),
            fe,
            ov,
            uart::rate(UART_BT),
            if BT_PWR.out_level() { "on" } else { "off" }
        );
    });
}

#[embassy_executor::task]
pub async fn task() {
    uart::init(UART_CPS, BAUD); // every boot starts at 115200 (LINK-10)
    let mut cable = Port { uart: UART_CPS, rx: &TERM_IN, tx: &TERM_OUT, link: &LINK_ON, held: false, cable: true, pacer: BtPacer::default() };
    let mut bt = Port { uart: UART_BT, rx: &BT_IN, tx: &BT_OUT, link: &BT_LINK_ON, held: false, cable: false, pacer: BtPacer::default() };
    // 0 9 2 1 and PB9 (`ferrite_radio::bt`, host-tested).
    let mut bt_pw = BtPower::default();
    // Whose turn it is to send when both ports have bytes waiting.
    let mut bt_turn = false;
    // A rate change on probation: its deadline and the HELLO count when it was made.
    let mut probation: Option<(Instant, u16)> = None;
    // SND-14: PC10 held high while a speaker mic is connected.
    let mut quiet = false;
    // The cable log's Bluetooth counters: when last looked at, and what they were.
    let mut bt_log = (Instant::now(), uart::stats(UART_BT));
    loop {
        if spkmic() != quiet {
            quiet = !quiet;
            cable_quiet(quiet);
        }
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
        // A bench `bt baud N`: USART1 restarted at the new rate (the MCU's side only).
        if bt_on && uart::rate(UART_BT) != BT_RATE.load(Relaxed) {
            uart::flush(UART_BT);
            uart::init(UART_BT, BT_RATE.load(Relaxed));
        }
        // TERM-28: Bluetooth's byte counts on the cable log every 5 s while they move.
        if bt_log.0.elapsed() >= Duration::from_millis(BT_LOG_MS) {
            let now = uart::stats(UART_BT);
            if now.rx_bytes != bt_log.1.rx_bytes || now.tx_bytes != bt_log.1.tx_bytes || now.line_errors != bt_log.1.line_errors {
                bt_log_line(&bt_log.1, &now);
            }
            bt_log = (Instant::now(), now);
        }
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
                if quiet {
                    uart::tx_pin_quiet(UART_CPS); // init gave the pin back to the UART
                }
                CABLE_BAUD.store(rate, Relaxed);
                probation = Some((Instant::now() + BAUD_PROOF, LINK_HELLOS.load(Relaxed)));
            }
        }
        if let Some((until, hellos)) = probation {
            if LINK_HELLOS.load(Relaxed) != hellos {
                probation = None;
            } else if Instant::now() >= until {
                uart::init(UART_CPS, BAUD);
                if quiet {
                    uart::tx_pin_quiet(UART_CPS);
                }
                CABLE_BAUD.store(BAUD, Relaxed);
                probation = None;
            }
        }
        let busy = TERM_OUT.count() > 0 && cable.may_send() || bt_on && BT_OUT.count() > 0;
        Timer::after_millis(if busy { 1 } else { 10 }).await;
    }
}
