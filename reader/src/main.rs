//! # The reader image
//!
//! What Ferrite Console loads through the stock bootloader to back a radio up before anything else (BACKUP-9.1,
//! BACKUP-9.3; decision 264, replacing fw/apps/hwtest). It reads, never writes (BACKUP-9.5): the SPI flash only
//! through [`flash::ReadOnly`] over w25q built without its `write` feature, so program, erase and status writes
//! aren't in the image at all; nothing here touches the MCU's own flash controller.
//!
//! The cable (UART4, 115200) speaks hwtest's console and its `fdump` (ferrite_core::reader has the protocol and
//! its tests): `sys` (the MCU's unique ID), `flash`, `fdump`, `flashcrc`, `reader`, `reboot`, `off`. The screen
//! says "Backing up your radio" with the progress (BACKUP-9.4, ferrite_ui::reader).
//!
//! One thread, no executor: a loop polls the cable, the power switch and a running `flashcrc`. The only interrupt
//! is the cable's receive (into a ring). The HAL, the board's pins and safe state, and the panel bus are the
//! firmware's own files (`#[path]` below), so the reader starts the radio exactly as Ferrite does.
#![no_std]
#![no_main]

#[allow(dead_code)]
#[path = "../../firmware/src/board.rs"]
mod board;
mod bringup;
#[allow(dead_code)]
#[path = "../../firmware/src/bus.rs"]
mod bus;
mod chan;
mod flash;
#[allow(dead_code, unexpected_cfgs)]
#[path = "../../firmware/src/hal/mod.rs"]
mod hal;
#[path = "../../firmware/src/report.rs"] // allows its own dead code
mod report;
#[allow(dead_code)]
#[path = "../../firmware/src/ring.rs"]
mod ring;
mod screen;

use board::{POWER_HOLD, POWER_SW};
use hal::gpio::Mode;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use cortex_m_rt::{entry, exception, ExceptionFrame};
use ferrite_core::reader::{self as proto, Command, Fed, LineEdit, Out};
use ferrite_ui::reader::{self as ui, Phase};
use hal::{clock, sys, time, uart};
use report::UART_CPS;

// ---- start-up (as firmware/src/main.rs) ----

// Entered by a jump from the bootloader: interrupts masked, thread mode on MSP and privileged, then the safe state
// and the bootloader's SysTick and interrupts off, before RAM is initialised.
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

/// Model tag at 0x080033E0: the stock updater and bootloader check it.
#[link_section = ".model_tag"]
#[used]
#[no_mangle]
static MODEL_TAG: [u8; 16] = *b"RT-950      \0\0\0\0";

/// The same tag at 0x080037E0: the bootloader compares it at every power-on ("Firmware Error!" when it differs).
#[link_section = ".model_tag_boot"]
#[used]
#[no_mangle]
static MODEL_TAG_BOOT: [u8; 16] = *b"RT-950      \0\0\0\0";

type Vector = unsafe extern "C" fn();

/// 96 peripheral vectors: the cable's receive, every other one the fault path.
#[link_section = ".vector_table.interrupts"]
#[no_mangle]
#[used]
static __INTERRUPTS: [Vector; 96] = {
    let mut v: [Vector; 96] = [stray; 96];
    v[hal::irq::UART4 as usize] = uart4;
    v
};

extern "C" fn stray() {
    board::fault()
}
extern "C" fn uart4() {
    uart::isr(UART_CPS)
}

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

// ---- time ----

/// Milliseconds since start-up, from the cycle counter (which wraps every 36 s at 120 MHz: called far more often).
fn now_ms() -> u32 {
    static LAST: AtomicU32 = AtomicU32::new(0);
    static MS: AtomicU32 = AtomicU32::new(0);
    static PART: AtomicU32 = AtomicU32::new(0);
    let c = time::cycles();
    let d = c.wrapping_sub(LAST.swap(c, Relaxed)) as u64 + PART.load(Relaxed) as u64;
    let per = clock::mhz() as u64 * 1000;
    PART.store((d % per) as u32, Relaxed);
    MS.fetch_add((d / per) as u32, Relaxed).wrapping_add((d / per) as u32)
}

