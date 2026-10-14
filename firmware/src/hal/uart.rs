//! Serial ports. Receive is interrupt driven into a ring; transmit is polled. Mirrors fw/hal/hal_uart.
//!   UART_BT   USART1 PA9/PA10   Bluetooth module
//!   UART_GPS  USART3 PB10/PB11  GPS module
//!   UART_CPS  UART4  PC10/PC11  programming cable (console)
use super::gpio::{pin, Mode, Pin, GPIOA, GPIOB, GPIOC};
use super::regs::*;
use super::{clock, irq, modify, rd, wr};
use crate::ring::Ring;
use crate::report::{UartStats, UART_COUNT};
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering::Relaxed};

/// Above the time driver and the executors; the handlers only fill the rings.
const PRIO: u8 = 3;

struct Hw {
    base: u32,
    en_reg: u32,
    en_bit: u32,
    apb2: bool,
    irq: u16,
    tx: Pin,
    rx: Pin,
}

// A `static`: the receive interrupt reads it, and only named statics stay in the fast flash (memory.x).
static HW: [Hw; UART_COUNT] = [
    Hw { base: USART1, en_reg: CRM_APB2EN, en_bit: CRM_APB2_USART1, apb2: true, irq: irq::USART1, tx: pin(GPIOA, 9), rx: pin(GPIOA, 10) },
    Hw { base: USART3, en_reg: CRM_APB1EN, en_bit: CRM_APB1_USART3, apb2: false, irq: irq::USART3, tx: pin(GPIOB, 10), rx: pin(GPIOB, 11) },
    Hw { base: UART4, en_reg: CRM_APB1EN, en_bit: CRM_APB1_UART4, apb2: false, irq: irq::UART4, tx: pin(GPIOC, 10), rx: pin(GPIOC, 11) },
];

static BUF_BT: [AtomicU8; 256] = [const { AtomicU8::new(0) }; 256];
static BUF_GPS: [AtomicU8; 1024] = [const { AtomicU8::new(0) }; 1024];
static BUF_CPS: [AtomicU8; 256] = [const { AtomicU8::new(0) }; 256];
static RX: [Ring<'static>; UART_COUNT] = [Ring::new(&BUF_BT), Ring::new(&BUF_GPS), Ring::new(&BUF_CPS)];
static RX_BYTES: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];
static LINE_ERRORS: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];
static TX_BYTES: [AtomicU32; UART_COUNT] = [const { AtomicU32::new(0) }; UART_COUNT];

pub fn init(id: usize, baud: u32) {
    let u = &HW[id];
    irq::disable(u.irq);
    modify(u.en_reg, 0, u.en_bit);
    wr(u.base + USART_CTRL1, 0);
    RX[id].reset();
    u.tx.output(true, Mode::AfPp); // idle high before the peripheral takes the pin
    u.rx.config(Mode::InPu);
    let clk = if u.apb2 { clock::APB2_HZ } else { clock::APB1_HZ };
    wr(u.base + USART_BAUDR, (clk + baud / 2) / baud);
    rd(u.base + USART_STS);
    rd(u.base + USART_DT);
    wr(u.base + USART_CTRL1, USART_CTRL1_UEN | USART_CTRL1_TEN | USART_CTRL1_REN | USART_CTRL1_RDBFIEN);
    irq::enable(u.irq, PRIO);
}

/// Whether this port reaches `baud` within 2 % from its bus clock (the divisor is 16 at least).
pub fn rate_ok(id: usize, baud: u32) -> bool {
    let clk = if HW[id].apb2 { clock::APB2_HZ } else { clock::APB1_HZ };
    let div = (clk + baud / 2) / baud;
    div >= 16 && (clk / div).abs_diff(baud) * 50 <= baud
}

/// SND-14: the TX pin a GPIO output held high, the UART still enabled (its receive works; what it shifts out
/// reaches no pin). Stock holds PC10 so outside programming mode: a speaker mic takes its bias from it. Call after
/// [`flush`]; [`init`] puts the pin back on the UART, so call again after it. Never low: that would cut the bias.
pub fn tx_pin_quiet(id: usize) {
    HW[id].tx.output(true, Mode::OutPp);
}

/// The TX pin back on the UART (idle high: no edge on the way).
pub fn tx_pin_live(id: usize) {
    HW[id].tx.output(true, Mode::AfPp);
}

/// Until the last byte written has left the pin (before a rate change).
pub fn flush(id: usize) {
    let base = HW[id].base;
    if rd(base + USART_CTRL1) & USART_CTRL1_UEN != 0 {
        while rd(base + USART_STS) & USART_STS_TDC == 0 {}
    }
}

/// Off, TX pin back to an input so a powered-down module is not fed.
pub fn deinit(id: usize) {
    let u = &HW[id];
    irq::disable(u.irq);
    wr(u.base + USART_CTRL1, 0);
    u.tx.config(Mode::InFloat);
    u.rx.config(Mode::InFloat);
    RX[id].clear();
}

pub fn write(id: usize, data: &[u8]) {
    let base = HW[id].base;
    if rd(base + USART_CTRL1) & USART_CTRL1_UEN == 0 {
        return;
    }
    TX_BYTES[id].fetch_add(data.len() as u32, Relaxed);
    for &b in data {
        while rd(base + USART_STS) & USART_STS_TDBE == 0 {}
        wr(base + USART_DT, b as u32);
    }
}

pub fn getc(id: usize) -> Option<u8> {
    RX[id].get()
}

/// Bytes received and not yet taken (the cable's flow control watches it).
pub fn rx_count(id: usize) -> u16 {
    RX[id].count()
}

pub fn stats(id: usize) -> UartStats {
    UartStats {
        rx_bytes: RX_BYTES[id].load(Relaxed),
        tx_bytes: TX_BYTES[id].load(Relaxed),
        dropped: RX[id].dropped(),
        line_errors: LINE_ERRORS[id].load(Relaxed),
    }
}

/// The interrupt handler of port `id`.
pub fn isr(id: usize) {
    let base = HW[id].base;
    let sts = rd(base + USART_STS);
    if sts & (USART_STS_RDBF | USART_STS_ERR) != 0 {
        let b = rd(base + USART_DT) as u8; // STS then DT read clears the flags
        if sts & USART_STS_ERR != 0 {
            LINE_ERRORS[id].fetch_add(1, Relaxed);
        }
        if sts & USART_STS_RDBF != 0 {
            RX_BYTES[id].fetch_add(1, Relaxed);
            RX[id].put(b);
        }
    }
}
