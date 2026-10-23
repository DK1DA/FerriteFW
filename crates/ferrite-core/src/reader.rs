//! The reader image's protocol (BACKUP-9.1, BACKUP-9.4, BACKUP-9.5; decision 264): its console lines, `fdump`'s
//! frames and the CRC-32 they carry. Byte for byte what the hwtest reader spoke (fw/src/core/fdump.h,
//! fw/apps/hwtest/svc.c), so Ferrite Console, fw/tools/backup.py and flashdump.py read either image unchanged.
//!
//! The console (115200 baud, 8N1): a line per command, echoed as typed, each answer ending with the `> ` prompt.
//! - `sys`: one line with ` uid ` and the MCU's 96-bit unique ID, three words lowest first (24 hex digits).
//! - `flash`: `flash id EF4016 (Winbond, 4096 KB) status 00 uid ...  cal sector sum ...`.
//! - `flashcrc`: the radio's own CRC-32 of the whole chip, `flash crc32 1A2B3C4D over 4096 KB, ...`.
//! - `fdump <addr> <len> [baud]`: the binary dump below, at `baud` (500000 when left out).
//! - `reader`: the backup screen ("reader screen on"); `reboot`: back through the bootloader; `off`.
//!
//! `fdump`:
//! 1. The answer is one line, `fdump <addr> <len> <baud> ready, ...` (or `fdump: ...` with the reason it won't);
//!    then the radio switches to `baud`.
//! 2. The host switches too and sends `FDS1` until the radio answers `FDOK` (3 s, else the radio goes back).
//! 3. Blocks of up to [`BLOCK`] bytes, each a frame: [`HDR`] bytes of header (`F` `D` type 0, address u32,
//!    length u16, little-endian), the data, then the CRC-32 (zlib's) of header and data, u32 LE. The host answers
//!    ACK (next block), NAK (send it again) or CAN (stop); silence for [`ACK_MS`] also resends, [`TRIES`] sends at
//!    most per frame.
//! 4. An end frame: type `E`, the start address, length 16, then start u32, length u32, the CRC-32 of all the
//!    data, the number of resends u32; acknowledged like a block.
//! 5. The radio goes back to 115200 and prints `fdump end crc32 XXXXXXXX ...` (or `fdump stopped at ...`).

pub const BLOCK: usize = 4096;
pub const HDR: usize = 10;
pub const ACK: u8 = 0x06;
pub const NAK: u8 = 0x15;
pub const CAN: u8 = 0x18;
pub const TRIES: u32 = 10;
pub const ACK_MS: u32 = 2000;
pub const SYNC_MS: u32 = 3000;
pub const TYPE_BLOCK: u8 = b'B';
pub const TYPE_END: u8 = b'E';
/// `fdumpz`'s frame: one [`ZCHUNK`] of the range, coded (BACKUP-9.6): a mode byte, then the data.
pub const TYPE_ZBLOCK: u8 = b'Z';
/// `fdumpz`'s chunk: 8 KB read, coded and sent as one frame.
pub const ZCHUNK: usize = 8192;
/// The coded chunk's room: the mode byte and the chunk raw (the coder falls back to raw when it can't do better).
pub const ZBUF: usize = ZCHUNK + 1;
pub const Z_RAW: u8 = 0;
pub const Z_ERASED: u8 = 1;
pub const Z_RLE: u8 = 2;
pub const SYNC: [u8; 4] = *b"FDS1";
pub const SYNC_OK: [u8; 4] = *b"FDOK";
/// The console's rate, and `fdump`'s when none is given.
pub const CONSOLE_BAUD: u32 = 115_200;
pub const FDUMP_BAUD: u32 = 500_000;

/// The help text. Ferrite Console tells the reader image from others by its "fdump <addr> <len>" (READER_MARK).
pub const HELP: &[u8] = b"sys | flash | flashcrc   (SPI flash, read only)\r\n\
fdump <addr> <len> [baud]   (SPI flash, read only, binary: fw/tools/flashdump.py --fast)\r\n\
fdumpz <addr> <len> [baud]  (as fdump, 8 KB chunks coded: erased, run-length or raw)\r\n\
reader | reboot | off\r\n";

// ---- CRC-32 ----

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

/// A named static (fast flash: firmware/memory.x's rule for tables read per byte).
static CRC_TABLE: [u32; 256] = table();

