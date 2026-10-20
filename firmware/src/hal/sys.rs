//! Chip identity, reset cause, reboot, the start-up quiesce, and the main stack's high-water mark.
use super::regs::*;
use super::{irq, modify, rd, rd16, wr};
use crate::report::SysOpt;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub const RESET_PIN: u32 = 1 << 0;
pub const RESET_POR: u32 = 1 << 1;
pub const RESET_SW: u32 = 1 << 2;
pub const RESET_WDT: u32 = 1 << 3;

const NOT_LATCHED: u32 = u32::MAX;
static CAUSE: AtomicU32 = AtomicU32::new(NOT_LATCHED);

/// RESET_* bits, latched at the first call; the flags are then cleared.
pub fn reset_cause() -> u32 {
    if CAUSE.load(Relaxed) == NOT_LATCHED {
        CAUSE.store(rd(CRM_CTRLSTS) >> 26 & 0x3F, Relaxed);
        modify(CRM_CTRLSTS, 0, CRM_CTRLSTS_RSTFC);
    }
    CAUSE.load(Relaxed)
}

pub fn flash_kb() -> u32 {
    rd16(DEV_FLASH_KB) as u32
}

/// SELF-8: the 96-bit factory unique ID (stock V0.27 and V0.29 read it there; hardware.md), lowest word first.
pub fn uid() -> [u32; 3] {
    [rd(DEV_UID), rd(DEV_UID + 4), rd(DEV_UID + 8)]
}

/// DEBUG_IDCODE: which MCU this really is (V2 boards have the marking erased).
pub fn idcode() -> u32 {
    rd(DEBUG_IDCODE)
}

/// SCB CPUID: core type and revision.
pub fn cpuid() -> u32 {
    rd(SCB_CPUID)
}

/// Option bytes as stored, plus the live protection flag.
pub fn options() -> SysOpt {
    SysOpt { fap: rd16(USD_FAP), ssb: rd16(USD_SSB), eopb0: rd16(USD_EOPB0), fap_active: rd(FLASH_USD) & FLASH_USD_FAP != 0 }
}

/// SYSRESETREQ: back through the bootloader.
pub fn reboot() -> ! {
    cortex_m::asm::dsb();
    wr(SCB_AIRCR, 0x05FA_0004);
    cortex_m::asm::dsb();
    loop {}
}

/// Start-up, before RAM is initialised: the bootloader's SysTick off, every interrupt disabled and
/// cleared, PRIGROUP 0. Touches no statics.
pub fn quiesce() {
    wr(SYST_CSR, 0);
    irq::disable_all();
    wr(SCB_AIRCR, 0x05FA_0000);
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

/// WFE also wakes on an interrupt that becomes pending while interrupts are masked (the idle loop sleeps
/// masked, so the time asleep can be measured).
pub fn wake_on_pending() {
    modify(SCB_SCR, 0, SCB_SCR_SEVONPEND);
}

/// The MCU's own flash, read only (0x08000000-0x080FFFFF): bootloader, this image, and the stock tail.
pub fn mcu_flash(addr: u32, len: u32) -> Option<&'static [u8]> {
    const START: u32 = 0x0800_0000;
    const END: u32 = 0x0810_0000;
    if !(START..END).contains(&addr) || len > END - addr {
        return None;
    }
    // SAFETY: the range was just checked to lie inside the MCU flash, which is always mapped, readable and
    // never written by this image, so a shared 'static slice of it is sound.
    Some(unsafe { core::slice::from_raw_parts(addr as *const u8, len as usize) })
}

// ---- main stack high-water mark: every task and handler runs on the one main stack ----

const PAINT: u32 = 0x5A5A_5A5A;

extern "C" {
    static _stack_end: u32; // cortex-m-rt: the lowest stack address, just above .bss and .uninit
}

fn stack_bottom() -> usize {
    core::ptr::addr_of!(_stack_end) as usize
}

/// SELF-9.2: the statics' RAM (.data, .bss, .uninit: from the start of SRAM to the stack's lowest address).
pub fn statics() -> u32 {
    (stack_bottom() - 0x2000_0000) as u32
}

/// Fill the unused part of the main stack with a pattern. Once, early in main.
pub fn paint_stack() {
    let top = cortex_m::register::msp::read() as usize - 64; // keep clear of this function's own frame
    for p in (stack_bottom()..top).step_by(4) {
        // SAFETY: between the end of the statics and below the live stack: memory no Rust object occupies.
        unsafe { write_volatile(p as *mut u32, PAINT) };
    }
}

/// Bytes of the main stack never used since `paint_stack`.
pub fn stack_free() -> u32 {
    let bottom = stack_bottom();
    let mut p = bottom;
    // SAFETY: reads RAM from the bottom of the stack upwards; it stops at the first word the stack has
    // used, which is at the latest the current stack pointer, all inside RAM.
    while unsafe { read_volatile(p as *const u32) } == PAINT {
        p += 4;
    }
    (p - bottom) as u32
}