// ---- the cable ----

struct Cable;

impl Out for Cable {
    fn put(&mut self, b: &[u8]) {
        uart::write(UART_CPS, b)
    }
}

/// `fdump`'s view of the cable, at its own rate.
struct Dump;

/// Bytes between short pauses while a frame goes out. Sent with no gap at all (the polled loop at 120 MHz keeps the
/// data register full), the CH340 on a Mac loses bursts of 3-14 bytes about once every 30 frames (Oct 22, black, at
/// 460800 and 500000 alike: the frame comes up short and runs into the next one). hwtest, at 8 MHz, left a gap
/// between bytes and never did.
static PACE_BYTES: AtomicU32 = AtomicU32::new(64);
static PACE_US: AtomicU32 = AtomicU32::new(100);
static STOP2: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

impl proto::Link for Dump {
    fn write(&mut self, b: &[u8]) {
        let (n, us) = (PACE_BYTES.load(Relaxed).max(1) as usize, PACE_US.load(Relaxed));
        for chunk in b.chunks(n) {
            uart::write(UART_CPS, chunk);
            if us > 0 {
                time::delay_us(us);
            }
        }
    }
    fn getc(&mut self, ms: u32) -> Option<u8> {
        let t0 = now_ms();
        loop {
            if let Some(c) = uart::getc(UART_CPS) {
                return Some(c);
            }
            if now_ms().wrapping_sub(t0) >= ms {
                return None;
            }
        }
    }
    fn clear_rx(&mut self) {
        uart::clear_rx(UART_CPS)
    }
    fn ms(&mut self) -> u32 {
        now_ms()
    }
    fn pause(&mut self, ms: u32) {
        let t0 = now_ms();
        while now_ms().wrapping_sub(t0) < ms {}
    }
}

// ---- the power switch ----

/// PE0 high (switched off) for 50 ms: as Ferrite (PWR-17) and hwtest, the power hold is released.
#[derive(Default)]
struct PowerSwitch {
    off_since: Option<u32>,
}

impl PowerSwitch {
    fn off(&mut self) -> bool {
        if !POWER_SW.read() {
            self.off_since = None;
            return false;
        }
        let t = now_ms();
        let since = *self.off_since.get_or_insert(t);
        t.wrapping_sub(since) >= 50
    }
}

/// The radio off: safe state, the screen dark, the power hold released. Still running half a second later (the
/// switch back on): a reset, which starts the reader again through the bootloader.
fn power_off(why: &[u8]) -> ! {
    Cable.put(why);
    uart::flush(UART_CPS);
    cortex_m::interrupt::disable();
    board::safe_state();
    hal::backlight::set(0);
    POWER_HOLD.write(false);
    time::delay_ms_busy(500);
    cortex_m::peripheral::SCB::sys_reset()
}

// ---- the reader ----

/// A running `flashcrc` (4 KB per pass of the loop, so the cable and the power switch stay live, as hwtest's).
struct Crc {
    addr: u32,
    size: u32,
    crc: u32,
    t0: u32,
    busy_ms: u32,
}

struct Reader {
    lcd: screen::Screen,
    flash: flash::ReadOnly,
    edit: LineEdit,
    /// The backup screen is up (Console's `reader`, or a dump or check started).
    backup_screen: bool,
    crc: Option<Crc>,
    power: PowerSwitch,
    buf: [u8; proto::BLOCK],
    zchunk: [u8; proto::ZCHUNK],
    zbuf: [u8; proto::ZBUF],
}

impl Reader {
    fn backup_screen(&mut self) {
        if !self.backup_screen {
            self.backup_screen = true;
            ui::draw(&mut self.lcd);
        }
    }

