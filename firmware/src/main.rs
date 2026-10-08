//! # Ferrite
//!
//! Open firmware for the Radtel RT-950 Pro. This is the start-up and the wiring: the safe state before anything
//! else, the vector table, three executors, and the six tasks (`tasks`), each the only owner of its hardware.
//!
//! | Executor | Priority | Tasks |
//! | --- | --- | --- |
//! | interrupt, IRQ 56 | 5 (highest of the three) | Radio |
//! | interrupt, IRQ 57 | 6 | DSP, input sampler |
//! | thread (WFE when idle) | lowest | UI, Link, Storage, GPS |
//!
//! Above them: UART receive (3) and the time driver (4). Any fault, stray interrupt or panic ends in
//! `board::fault`: interrupts off, safe state, red LED, stop.
#![no_std]
#![no_main]

mod board;
mod bringup;
mod bus;
mod chan;
mod debug;
mod display;
#[allow(dead_code)] // the prototype's HAL, carried over whole; not every part has a user yet
mod hal;
mod report;
#[allow(dead_code)]
mod ring;
mod store;
mod tasks;

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use cortex_m_rt::{entry, exception, ExceptionFrame};
use embassy_executor::{raw, InterruptExecutor, Spawner};
use ferrite_core::chipid;
use hal::irq::Irq;
use report::{UART_BT, UART_CPS, UART_GPS};

// ---- start-up ----

// Entered by a jump from the bootloader, not a reset. cortex-m-rt has loaded our stack pointer and VTOR;
// before RAM is initialised: interrupts masked, thread mode on MSP and privileged (CONTROL cleared), then the
// safe state and the bootloader's SysTick and interrupts off. before_ram touches no statics.
core::arch::global_asm!(
    ".global __pre_init",
    ".type __pre_init,%function",
    ".thumb_func",
    "__pre_init:",
    "cpsid i",
    "movs r0, #0",
    "msr control, r0",
    "isb",
    "b before_ram",
);

#[no_mangle]
extern "C" fn before_ram() {
    board::before_ram();
}

/// Model tag at 0x080033E0: the stock updater and bootloader check it (`btf.py` refuses images without it).
#[link_section = ".model_tag"]
#[used]
#[no_mangle]
static MODEL_TAG: [u8; 16] = *b"RT-950      \0\0\0\0";

/// Spare interrupt lines for the two interrupt executors: DMA2 channels 1 and 2, never clocked.
const IRQ_RADIO: u16 = 56;
const IRQ_DSP: u16 = 57;
// NVIC priorities (lower is more urgent): UART receive 3 (hal::uart), time driver 4, then the executors.
const PRIO_TIME: u8 = 4;
const PRIO_RADIO: u8 = 5;
const PRIO_DSP: u8 = 6;

type Vector = unsafe extern "C" fn();

/// 96 peripheral vectors: the ones Ferrite uses, every other one the fault path.
#[link_section = ".vector_table.interrupts"]
#[no_mangle]
#[used]
static __INTERRUPTS: [Vector; 96] = {
    let mut v: [Vector; 96] = [stray; 96];
    v[hal::time_driver::IRQ as usize] = tmr3;
    v[IRQ_RADIO as usize] = radio_irq;
    v[IRQ_DSP as usize] = dsp_irq;
    v[hal::irq::USART1 as usize] = usart1;
    v[hal::irq::USART3 as usize] = usart3;
    v[hal::irq::UART4 as usize] = uart4;
    v[hal::audio::IRQ_ADC_DMA as usize] = adc_dma;
    v[hal::audio::IRQ_DAC_DMA as usize] = dac_dma;
    v
};

extern "C" fn stray() {
    board::fault()
}
extern "C" fn tmr3() {
    hal::time_driver::on_interrupt()
}
extern "C" fn usart1() {
    hal::uart::isr(UART_BT)
}
extern "C" fn usart3() {
    hal::uart::isr(UART_GPS)
}
extern "C" fn uart4() {
    hal::uart::isr(UART_CPS)
}
extern "C" fn adc_dma() {
    hal::audio::adc_isr()
}
extern "C" fn dac_dma() {
    hal::audio::dac_isr()
}

static RADIO_EXECUTOR: InterruptExecutor = InterruptExecutor::new();
static DSP_EXECUTOR: InterruptExecutor = InterruptExecutor::new();

extern "C" fn radio_irq() {
    // SAFETY: the executor's own interrupt handler, the only place it is polled.
    unsafe { RADIO_EXECUTOR.on_interrupt() }
}
extern "C" fn dsp_irq() {
    // SAFETY: as above.
    unsafe { DSP_EXECUTOR.on_interrupt() }
}

// With panic=immediate-abort a panic is an undefined instruction, so it arrives here as a HardFault.
#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    board::fault_at(Some(ef.pc()))
}

#[exception]
unsafe fn DefaultHandler(_: i16) {
    board::fault()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    board::fault()
}

/// Microseconds the core spent asleep: the CPU-load figure.
pub static IDLE_US: AtomicU32 = AtomicU32::new(0);

#[entry]
fn main() -> ! {
    board::init();
    hal::time::us_init();
    // Transmit (in the tx build) also needs the expected MCU; receive runs regardless.
    let _mcu_ok = hal::sys::idcode() == chipid::EXPECTED_PID;

    hal::sys::paint_stack();
    hal::sys::wake_on_pending();
    hal::time_driver::init(PRIO_TIME);
    hal::irq::set_priority(IRQ_RADIO, PRIO_RADIO);
    hal::irq::set_priority(IRQ_DSP, PRIO_DSP);
    RADIO_EXECUTOR.start(Irq(IRQ_RADIO)).spawn(tasks::radio::task().unwrap());
    let dsp = DSP_EXECUTOR.start(Irq(IRQ_DSP));
    dsp.spawn(tasks::dsp::task().unwrap());
    dsp.spawn(tasks::input::task().unwrap());
    // SAFETY: start-up is done; interrupts may run from here.
    unsafe { cortex_m::interrupt::enable() };
    run_thread_executor(|s| {
        s.spawn(tasks::ui::task().unwrap());
        s.spawn(tasks::storage::task().unwrap());
        s.spawn(tasks::link::task().unwrap());
        s.spawn(tasks::gps::task().unwrap());
    })
}

/// Embassy's thread-mode executor with idle accounting: poll the ready tasks, then sleep until the next event,
/// with interrupts masked (WFE still wakes on them: SEVONPEND) so the time measured is purely asleep.
fn run_thread_executor(init: impl FnOnce(Spawner)) -> ! {
    // embassy-executor's cortex-m pender treats this context as the thread executor: it signals with SEV.
    const THREAD_CONTEXT: *mut () = usize::MAX as *mut ();
    let executor: &'static raw::Executor =
        cortex_m::singleton!(: raw::Executor = raw::Executor::new(THREAD_CONTEXT)).unwrap();
    init(executor.spawner());
    loop {
        // SAFETY: polled only here and never re-entered (the pender only sets the event register).
        unsafe { executor.poll() };
        cortex_m::interrupt::disable();
        let t0 = hal::time::us16();
        cortex_m::asm::wfe(); // returns at once if a task was woken since the poll
        let t1 = hal::time::us16();
        // SAFETY: re-enables what the line above masked; no critical section is open here.
        unsafe { cortex_m::interrupt::enable() };
        IDLE_US.fetch_add(t1.wrapping_sub(t0) as u32, Relaxed);
    }
}
