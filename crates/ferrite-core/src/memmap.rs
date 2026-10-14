//! The terminal's memory reads (TERM-22): which addresses of the AT32F403A may be read, so a typo can't fault
//! the radio. From the reference manual (RM_AT32F403A_407 V2.07, 2 Memory resources, 1.x system architecture):
//! flash, SRAM, the information block (boot memory, the flash size and unique ID, the user system data), the
//! peripheral blocks the radio's MCU has, and the Cortex-M system control space and DBGMCU. Reserved holes
//! between them are refused. Peripheral blocks, the SCS and DBGMCU take 32-bit reads only (`mem32`): byte reads
//! of some registers misbehave, and some reads have side effects (a UART's data register gives up its byte).

use core::ops::Range;

/// Byte-readable memory.
pub const PLAIN: [Range<u32>; 5] = [
    0x0800_0000..0x0810_0000, // main flash, 1 MB
    0x1FFF_B000..0x1FFF_F000, // boot memory (the AT32's own ISP loader), 16 KB
    0x1FFF_F7E0..0x1FFF_F7F4, // F_SIZE and the 96-bit unique ID
    0x1FFF_F800..0x1FFF_F830, // user system data, 48 B (the AT32's option bytes: FAP, SSB, EPP, EOPB0, data)
    0x2000_0000..0x2001_8000, // SRAM, 96 KB (EOPB0 = 1, never changed)
];

/// Peripheral blocks (1 KB each, base addresses from the RM's boundary table), word reads only.
pub const BLOCKS: [u32; 44] = [
    // APB1: TMR2-7, TMR12-14, RTC, WWDT, WDT, SPI2, SPI3, USART2, USART3, UART4, UART5, I2C1, I2C2, BPR, PWC, DAC
    0x4000_0000, 0x4000_0400, 0x4000_0800, 0x4000_0C00, 0x4000_1000, 0x4000_1400, 0x4000_1800, 0x4000_1C00,
    0x4000_2000, 0x4000_2800, 0x4000_2C00, 0x4000_3000, 0x4000_3800, 0x4000_3C00, 0x4000_4400, 0x4000_4800,
    0x4000_4C00, 0x4000_5000, 0x4000_5400, 0x4000_5800, 0x4000_6C00, 0x4000_7000, 0x4000_7400,
    // APB2: IOMUX, EXINT, GPIO A-E, ADC1, ADC2, TMR1, SPI1, TMR8, USART1, ADC3, TMR9-11
    0x4001_0000, 0x4001_0400, 0x4001_0800, 0x4001_0C00, 0x4001_1000, 0x4001_1400, 0x4001_1800, 0x4001_2400,
    0x4001_2800, 0x4001_2C00, 0x4001_3000, 0x4001_3400, 0x4001_3800, 0x4001_3C00, 0x4001_4C00, 0x4001_5000,
    0x4001_5400,
    // AHB: DMA1, DMA2, CRM, flash controller
    0x4002_0000, 0x4002_0400, 0x4002_1000, 0x4002_2000,
];

/// The Cortex-M system control space (SysTick, NVIC, SCB), word reads only.
pub const SCS: Range<u32> = 0xE000_E000..0xE000_F000;
/// DBGMCU: the MCU ID code and debug freeze bits, word reads only.
pub const DBGMCU: Range<u32> = 0xE004_2000..0xE004_2008;

/// The terminal's answer when an address is refused: what can be read.
pub const READABLE: &str = "not readable; readable: flash 08000000-080FFFFF, RAM 20000000-20017FFF, system 1FFFB000 (boot), 1FFFF7E0 (ID), 1FFFF800 (user system data), peripherals 4000xxxx-4002xxxx (mem32)";

fn word_ok(a: u32) -> bool {
    BLOCKS.iter().any(|&b| (b..b + 0x400).contains(&a)) || SCS.contains(&a) || DBGMCU.contains(&a)
}

/// Why a read of `n` bytes from `addr` (`words`: 32-bit reads) is refused, or Ok when it is safe: every byte in
/// one byte-readable range, or every word in a peripheral block (word reads only).
pub fn check(addr: u32, n: u32, words: bool) -> Result<(), &'static str> {
    if n == 0 {
        return Ok(());
    }
    let Some(end) = addr.checked_add(n) else { return Err("past the end of memory") };
    let plain = PLAIN.iter().any(|m| m.start <= addr && end <= m.end);
    let periph = (addr..end).step_by(4).all(word_ok) && word_ok(end - 1);
    if words && !(addr.is_multiple_of(4) && n.is_multiple_of(4)) {
        return Err("mem32 needs a word-aligned address");
    }
    match (plain, periph) {
        (true, _) => Ok(()),
        (false, true) if words => Ok(()),
        (false, true) => Err("peripheral registers: use mem32 (word reads)"),
        _ => Err(READABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TERM-22: flash, RAM, the information block and the peripherals read; holes, the ends and byte reads of
    /// registers are refused.
    #[test]
    fn safe_ranges_only() {
        assert_eq!(check(0x0800_0000, 64, false), Ok(()));
        assert_eq!(check(0x080F_FFC0, 64, false), Ok(()), "flash's last bytes");
        assert!(check(0x080F_FFC1, 64, false).is_err(), "runs past the end of flash");
        assert!(check(0x0810_0000, 4, false).is_err());
        assert_eq!(check(0x2001_7FC0, 64, true), Ok(()));
        assert!(check(0x2001_8000, 4, false).is_err(), "past 96 KB of SRAM");
        // The information block: the boot memory, the ID and the user system data (the STM32 option-byte
        // address, the AT32's USD, 48 B); the reserved space between is refused.
        assert_eq!(check(0x1FFF_B000, 64, false), Ok(()));
        assert!(check(0x1FFF_F000, 4, false).is_err(), "reserved after the boot memory");
        assert_eq!(check(0x1FFF_F7E8, 12, false), Ok(()), "unique ID");
        assert_eq!(check(0x1FFF_F800, 48, false), Ok(()), "user system data");
        assert_eq!(check(0x1FFF_F800, 16, true), Ok(()));
        assert!(check(0x1FFF_F800, 64, false).is_err(), "past its 48 B");
        // Peripherals: words only, inside a block the MCU has.
        assert_eq!(check(0x4001_0808, 8, true), Ok(()), "GPIOA");
        assert_eq!(check(0x4001_0808, 8, false), Err("peripheral registers: use mem32 (word reads)"));
        assert!(check(0x4001_0809, 4, true).is_err(), "unaligned");
        assert!(check(0x4001_2000, 4, true).is_err(), "a reserved block");
        assert_eq!(check(0x4001_13FC, 8, true), Ok(()), "GPIOC's last word on into GPIOD");
        assert!(check(0x4001_1BFC, 8, true).is_err(), "GPIOE's last word on into a reserved block");
        assert_eq!(check(0x4002_1000, 4, true), Ok(()), "CRM");
        assert_eq!(check(0xE004_2000, 4, true), Ok(()), "DBGMCU's IDCODE");
        assert_eq!(check(0xE000_ED00, 4, true), Ok(()), "SCB CPUID");
        // Typos: nothing there.
        for a in [0x0000_0000, 0x0840_0000, 0x3000_0000, 0x5000_0000, 0x6000_0000, 0xA000_0000, 0xFFFF_FFF0] {
            assert_eq!(check(a, 4, true), Err(READABLE), "{a:#x}");
        }
        assert_eq!(check(0xFFFF_FFFC, 8, false), Err("past the end of memory"));
    }
}