    fn poll(&mut self) {
        if self.power.off() {
            power_off(b"\r\npower switch off: releasing PA11\r\n");
        }
        while let Some(c) = uart::getc(UART_CPS) {
            match self.edit.feed(c) {
                Fed::Nothing => {}
                Fed::Echo(c) => Cable.put(&[c]),
                Fed::Erase => Cable.put(b"\x08 \x08"),
                Fed::Line => {
                    Cable.put(proto::NL);
                    let cmd = proto::parse(self.edit.line());
                    self.edit.clear();
                    self.run(cmd);
                    Cable.put(b"> ");
                }
            }
        }
        self.crc_step();
    }

    fn run(&mut self, cmd: Command) {
        match cmd {
            Command::Empty => {}
            Command::Help => Cable.put(proto::HELP),
            Command::Sys => {
                proto::sys_line(&mut Cable, ferrite_core::VERSION, clock::mhz(), sys::idcode(), sys::flash_kb(), sys::uid())
            }
            Command::Flash => self.flash_info(),
            Command::FlashCrc => self.crc_start(),
            Command::Fdump { addr, len, baud } => self.fdump(addr, len, baud, false),
            Command::FdumpZ { addr, len, baud } => self.fdump(addr, len, baud, true),
            Command::Tune { bytes, us, stop2 } => {
                PACE_BYTES.store(bytes, Relaxed);
                PACE_US.store(us, Relaxed);
                STOP2.store(stop2, Relaxed);
                Cable.put(b"tune set\r\n");
            }
            Command::Reader => {
                self.backup_screen();
                Cable.put(b"reader screen on\r\n");
            }
            Command::Reboot => {
                Cable.put(b"rebooting\r\n");
                uart::flush(UART_CPS);
                time::delay_ms_busy(50);
                sys::reboot()
            }
            Command::Off => {
                Cable.put(b"power: releasing PA11\r\n");
                uart::flush(UART_CPS);
                time::delay_ms_busy(300);
                POWER_HOLD.write(false);
                time::delay_ms_busy(300);
                // Still here: the switch is on, and only it cuts the power.
                POWER_HOLD.output(true, Mode::OutPp);
                Cable.put(b"power: still running (expected while the power switch is on)\r\n");
            }
            Command::Unknown => Cable.put(b"?  (help)\r\n"),
        }
    }

    fn flash_info(&mut self) {
        self.flash.probe();
        let (mut sum, mut blank) = (0u32, true);
        let mut b = [0u8; 256];
        for a in (0xF000..0x10000).step_by(b.len()) {
            self.flash.read(a, &mut b); // the calibration sector: read, summed, nothing more
            for &x in &b {
                sum = sum.wrapping_add(x as u32);
                blank &= x == 0xFF;
            }
        }
        let info = proto::FlashInfo {
            jedec: self.flash.jedec(),
            status: self.flash.status(),
            uid: self.flash.unique_id(),
            cal_sum: sum,
            cal_blank: blank,
        };
        proto::flash_line(&mut Cable, &info);
    }

    fn crc_start(&mut self) {
        if self.crc.is_some() {
            return;
        }
        self.flash.probe();
        let size = self.flash.bytes();
        if size == 0 {
            Cable.put(b"flash crc: size unknown from id ");
            proto::hex(&mut Cable, self.flash.jedec(), 6);
            Cable.put(b", not run\r\n");
            return;
        }
        self.backup_screen();
        ui::progress(&mut self.lcd, Phase::Checking, 0, size);
        self.crc = Some(Crc { addr: 0, size, crc: 0, t0: now_ms(), busy_ms: 0 });
    }

    fn crc_step(&mut self) {
        let Some(c) = self.crc.as_mut() else { return };
        let t = now_ms();
        let n = (c.size - c.addr).min(proto::BLOCK as u32);
        self.flash.read(c.addr, &mut self.buf[..n as usize]);
        c.crc = proto::crc32(c.crc, &self.buf[..n as usize]);
        c.addr += n;
        c.busy_ms += now_ms().wrapping_sub(t);
        if c.addr % 0x10000 == 0 || c.addr >= c.size {
            ui::progress(&mut self.lcd, Phase::Checking, c.addr, c.size);
        }
        if c.addr >= c.size {
            proto::flashcrc_line(&mut Cable, c.crc, c.size, now_ms().wrapping_sub(c.t0), c.busy_ms);
            self.crc = None;
        }
    }