/// zlib's CRC-32, continued from `crc` (start at 0): `crc32(crc32(0, a), b) == crc32(0, a ++ b)`. hwtest's
/// `crc32_update`, so `flashcrc` and the frames agree with Console's own CRC of what it read.
pub fn crc32(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

// ---- frames ----

/// A frame's header.
pub fn header(kind: u8, addr: u32, len: u16) -> [u8; HDR] {
    let a = addr.to_le_bytes();
    let l = len.to_le_bytes();
    [b'F', b'D', kind, 0, a[0], a[1], a[2], a[3], l[0], l[1]]
}

/// The end frame's payload.
pub fn end_payload(start: u32, len: u32, data_crc: u32, resends: u32) -> [u8; 16] {
    let mut p = [0u8; 16];
    for (i, v) in [start, len, data_crc, resends].into_iter().enumerate() {
        p[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    p
}

/// The CRC that closes a frame: header and data.
pub fn frame_crc(hdr: &[u8], data: &[u8]) -> [u8; 4] {
    crc32(crc32(0, hdr), data).to_le_bytes()
}

/// Watches the cable for the host's `FDS1`, with any noise before it.
#[derive(Default)]
pub struct Sync {
    got: usize,
}

impl Sync {
    /// True once the whole `FDS1` has come.
    pub fn feed(&mut self, b: u8) -> bool {
        self.got = if b == SYNC[self.got] { self.got + 1 } else if b == SYNC[0] { 1 } else { 0 };
        if self.got == SYNC.len() {
            self.got = 0;
            return true;
        }
        false
    }
}

/// The cable and a clock, as `fdump` needs them (the radio's UART4 and cycle counter; a scripted host in the tests).
pub trait Link {
    fn write(&mut self, b: &[u8]);
    /// The next byte within `ms`, or None.
    fn getc(&mut self, ms: u32) -> Option<u8>;
    /// Drops whatever has come and not been taken.
    fn clear_rx(&mut self);
    /// Milliseconds, monotonic.
    fn ms(&mut self) -> u32;
    fn pause(&mut self, ms: u32);
}

/// A finished dump.
#[derive(Debug, PartialEq)]
pub struct Dumped {
    pub crc: u32,
    pub blocks: u32,
    pub resends: u32,
    pub ms: u32,
}

/// Why a dump stopped, and where.
#[derive(Debug, PartialEq)]
pub struct Stopped {
    pub at: u32,
    pub why: &'static str,
}

/// BACKUP-9.6: a chunk coded into `out` (at least [`ZBUF`] long), the smallest of: [`Z_ERASED`] (every byte 0xFF, the
/// mode byte alone), [`Z_RLE`] (PackBits: a control byte 0-127 is that plus one literal bytes, 128-255 is a run of
/// that minus 125, 3 to 130, of the next byte) and [`Z_RAW`]. Returns the coded length.
pub fn zcode(chunk: &[u8], out: &mut [u8]) -> usize {
    if chunk.iter().all(|&b| b == 0xFF) {
        out[0] = Z_ERASED;
        return 1;
    }
    let raw = |out: &mut [u8]| {
        out[0] = Z_RAW;
        out[1..1 + chunk.len()].copy_from_slice(chunk);
        1 + chunk.len()
    };
    let limit = chunk.len(); // coded must beat raw
    let mut o = 1;
    let mut i = 0;
    let mut lit = 0usize; // start of the pending literal run
    let mut lit_n = 0usize;
    macro_rules! flush_lit {
        () => {
            while lit_n > 0 {
                let n = lit_n.min(128);
                if o + 1 + n > limit {
                    return raw(out);
                }
                out[o] = (n - 1) as u8;
                out[o + 1..o + 1 + n].copy_from_slice(&chunk[lit..lit + n]);
                o += 1 + n;
                lit += n;
                lit_n -= n;
            }
        };
    }
    while i < chunk.len() {
        let b = chunk[i];
        let mut run = 1;
        while i + run < chunk.len() && chunk[i + run] == b && run < 130 {
            run += 1;
        }
        if run >= 3 {
            flush_lit!();
            if o + 2 > limit {
                return raw(out);
            }
            out[o] = (run + 125) as u8;
            out[o + 1] = b;
            o += 2;
            i += run;
            lit = i;
        } else {
            if lit_n == 0 {
                lit = i;
            }
            lit_n += 1;
            i += 1;
        }
    }
    flush_lit!();
    out[0] = Z_RLE;
    o
}

/// BACKUP-9.6: a coded chunk back into `out` (its length is the chunk's); false when it doesn't decode to exactly that.
pub fn zdecode(coded: &[u8], out: &mut [u8]) -> bool {
    let Some((&mode, data)) = coded.split_first() else { return false };
    match mode {
        Z_ERASED if data.is_empty() => {
            out.fill(0xFF);
            true
        }
        Z_RAW if data.len() == out.len() => {
            out.copy_from_slice(data);
            true
        }
        Z_RLE => {
            let (mut i, mut o) = (0, 0);
            while i < data.len() {
                let c = data[i] as usize;
                i += 1;
                if c < 128 {
                    let n = c + 1;
                    if i + n > data.len() || o + n > out.len() {
                        return false;
                    }
                    out[o..o + n].copy_from_slice(&data[i..i + n]);
                    i += n;
                    o += n;
                } else {
                    let n = c - 125;
                    if i >= data.len() || o + n > out.len() {
                        return false;
                    }
                    out[o..o + n].fill(data[i]);
                    i += 1;
                    o += n;
                }
            }
            o == out.len()
        }
        _ => false,
    }
}

/// Sends one frame until the host ACKs it: Some(true) ACK, Some(false) cancelled, None out of tries.
fn send_frame(link: &mut impl Link, hdr: &[u8; HDR], data: &[u8], resends: &mut u32) -> Option<bool> {
    let crc = frame_crc(hdr, data);
    for t in 0..TRIES {
        if t > 0 {
            *resends += 1;
        }
        link.clear_rx();
        link.write(hdr);
        link.write(data);
        link.write(&crc);
        match link.getc(ACK_MS) {
            Some(ACK) => return Some(true),
            Some(CAN) => return Some(false),
            _ => {}
        }
    }
    None
}

/// `fdump`'s binary part, after the "ready" line and the switch to its rate: the sync, every block of
/// `addr..addr + len` (`read` fills each from the SPI flash, read only), the end frame. `progress` gets the bytes
/// acknowledged and the blocks so far after each block, and stops the dump by returning false (the power switch
/// turned off). hwtest's `fdump`, step for step.
pub fn fdump_run(
    link: &mut impl Link,
    buf: &mut [u8; BLOCK],
    addr: u32,
    len: u32,
    read: impl FnMut(u32, &mut [u8]),
    progress: impl FnMut(u32, u32) -> bool,
) -> Result<Dumped, Stopped> {
    dump(link, buf, None, addr, len, read, progress)
}

/// `fdumpz` (BACKUP-9.6): `fdump` with [`ZCHUNK`] chunks sent coded ([`zcode`]) as [`TYPE_ZBLOCK`] frames; the
/// frames' CRC covers what goes over the cable, the end frame's CRC-32 the range as read, as `fdump`'s.
pub fn fdumpz_run(
    link: &mut impl Link,
    buf: &mut [u8; ZCHUNK],
    zbuf: &mut [u8; ZBUF],
    addr: u32,
    len: u32,
    read: impl FnMut(u32, &mut [u8]),
    progress: impl FnMut(u32, u32) -> bool,
) -> Result<Dumped, Stopped> {
    dump(link, buf, Some(zbuf), addr, len, read, progress)
}

fn dump(
    link: &mut impl Link,
    buf: &mut [u8],
    mut zbuf: Option<&mut [u8; ZBUF]>,
    addr: u32,
    len: u32,
    mut read: impl FnMut(u32, &mut [u8]),
    mut progress: impl FnMut(u32, u32) -> bool,
) -> Result<Dumped, Stopped> {
    let t0 = link.ms();
    let mut sync = Sync::default();
    let mut synced = false;
    while !synced && link.ms().wrapping_sub(t0) < SYNC_MS {
        if let Some(b) = link.getc(10) {
            synced = sync.feed(b);
        }
    }
    if !synced {
        return Err(Stopped { at: addr, why: "no sync from the host" });
    }
    // Any of the host's repeated FDS1 still on the way is dropped before the first frame.
    link.pause(20);
    link.clear_rx();
    link.write(&SYNC_OK);
    link.pause(20);
    link.clear_rx();
    let (mut done, mut resends, mut crc, mut blocks) = (0u32, 0u32, 0u32, 0u32);
    while done < len {
        let n = (len - done).min(buf.len() as u32);
        let data = &mut buf[..n as usize];
        read(addr + done, data);
        let sent = match zbuf.as_deref_mut() {
            Some(z) => {
                let c = zcode(data, z);
                send_frame(link, &header(TYPE_ZBLOCK, addr + done, c as u16), &z[..c], &mut resends)
            }
            None => send_frame(link, &header(TYPE_BLOCK, addr + done, n as u16), data, &mut resends),
        };
        match sent {
            Some(true) => {}
            Some(false) => return Err(Stopped { at: addr + done, why: "cancelled by the host" }),
            None => return Err(Stopped { at: addr + done, why: "no acknowledgement" }),
        }
        crc = crc32(crc, data);
        done += n;
        blocks += 1;
        if !progress(done, blocks) {
            return Err(Stopped { at: addr + done, why: "the power switch is off" });
        }
    }
    let end = end_payload(addr, len, crc, resends);
    if send_frame(link, &header(TYPE_END, addr, 16), &end, &mut resends) != Some(true) {
        return Err(Stopped { at: addr + done, why: "end frame not acknowledged" });
    }
    Ok(Dumped { crc, blocks, resends, ms: link.ms().wrapping_sub(t0) })
}

/// Whole percent, 100 only at the end.
pub fn percent(done: u32, total: u32) -> u32 {
    if total == 0 || done >= total { 100 } else { (done as u64 * 100 / total as u64) as u32 }
}

// ---- the console ----

/// Typed bytes into a line, as hwtest's `cmdline_feed`.
pub struct LineEdit {
    buf: [u8; 64],
    len: usize,
}

/// What one byte did, and what to echo for it.
#[derive(Debug, PartialEq)]
pub enum Fed {
    Nothing,
    /// Echo this character.
    Echo(u8),
    /// Echo "\b \b".
    Erase,
    /// Echo "\r\n": the line is complete ([`LineEdit::line`]).
    Line,
}

impl Default for LineEdit {
    fn default() -> Self {
        LineEdit { buf: [0; 64], len: 0 }
    }
}

impl LineEdit {
    pub fn feed(&mut self, c: u8) -> Fed {
        match c {
            // The \n of a \r\n pair (or a bare one) after an empty line: nothing.
            b'\n' if self.len == 0 => Fed::Nothing,
            b'\r' | b'\n' => Fed::Line,
            0x08 | 0x7F if self.len > 0 => {
                self.len -= 1;
                Fed::Erase
            }
            0x20..=0x7E if self.len < self.buf.len() - 1 => {
                self.buf[self.len] = c;
                self.len += 1;
                Fed::Echo(c)
            }
            _ => Fed::Nothing,
        }
    }

    pub fn line(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

/// hwtest's `cmd_uint`: decimal, or hex after `0x`.
pub fn uint(s: &[u8]) -> Option<u32> {
    let (base, digits) = match s {
        [b'0', b'x' | b'X', rest @ ..] => (16, rest),
        _ => (10, s),
    };
    if digits.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in digits {
        let d = (c as char).to_digit(base)?;
        v = v.checked_mul(base)?.checked_add(d)?;
    }
    Some(v)
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Empty,
    Help,
    Sys,
    Flash,
    FlashCrc,
    Fdump { addr: u32, len: u32, baud: u32 },
    /// `fdumpz`: as `fdump`, coded (BACKUP-9.6).
    FdumpZ { addr: u32, len: u32, baud: u32 },
    Reader,
    /// `tune <pace bytes> <pace us> <stop bits>`: the dump's pacing and stop bits, for link tests.
    Tune { bytes: u32, us: u32, stop2: bool },
    Reboot,
    Off,
    Unknown,
}

/// A console line as a command; what hwtest wouldn't run is [`Command::Unknown`].
pub fn parse(line: &[u8]) -> Command {
    let mut words = line.split(|&c| c == b' ' || c == b'\t').filter(|w| !w.is_empty());
    let Some(cmd) = words.next() else { return Command::Empty };
    let a: [Option<&[u8]>; 3] = [words.next(), words.next(), words.next()];
    match cmd {
        b"help" | b"?" => Command::Help,
        b"sys" => Command::Sys,
        b"flash" => Command::Flash,
        b"flashcrc" => Command::FlashCrc,
        b"reader" => Command::Reader,
        b"tune" => match (a[0].and_then(uint), a[1].and_then(uint), a[2].and_then(uint)) {
            (Some(bytes), Some(us), Some(stop)) => Command::Tune { bytes, us, stop2: stop == 2 },
            _ => Command::Unknown,
        },
        b"reboot" => Command::Reboot,
        b"off" => Command::Off,
        b"fdump" | b"fdumpz" => {
            let (Some(addr), Some(len)) = (a[0].and_then(uint), a[1].and_then(uint)) else { return Command::Unknown };
            let baud = match a[2] {
                None => FDUMP_BAUD,
                Some(b) => match uint(b) {
                    Some(b) => b,
                    None => return Command::Empty, // hwtest: a bad rate is ignored silently
                },
            };
            if cmd == b"fdumpz" { Command::FdumpZ { addr, len, baud } } else { Command::Fdump { addr, len, baud } }
        }
        _ => Command::Unknown,
    }
}

// ---- the answers ----

/// Where a line goes (the cable on the radio, a Vec in the tests).
pub trait Out {
    fn put(&mut self, b: &[u8]);
}

/// Upper-case hex, `digits` at least.
pub fn hex(o: &mut impl Out, v: u32, digits: usize) {
    let mut s = [0u8; 8];
    let mut n = 0;
    let mut x = v;
    while n < 8 && (x != 0 || n < digits.max(1)) {
        s[7 - n] = b"0123456789ABCDEF"[(x & 15) as usize];
        x >>= 4;
        n += 1;
    }
    o.put(&s[8 - n..]);
}

pub fn dec(o: &mut impl Out, v: u32) {
    let mut s = [0u8; 10];
    let mut n = 0;
    let mut x = v;
    loop {
        s[9 - n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
        if x == 0 {
            break;
        }
    }
    o.put(&s[10 - n..]);
}

pub const NL: &[u8] = b"\r\n";

/// `sys`: the image, the clock and the MCU, then ` uid ` and the 96-bit unique ID (three words, lowest first).
pub fn sys_line(o: &mut impl Out, version: &str, mhz: u32, idcode: u32, flash_kb: u32, uid: [u32; 3]) {
    o.put(b"Ferrite reader ");
    o.put(version.as_bytes());
    o.put(b"  clock ");
    dec(o, mhz);
    o.put(b" MHz  mcu idcode ");
    hex(o, idcode, 8);
    o.put(b"  flash ");
    dec(o, flash_kb);
    o.put(b" KB  uid ");
    for w in uid {
        hex(o, w, 8);
    }
    o.put(NL);
}

/// What `flash` reports about the chip.
pub struct FlashInfo {
    pub jedec: u32,
    pub status: u8,
    pub uid: [u8; 8],
    /// The byte sum and blankness of the calibration sector (0x00F000-0x00FFFF), read only.
    pub cal_sum: u32,
    pub cal_blank: bool,
}

/// `flash`: hwtest's line; Console takes the ID, vendor and size from it.
pub fn flash_line(o: &mut impl Out, f: &FlashInfo) {
    o.put(b"flash id ");
    hex(o, f.jedec, 6);
    o.put(b" (");
    o.put(crate::chipid::flash_vendor(f.jedec).as_bytes());
    o.put(b", ");
    dec(o, crate::chipid::flash_bytes(f.jedec) / 1024);
    o.put(b" KB) status ");
    hex(o, f.status as u32, 2);
    o.put(b" uid ");
    for b in f.uid {
        hex(o, b as u32, 2);
    }
    o.put(b"  cal sector sum ");
    hex(o, f.cal_sum, 8);
    if f.cal_blank {
        o.put(b" (blank)");
    }
    o.put(NL);
}

/// `flashcrc`'s result.
pub fn flashcrc_line(o: &mut impl Out, crc: u32, bytes: u32, ms: u32, busy_ms: u32) {
    o.put(b"flash crc32 ");
    hex(o, crc, 8);
    o.put(b" over ");
    dec(o, bytes / 1024);
    o.put(b" KB, ");
    dec(o, ms / 1000);
    o.put(b" s (read + CRC ");
    dec(o, ((bytes / 1024) * 1000).checked_div(busy_ms).unwrap_or(0));
    o.put(b" KB/s)");
    o.put(NL);
}

/// Why an `fdump` won't run, before anything changes.
pub fn fdump_refusal(o: &mut impl Out, addr: u32, len: u32, chip: u32) -> bool {
    if chip == 0 || addr >= chip || len == 0 || len > chip - addr {
        o.put(b"fdump: range ");
        hex(o, addr, 6);
        o.put(b"+");
        hex(o, len, 1);
        o.put(b" outside the ");
        dec(o, chip / 1024);
        o.put(b" KB chip");
        o.put(NL);
        return true;
    }
    false
}

pub fn fdump_bad_rate(o: &mut impl Out, baud: u32, pclk: u32) {
    o.put(b"fdump: ");
    dec(o, baud);
    o.put(b" baud not within 2% from ");
    dec(o, pclk);
    o.put(b" Hz (500000, 250000, 115200 are exact)");
    o.put(NL);
}

pub fn fdump_ready(o: &mut impl Out, addr: u32, len: u32, baud: u32, mhz: u32) {
    o.put(b"fdump ");
    hex(o, addr, 6);
    o.put(b" ");
    hex(o, len, 1);
    o.put(b" ");
    dec(o, baud);
    o.put(b" ready, clock ");
    dec(o, mhz);
    o.put(b" MHz");
    o.put(NL);
}

pub fn fdump_end(o: &mut impl Out, crc: u32, len: u32, blocks: u32, resends: u32, ms: u32) {
    o.put(b"\r\nfdump end crc32 ");
    hex(o, crc, 8);
    o.put(b", ");
    dec(o, len);
    o.put(b" bytes in ");
    dec(o, blocks);
    o.put(b" blocks, ");
    dec(o, resends);
    o.put(b" resends, ");
    dec(o, ms);
    o.put(b" ms");
    o.put(NL);
}

pub fn fdump_stopped(o: &mut impl Out, at: u32, why: &str) {
    o.put(b"\r\nfdump stopped at ");
    hex(o, at, 6);
    o.put(b": ");
    o.put(why.as_bytes());
    o.put(b"; back to ");
    dec(o, CONSOLE_BAUD);
    o.put(b" baud");
    o.put(NL);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    impl Out for Vec<u8> {
        fn put(&mut self, b: &[u8]) {
            self.extend_from_slice(b)
        }
    }

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut v = Vec::new();
        f(&mut v);
        String::from_utf8(v).unwrap()
    }

    /// BACKUP-9.5: the reader's CRC is zlib's, continued across pieces, as hwtest's `crc32_update` and Console's.
    #[test]
    fn crc32_is_zlibs_and_continues() {
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(0, b""), 0);
        let data: Vec<u8> = (0..10_000u32).map(|i| (i * 31 + 7) as u8).collect();
        let whole = crc32(0, &data);
        let mut c = 0;
        for piece in data.chunks(256) {
            c = crc32(c, piece);
        }
        assert_eq!(c, whole);
        // A blank 4 MB chip, as `flashcrc` would report it.
        let mut c = 0;
        let ff = [0xFFu8; 4096];
        for _ in 0..1024 {
            c = crc32(c, &ff);
        }
        assert_eq!(c, crc32(0, &std::vec![0xFFu8; 4 << 20]));
    }

    /// BACKUP-9.5: frames as fdump.h builds them (header LE, CRC over header and data, the end payload).
    #[test]
    fn frames_match_fdump_h() {
        let h = header(TYPE_BLOCK, 0x0012_3456, 0x1000);
        assert_eq!(h, [b'F', b'D', b'B', 0, 0x56, 0x34, 0x12, 0x00, 0x00, 0x10]);
        let data = [1u8, 2, 3];
        let mut all = h.to_vec();
        all.extend_from_slice(&data);
        assert_eq!(frame_crc(&h, &data), crc32(0, &all).to_le_bytes());
        let e = end_payload(0, 0x40_0000, 0xDEAD_BEEF, 3);
        assert_eq!(&e[..4], &[0, 0, 0, 0]);
        assert_eq!(&e[4..8], &[0, 0, 0x40, 0]);
        assert_eq!(&e[8..12], &[0xEF, 0xBE, 0xAD, 0xDE]);
        assert_eq!(&e[12..], &[3, 0, 0, 0]);
        assert_eq!(header(TYPE_END, 0, 16), [b'F', b'D', b'E', 0, 0, 0, 0, 0, 16, 0]);
    }

    /// BACKUP-9.5: the host's FDS1 is found after noise, repeated, or split oddly.
    #[test]
    fn sync_after_noise() {
        let mut s = Sync::default();
        let hits: Vec<bool> = b"\x00xFDFFDS1".iter().map(|&b| s.feed(b)).collect();
        assert_eq!(hits.iter().filter(|&&h| h).count(), 1);
        assert!(*hits.last().unwrap());
        let mut s = Sync::default();
        assert!(!b"FDS".iter().any(|&b| s.feed(b)));
        assert!(!s.feed(b'2'));
        assert!(!b"FDS".iter().any(|&b| s.feed(b)));
        assert!(s.feed(b'1'));
    }

    /// BACKUP-9.5: the commands Console and backup.py send, as hwtest parsed them.
    #[test]
    fn commands_parse_like_hwtest() {
        assert_eq!(parse(b"fdump 0x0 0x400000 500000"), Command::Fdump { addr: 0, len: 0x40_0000, baud: 500_000 });
        assert_eq!(parse(b"fdump 0xF000 4096"), Command::Fdump { addr: 0xF000, len: 4096, baud: FDUMP_BAUD });
        assert_eq!(parse(b"  fdump  0X10 0x20  115200 "), Command::Fdump { addr: 0x10, len: 0x20, baud: 115_200 });
        assert_eq!(parse(b"fdump 0x10"), Command::Unknown);
        assert_eq!(parse(b"fdump 0xG 1"), Command::Unknown);
        assert_eq!(parse(b"sys"), Command::Sys);
        assert_eq!(parse(b"flash"), Command::Flash);
        assert_eq!(parse(b"flashcrc"), Command::FlashCrc);
        assert_eq!(parse(b"reader"), Command::Reader);
        assert_eq!(parse(b"reboot"), Command::Reboot);
        assert_eq!(parse(b""), Command::Empty);
        assert_eq!(parse(b"write 0 0"), Command::Unknown);
        assert_eq!(uint(b"4294967295"), Some(u32::MAX));
        assert_eq!(uint(b"4294967296"), None);
        assert_eq!(uint(b"0x"), None);
        assert_eq!(uint(b"0xffffFFFF"), Some(u32::MAX));
    }

    /// BACKUP-9.5: typed bytes echo as hwtest's console did; CR LF ends a line once.
    #[test]
    fn line_editing() {
        let mut e = LineEdit::default();
        assert_eq!(e.feed(b'\n'), Fed::Nothing);
        assert_eq!(e.feed(b's'), Fed::Echo(b's'));
        assert_eq!(e.feed(b'x'), Fed::Echo(b'x'));
        assert_eq!(e.feed(0x7F), Fed::Erase);
        assert_eq!(e.feed(0x01), Fed::Nothing);
        assert_eq!(e.feed(b'y'), Fed::Echo(b'y'));
        assert_eq!(e.feed(b'\r'), Fed::Line);
        assert_eq!(e.line(), b"sy");
        e.clear();
        assert_eq!(e.feed(b'\n'), Fed::Nothing);
        assert_eq!(e.feed(b'\r'), Fed::Line);
        assert_eq!(e.line(), b"");
        e.clear();
        for _ in 0..100 {
            e.feed(b'a');
        }
        assert_eq!(e.line().len(), 63);
    }

    /// BACKUP-9.4, BACKUP-9.5: the lines Console reads, in hwtest's format (Console's patterns: ` uid ` and 24 hex
    /// digits; `flash id XXXXXX (vendor, N KB)`; `flash crc32 XXXXXXXX over N KB`; `fdump XXXXXX X N ready`).
    #[test]
    fn answers_are_hwtests_lines() {
        let s = text(|o| sys_line(o, "v0.1.0", 120, 0x7005_0344, 1024, [0x3F2A_91C0, 0x7D44_0123, 0x89AB_CDEF]));
        assert_eq!(s, "Ferrite reader v0.1.0  clock 120 MHz  mcu idcode 70050344  flash 1024 KB  uid 3F2A91C07D44012389ABCDEF\r\n");
        assert!(!s.contains("Ferrite terminal"));
        let s = text(|o| sys_line(o, "v0.1.0", 120, 0, 0, [1, 0, 0xA]));
        assert!(s.ends_with(" uid 00000001000000000000000A\r\n"));
        let f = FlashInfo { jedec: 0xEF4016, status: 0, uid: [0xA0, 1, 2, 3, 4, 5, 6, 0xFF], cal_sum: 0x1234, cal_blank: false };
        assert_eq!(
            text(|o| flash_line(o, &f)),
            "flash id EF4016 (Winbond, 4096 KB) status 00 uid A0010203040506FF  cal sector sum 00001234\r\n"
        );
        let f = FlashInfo { jedec: 0x5E4016, status: 0x1C, cal_blank: true, ..f };
        assert!(text(|o| flash_line(o, &f)).starts_with("flash id 5E4016 (Zbit, 4096 KB) status 1C "));
        assert!(text(|o| flash_line(o, &f)).ends_with(" (blank)\r\n"));
        assert_eq!(
            text(|o| flashcrc_line(o, 0x1A2B_3C4D, 4 << 20, 52_300, 4_000)),
            "flash crc32 1A2B3C4D over 4096 KB, 52 s (read + CRC 1024 KB/s)\r\n"
        );
        assert_eq!(text(|o| fdump_ready(o, 0, 0x40_0000, 500_000, 120)), "fdump 000000 400000 500000 ready, clock 120 MHz\r\n");
        assert_eq!(text(|o| fdump_ready(o, 0xF000, 0x1000, 250_000, 120)), "fdump 00F000 1000 250000 ready, clock 120 MHz\r\n");
        assert_eq!(
            text(|o| fdump_end(o, 0xCBF4_3926, 9, 1, 0, 12)),
            "\r\nfdump end crc32 CBF43926, 9 bytes in 1 blocks, 0 resends, 12 ms\r\n"
        );
        assert_eq!(
            text(|o| fdump_stopped(o, 0x1000, "cancelled by the host")),
            "\r\nfdump stopped at 001000: cancelled by the host; back to 115200 baud\r\n"
        );
        assert!(String::from_utf8_lossy(HELP).contains("fdump <addr> <len>"));
    }

    /// A host as Console's `Fdump` is: FDS1 until FDOK, each frame checked and answered. `bad` frames (by number,
    /// counting resends) are answered NAK, `silent` ones not at all (a lost ACK), `cancel_at` with CAN.
    struct Host {
        to_radio: std::collections::VecDeque<u8>,
        from_radio: Vec<u8>,
        synced: bool,
        frames: u32,
        bad: Vec<u32>,
        silent: Vec<u32>,
        cancel_at: Option<u32>,
        data: Vec<u8>,
        end: Option<[u8; 16]>,
        now: u32,
        clears: u32,
    }

    impl Host {
        fn new() -> Self {
            Host {
                to_radio: b"\x00noiseFDS1FDS1".iter().copied().collect(),
                from_radio: Vec::new(),
                synced: false,
                frames: 0,
                bad: Vec::new(),
                silent: Vec::new(),
                cancel_at: None,
                data: Vec::new(),
                end: None,
                now: 0,
                clears: 0,
            }
        }

        /// A whole frame from the radio: checked and answered.
        fn take_frame(&mut self) {
            let b = &self.from_radio;
            if b.len() < HDR {
                return;
            }
            let n = u16::from_le_bytes([b[8], b[9]]) as usize;
            if b.len() < HDR + n + 4 {
                return;
            }
            let frame: Vec<u8> = self.from_radio.drain(..HDR + n + 4).collect();
            let k = self.frames;
            self.frames += 1;
            assert_eq!(&frame[..2], b"FD");
            let ok = frame_crc(frame[..HDR].try_into().unwrap(), &frame[HDR..HDR + n]) == frame[HDR + n..];
            assert!(ok, "frame {k}: bad CRC");
            if self.cancel_at == Some(k) {
                return self.to_radio.push_back(CAN);
            }
            if self.silent.contains(&k) {
                return;
            }
            if self.bad.contains(&k) {
                return self.to_radio.push_back(NAK);
            }
            match frame[2] {
                TYPE_BLOCK => {
                    let a = u32::from_le_bytes(frame[4..8].try_into().unwrap());
                    assert_eq!(a as usize, self.data.len(), "blocks in order, no repeats");
                    self.data.extend_from_slice(&frame[HDR..HDR + n]);
                }
                TYPE_END => self.end = Some(frame[HDR..HDR + 16].try_into().unwrap()),
                t => panic!("frame type {t}"),
            }
            self.to_radio.push_back(ACK);
        }
    }

    impl Link for Host {
        fn write(&mut self, b: &[u8]) {
            if !self.synced {
                assert_eq!(b, b"FDOK");
                self.synced = true;
                return;
            }
            self.from_radio.extend_from_slice(b);
            self.take_frame();
        }
        fn getc(&mut self, ms: u32) -> Option<u8> {
            let b = self.to_radio.pop_front();
            if b.is_none() {
                self.now += ms;
            }
            b
        }
        fn clear_rx(&mut self) {
            self.clears += 1;
            if self.synced {
                self.to_radio.clear();
            }
        }
        fn ms(&mut self) -> u32 {
            self.now
        }
        fn pause(&mut self, ms: u32) {
            self.now += ms;
        }
    }

    fn chip(a: u32) -> u8 {
        (a.wrapping_mul(2_654_435_761) >> 13) as u8
    }

    fn run(host: &mut Host, addr: u32, len: u32) -> Result<Dumped, Stopped> {
        let mut buf = [0u8; BLOCK];
        let mut last = 0;
        fdump_run(
            host,
            &mut buf,
            addr,
            len,
            |a, d| d.iter_mut().enumerate().for_each(|(i, x)| *x = chip(a + i as u32)),
            |done, _| {
                assert!(done > last);
                last = done;
                true
            },
        )
    }

    /// BACKUP-9.6: every coding round-trips, picks erased/RLE/raw by size, and never beats raw's room.
    #[test]
    fn zcode_round_trips() {
        let mut cases: [[u8; ZCHUNK]; 5] = [[0xFF; ZCHUNK], [0; ZCHUNK], [0; ZCHUNK], [0; ZCHUNK], [0xFF; ZCHUNK]];
        for (i, b) in cases[2].iter_mut().enumerate() {
            *b = chip(i as u32); // noise: raw
        }
        for (i, b) in cases[3].iter_mut().enumerate() {
            *b = if i % 300 < 200 { 0xFF } else { chip(i as u32) }; // mixed
        }
        cases[4][100] = 0x12; // one byte off erased
        let modes = [Z_ERASED, Z_RLE, Z_RAW, Z_RLE, Z_RLE];
        for (c, m) in cases.iter().zip(modes) {
            let mut z = [0u8; ZBUF];
            let n = zcode(c, &mut z);
            assert_eq!(z[0], m);
            assert!(n <= ZBUF);
            let mut back = [0u8; ZCHUNK];
            assert!(zdecode(&z[..n], &mut back));
            assert_eq!(&back[..], &c[..]);
        }
        let mut back = [0u8; 10];
        assert!(!zdecode(&[Z_RLE, 0x85, 0xAA], &mut back)); // 8 bytes, not 10
        assert!(zdecode(&[Z_RLE, 0x87, 0xAA], &mut back) && back == [0xAA; 10]);
        assert_eq!(parse(b"fdumpz 0 0x400000 460800"), Command::FdumpZ { addr: 0, len: 0x40_0000, baud: 460_800 });
    }

    /// BACKUP-9.5: a whole dump against a host like Console's: every byte, the end frame's CRC and count.
    #[test]
    fn fdump_whole_range() {
        let mut h = Host::new();
        let len = 3 * BLOCK as u32 + 100;
        let r = run(&mut h, 0, len).unwrap();
        let want: Vec<u8> = (0..len).map(chip).collect();
        assert_eq!(h.data, want);
        assert_eq!(r.crc, crc32(0, &want));
        assert_eq!((r.blocks, r.resends), (4, 0));
        assert_eq!(h.end.unwrap(), end_payload(0, len, crc32(0, &want), 0));
    }

    /// BACKUP-9.5: a NAK and a lost ACK each resend the frame (counted), and the data is still whole and in order.
    #[test]
    fn fdump_resends() {
        let mut h = Host::new();
        h.bad = std::vec![1];
        h.silent = std::vec![2]; // block 1 sent again, and its ACK lost
        let len = 2 * BLOCK as u32;
        let r = run(&mut h, 0, len).unwrap();
        assert_eq!(h.data, (0..len).map(chip).collect::<Vec<_>>());
        assert_eq!(r.resends, 2);
        assert_eq!(u32::from_le_bytes(h.end.unwrap()[12..].try_into().unwrap()), 2);
    }

    /// BACKUP-9.5: CAN stops at once; no sync, or a host that never answers, stops with the reason.
    #[test]
    fn fdump_stops() {
        let mut h = Host::new();
        h.cancel_at = Some(1);
        assert_eq!(run(&mut h, 0, 4 * BLOCK as u32), Err(Stopped { at: BLOCK as u32, why: "cancelled by the host" }));
        let mut h = Host::new();
        h.to_radio = b"FDS2FD".iter().copied().collect();
        assert_eq!(run(&mut h, 0, 10), Err(Stopped { at: 0, why: "no sync from the host" }));
        assert!(h.now >= SYNC_MS);
        let mut h = Host::new();
        h.silent = (0..TRIES).collect();
        assert_eq!(run(&mut h, 0, 10), Err(Stopped { at: 0, why: "no acknowledgement" }));
        assert_eq!(h.frames, TRIES);
    }

    /// BACKUP-9.5: a dump past the chip is refused before the rate changes, as hwtest did.
    #[test]
    fn fdump_range_refusals() {
        let chip = 4 << 20;
        assert!(!fdump_refusal(&mut Vec::new(), 0, chip, chip));
        assert!(!fdump_refusal(&mut Vec::new(), chip - 1, 1, chip));
        for (a, l) in [(chip, 1), (0, 0), (0, chip + 1), (chip - 1, 2), (1, u32::MAX)] {
            let s = text(|o| assert!(fdump_refusal(o, a, l, chip)));
            assert!(s.starts_with("fdump: range ") && s.contains("outside the 4096 KB chip"), "{s}");
        }
        assert!(fdump_refusal(&mut Vec::new(), 0, 1, 0), "no chip, no dump");
        assert_eq!(text(|o| fdump_bad_rate(o, 7, 60_000_000)), "fdump: 7 baud not within 2% from 60000000 Hz (500000, 250000, 115200 are exact)\r\n");
    }

    #[test]
    fn percent_and_numbers() {
        assert_eq!(percent(0, 100), 0);
        assert_eq!(percent(99, 100), 99);
        assert_eq!(percent(4 << 20, 4 << 20), 100);
        assert_eq!(percent((4 << 20) - 1, 4 << 20), 99);
        assert_eq!(percent(1, 0), 100);
        assert_eq!(text(|o| hex(o, 0, 1)), "0");
        assert_eq!(text(|o| hex(o, 0xABC, 6)), "000ABC");
        assert_eq!(text(|o| hex(o, u32::MAX, 2)), "FFFFFFFF");
        assert_eq!(text(|o| dec(o, 0)), "0");
        assert_eq!(text(|o| dec(o, u32::MAX)), "4294967295");
    }
}
