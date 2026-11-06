//! SAFE-8 (decision 364): the independent watchdog (WDT, RM 16), from LICK (30-60 kHz). Started once the tasks
//! are about to run and fed by the thread executor's loop; a reset of any kind stops it again (RM 4.2.1: every
//! system reset sets the WDT's registers back), so the stock bootloader and the updater never meet it. Only the
//! option bytes' hardware watchdog would survive one, and Ferrite never writes them.
use super::{rd, wr};

const WDT_CMD: u32 = 0x4000_3000;
const WDT_DIV: u32 = 0x4000_3004;
const WDT_RLD: u32 = 0x4000_3008;
const WDT_STS: u32 = 0x4000_300C;
const CMD_RELOAD: u32 = 0xAAAA;
const CMD_UNLOCK: u32 = 0x5555;
const CMD_ENABLE: u32 = 0xCCCC;
/// LICK / 64 and the full 12-bit count: 6.5 s at LICK's nominal 40 kHz, 4.4-8.7 s across its range. The thread
/// executor turns at least every 10 ms (the Link task's loop), so only a hang or a fault ever lets it run out.
const DIV_64: u32 = 4;
const RLD_MAX: u32 = 0xFFF;

/// On, from the full count.
pub fn start() {
    wr(WDT_CMD, CMD_UNLOCK);
    wr(WDT_DIV, DIV_64);
    wr(WDT_RLD, RLD_MAX);
    while rd(WDT_STS) & 3 != 0 {} // DIVF, RLDF: the values crossed into the LICK domain
    wr(WDT_CMD, CMD_RELOAD);
    wr(WDT_CMD, CMD_ENABLE);
}

/// Back to the full count.
#[inline(always)]
pub fn feed() {
    wr(WDT_CMD, CMD_RELOAD);
}
