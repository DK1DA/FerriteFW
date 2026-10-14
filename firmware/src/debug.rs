//! The terminal's memory reads (TERM-22), done in the UI task where the terminal runs. Only addresses that exist
//! are read, so a typo can't fault the radio: the ranges are `ferrite_core::memmap`'s (flash, SRAM, the
//! information block, the AT32F403A's peripheral blocks, the Cortex-M system control space), host-tested.
//! Peripheral blocks take 32-bit reads only (`mem32`): some reads have side effects (a UART's data register gives
//! up its byte), which the help says.

#[cfg(feature = "bench")]
use ferrite_core::memmap;
use ferrite_core::msg::DebugAns;
#[cfg(feature = "bench")]
use ferrite_core::msg::{DebugReq, DEBUG_MEM};

/// Answers a `mem` or `mem32` request, or says why not. A bench build's (TERM-38).
#[cfg(feature = "bench")]
pub fn mem(r: DebugReq) -> DebugAns {
    let DebugReq::Mem { addr, n, words } = r else { return DebugAns::Refused("not a memory request") };
    let n = (n as usize).min(DEBUG_MEM);
    // TERM-22: only what `memmap` lists (the reference manual's map), checked again here whatever the terminal did.
    if let Err(why) = memmap::check(addr, n as u32, words) {
        return DebugAns::Refused(why);
    }
    let mut data = [0u8; DEBUG_MEM];
    if words {
        for (i, w) in data[..n].chunks_mut(4).enumerate() {
            // SAFETY: an aligned word inside memory or a peripheral block `memmap` lists.
            let v = unsafe { core::ptr::read_volatile((addr + i as u32 * 4) as *const u32) };
            w.copy_from_slice(&v.to_le_bytes());
        }
    } else {
        for (i, b) in data[..n].iter_mut().enumerate() {
            // SAFETY: a byte inside flash, the information block or SRAM, as `memmap` lists.
            *b = unsafe { core::ptr::read_volatile((addr + i as u32) as *const u8) };
        }
    }
    DebugAns::Mem { addr, data, n: n as u8 }
}

/// TERM-30: the GPS module's last GGA sentence (kept by the GPS task) and its age.
#[inline(never)] // in .slow_text (memory.x): the terminal's, never from an interrupt
pub fn gga() -> DebugAns {
    let now = embassy_time::Instant::now().as_millis() as u32;
    crate::chan::GGA.lock(|g| {
        let g = g.borrow();
        DebugAns::Gga { data: g.data, n: g.n, age_s: now.wrapping_sub(g.at_ms) / 1000 }
    })
}
