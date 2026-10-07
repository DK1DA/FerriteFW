//! The terminal's memory reads (TERM-22), done in the UI task where the terminal runs. Only addresses that exist
//! are read, so a typo can't fault the radio: flash, SRAM, the peripheral blocks of the AT32F403A, the
//! system memory (option bytes, unique ID) and the Cortex-M system control space. Peripheral blocks take
//! 32-bit reads only (`mem32`): byte reads of some registers misbehave, and some reads have side effects
//! (a UART's data register gives up its byte), which the help says.

use core::ops::Range;
use ferrite_core::msg::{DebugAns, DebugReq, DEBUG_MEM};

/// Byte-readable memory.
const PLAIN: [Range<u32>; 3] = [
    0x0800_0000..0x0810_0000, // flash, 1 MB
    0x1FFF_B000..0x1FFF_F810, // system memory: bootloader ROM, option bytes, unique ID
    0x2000_0000..0x2001_8000, // SRAM, 96 KB (EOPB0 = 1, never changed)
];

/// Peripheral blocks (1 KB each, base addresses from the reference manual), word reads only.
const BLOCKS: [u32; 44] = [
    // APB1
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
const SCS: Range<u32> = 0xE000_E000..0xE000_F000;

fn word_ok(a: u32) -> bool {
    BLOCKS.iter().any(|&b| (b..b + 0x400).contains(&a)) || SCS.contains(&a)
}

/// Answers a `mem` or `mem32` request, or says why not.
pub fn mem(r: DebugReq) -> DebugAns {
    let DebugReq::Mem { addr, n, words } = r else { return DebugAns::Refused("not a memory request") };
    let n = (n as usize).min(DEBUG_MEM);
    let Some(end) = addr.checked_add(n as u32) else { return DebugAns::Refused("past the end of memory") };
    let plain = PLAIN.iter().any(|m| m.start <= addr && end <= m.end);
    let periph = (addr..end).step_by(4).all(word_ok) && word_ok(end - 1);
    let mut data = [0u8; DEBUG_MEM];
    if words && addr.is_multiple_of(4) && n.is_multiple_of(4) && (plain || periph) {
        for (i, w) in data[..n].chunks_mut(4).enumerate() {
            // SAFETY: an aligned word inside memory or a peripheral block listed above.
            let v = unsafe { core::ptr::read_volatile((addr + i as u32 * 4) as *const u32) };
            w.copy_from_slice(&v.to_le_bytes());
        }
    } else if !words && plain {
        for (i, b) in data[..n].iter_mut().enumerate() {
            // SAFETY: a byte inside flash, system memory or SRAM, listed above.
            *b = unsafe { core::ptr::read_volatile((addr + i as u32) as *const u8) };
        }
    } else if periph {
        return DebugAns::Refused("peripheral registers: use mem32 (word reads)");
    } else {
        return DebugAns::Refused("not readable: flash 08000000, RAM 20000000 (96 KB), peripherals 4000xxxx (mem32)");
    }
    DebugAns::Mem { addr, data, n: n as u8 }
}