    fn fdump(&mut self, addr: u32, len: u32, baud: u32, z: bool) {
        self.flash.probe();
        let chip = self.flash.bytes();
        if proto::fdump_refusal(&mut Cable, addr, len, chip) {
            return;
        }
        if !clock::tree().baud_ok(baud) {
            proto::fdump_bad_rate(&mut Cable, baud, clock::apb_hz());
            return;
        }
        // The screen first: once "ready" is out, the host switches rate and starts its sync.
        self.backup_screen();
        ui::progress(&mut self.lcd, Phase::Reading, 0, len);
        proto::fdump_ready(&mut Cable, addr, len, baud, clock::mhz());
        uart::flush(UART_CPS);
        uart::set_rate(UART_CPS, baud);
        uart::set_stop2(UART_CPS, STOP2.load(Relaxed));
        uart::clear_rx(UART_CPS);
        let Reader { lcd, flash, power, buf, zchunk, zbuf, .. } = self;
        let mut progress = |done: u32, _blocks: u32| {
            // Every 64 KB, and at the end.
            if done.is_multiple_of(0x10000) || done == len {
                ui::progress(lcd, Phase::Reading, done, len);
            }
            !power.off()
        };
        let r = if z {
            proto::fdumpz_run(&mut Dump, zchunk, zbuf, addr, len, |a, d| flash.read(a, d), &mut progress)
        } else {
            proto::fdump_run(&mut Dump, buf, addr, len, |a, d| flash.read(a, d), &mut progress)
        };
        uart::flush(UART_CPS);
        uart::set_stop2(UART_CPS, false);
        uart::set_rate(UART_CPS, proto::CONSOLE_BAUD);
        // Console switches back 50-100 ms after the last ACK: the result line waits for it (hwtest's came at once,
        // and Console then waited out its 3 s for a line it had missed).
        proto::Link::pause(&mut Dump, 150);
        uart::clear_rx(UART_CPS);
        match r {
            Ok(d) => proto::fdump_end(&mut Cable, d.crc, len, d.blocks, d.resends, d.ms),
            Err(s) => proto::fdump_stopped(&mut Cable, s.at, s.why),
        }
        // What the cable's receiver saw since start-up: line errors by kind, bytes the ring had no room for.
        let (fe, ore) = uart::errors(UART_CPS);
        let st = uart::stats(UART_CPS);
        for (k, v) in [(&b"uart rx "[..], st.rx_bytes), (b" framing ", fe), (b" overrun ", ore), (b" ring dropped ", st.dropped)] {
            Cable.put(k);
            let mut d = [0u8; 10];
            let mut n = v;
            let mut i = d.len();
            loop {
                i -= 1;
                d[i] = b'0' + (n % 10) as u8;
                n /= 10;
                if n == 0 {
                    break;
                }
            }
            Cable.put(&d[i..]);
        }
        Cable.put(b"\r\n");
    }
}

#[entry]
fn main() -> ! {
    board::init();
    uart::init(UART_CPS, proto::CONSOLE_BAUD);
    // SAFETY: start-up is done; the cable's receive interrupt may run from here.
    unsafe { cortex_m::interrupt::enable() };
    let mut lcd = screen::Screen::new();
    ui::waiting(&mut lcd);
    let mut r = Reader {
        lcd,
        flash: flash::ReadOnly::new(),
        edit: LineEdit::default(),
        backup_screen: false,
        crc: None,
        power: PowerSwitch::default(),
        buf: [0; proto::BLOCK],
        zchunk: [0; proto::ZCHUNK],
        zbuf: [0; proto::ZBUF],
    };
    Cable.put(b"\r\nFerrite reader ");
    Cable.put(ferrite_core::VERSION.as_bytes());
    Cable.put(b" (read only): type help for commands\r\n> ");
    loop {
        r.poll();
    }
}
