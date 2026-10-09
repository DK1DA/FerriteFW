//! Radio: the only task that touches the BK4829s and their bus, the SI4732, the RF path pins and the PA. Runs on
//! the highest-priority interrupt executor.
//!
//! Three slots on two chips (manual chapters 6, 13, 14): the host-tested engines decide, this task carries out.
//! - [`Watch`] names the slot each chip listens to (WATCH-2), whose audio is heard (WATCH-3) and when watch is
//!   paused or cancelled (WATCH-6, WATCH-11, WATCH-16). The active slot never changes here (WATCH-5).
//! - [`Hop`] moves the one receive path select between the two chips while both have a slot, as stock's dual
//!   watch hops: a chip without its band's front end is taken as hearing nothing (to confirm on the bench).
//! - [`Scan`] walks a slot's band (a range at its step, the fast pass of SCAN-21) while the slot has a chip,
//!   or its channels: a channel scan (zone, zone set) from the set the UI sends, asking the UI for the
//!   frequencies a few stops ahead ([`scan::ChanScans`], shared with the simulators' fake radio).
//! - Battery Save (`ferrite_core::rxsleep`) sleeps both chips together (WATCH-8, as stock's dual watch).
//!
//! Every path select, the PA drive and the band relay change through the path machine (`ferrite_core::path`,
//! SAFE-11): it plans, [`Radio::run`] carries the plan out to the end. Watch, scan and the path hop ask with
//! [`Cause::Auto`], which never moves the relay (SAFE-20, WATCH-10); only the active slot and HF mode do.
//!
//! The bring-up build logs every relay move and path select on the cable (plain byte lines: this task runs from
//! the fast flash and never calls the UI's text code).
//!
//! Transmit is only in the `tx` build (milestone 4): without it the bk4829 crate's transmit methods don't exist,
//! the transmit command does nothing and no transmit path pin or PA drive is ever raised (only a key-up plans
//! one, and only the `tx` build asks for a key-up). With it, every key-up goes through the sequencer
//! (`ferrite_core::txseq`, TX-17) with its own checks (region, calibration, battery, duty, PTT seen released
//! since boot), then the path machine's TX-23 order: PA 0, paths clear, the TX path, the chip to transmit with
//! its read-back (TX-7), the PA ramp to the calibrated level (`ferrite_core::txcal`, stock's lookup).
//!
//! The crossband repeater (manual chapter 19, decision 93) is the `tx` build's too: [`Repeater`] decides, the
//! receive path hops between the two sides while ready, and a pass keys the other chip with the receiving one
//! left in receive (`Path::key_up_xb`), the receiving chip's audio routed into the transmitting chip's mic input
//! as stock does it (PA15 low, PB8 on; `analysis/crossband_repeat.md`, to confirm on the bench).

use crate::board::{AMP_EN, BK_CS, BK_SCK, BK_SDA, RELAY, REPEAT_AUDIO, RF_PINS, SI4732_ADDR, SI_IN, SI_RST, SI_SCL, SI_SDA};
use crate::bus::{BusyDelay, Io, Out, SiBus};
use crate::chan::{ALERTS, DEBUG_ANS, DEBUG_REQ, EVENTS, LIVE, RADIO, TERM_WAKE};
use crate::hal::gpio::Mode;
use crate::hal::i2c::I2c;
use crate::tasks::{check, CHECK_BK0, CHECK_BK1};
use bk4829::wire::ThreeWire;
use bk4829::{int, AfOut, Bandwidth, Bk4829, Demod, Fsk};
use embassy_futures::select::{select3, Either3};
use embassy_time::{Instant, Timer};
use ferrite_core::freq::{self, mhz, Freq10};
use ferrite_core::msg::{bk_write_refused, BroadcastCmd, DebugAns, DebugReq, Event, Kind, LiveAll, ModeLive, Param, RadioCmd, SameDecoded, Source, SLOTS};
use ferrite_core::same_rx::{self, Assembler, Repeats, Step};
use ferrite_core::path::{Act, Cause, Path, Rx, Sel, Steps};
use ferrite_core::rxsleep::{Action, Level, RxSleep};
use ferrite_core::scan::{self, Check, Range, Scan};
use ferrite_core::tonegen;
use ferrite_core::watch::{self, Hop, Watch};
use si4732::{Mode as SiMode, Si4732, Sideband};
#[cfg(feature = "tx")]
use ferrite_core::{
    msg::TxLive,
    txcal::{Gauge, TxCal},
    txplan::{Refusal, Region},
    txseq::{self, Kind as TxKind, Req, TxSeq},
    xb::{self, Cmd as XbCmd, Repeater},
};

type Wire = ThreeWire<Out, Io, BusyDelay>;

/// Start-up frequency until the UI's slots arrive: 2 m calling.
const DEFAULT: Freq10 = freq::mhz(146) + 52_000;
/// Battery Save until settings exist.
const DEFAULT_SAVE: Level = Level::L1;
/// Signal meter and squelch poll while one slot listens alone.
const POLL_MS: u64 = 50;
/// The engines' tick while watch hops or a scan runs: under the fast scan step (15 ms) and the watched dwell
/// (70 ms).
const TICK_MS: u64 = 10;
/// The fast pass (SCAN-21) counts a step as "over the floor" this far above the chip's noise floor (to confirm
/// on the bench).
const FAST_MARGIN_DB: i16 = 10;
/// The HF mode's internal slot index (the chips' `tuned` entry for it).
const HF_SLOT: u8 = 3;
/// The sequencer's settings in the `tx` build: the timeout a little after the UI's own 120 s (the UI unkeys
/// first; this is the backstop), no pre-alert (the UI shows it). End tone, CW ID and tail elimination follow
/// what the UI sends ([`RadioCmd::TxTones`], [`Param::EndTone`], [`Param::Tail`]): see `Radio::tx_cfg`.
#[cfg(feature = "tx")]
const TX_CFG: txseq::Cfg = txseq::Cfg { timeout_s: 125, prealert_s: 0, end_tone: false, cw_id: false, cw_each: false, tail: false };
/// The crossband repeater's receive path turn per side while ready (XB-4: a pass needs 30 ms of open squelch; to
/// confirm on the bench with the unselected front end, which may hear enough to drop the hop).
#[cfg(feature = "tx")]
const XB_TURN_MS: u32 = 50;
/// REG_30 read back after the key-up write (TX-7): stock's transmit value with the microphone.
#[cfg(feature = "tx")]
const REG30_TX_MIC: u16 = 0xC1FE;

/// Speaker amplifier and the shared audio stage (both roles likely, from stock).
fn speaker(on: bool) {
    for p in AMP_EN {
        p.write(on);
    }
}

/// What the Radio task keeps of a slot's settings for receiving (the transmit side is the `tx` build's).
#[derive(Clone, Copy)]
struct SlotRx {
    freq: Freq10,
    narrow: bool,
    /// The receive tone word (CHAN-3): CTCSS in 0.1 Hz, bit 15 DCS, bit 14 inverted; 0 none.
    rx_tone: u16,
    /// The clarifier, Hz (VFO-22): receive only.
    clar: i16,
    /// Squelch 0-9; 0 is open (SQL: always heard, like monitor).
    sq: u8,
    /// AM (Param::Mode 2): receive only (TX-20).
    am: bool,
    /// Tail elimination on receive (TONE-6, [`Param::Tail`] bit 0): with no receive tone, the 55 Hz tail mutes.
    rx_tail: bool,
}

impl SlotRx {
    const DEFAULT: SlotRx = SlotRx { freq: DEFAULT, narrow: false, rx_tone: 0, clar: 0, sq: 3, am: false, rx_tail: false };

    /// Where the chip tunes: the frequency moved by the clarifier (10 Hz units).
    fn rx_freq(&self) -> Freq10 {
        (self.freq as i32 + self.clar as i32 / 10).max(0) as Freq10
    }
}

/// HF mode (manual chapter 20a): the frequency, its mode (as [`Param::Mode`]) and which receiver has it.
#[derive(Clone, Copy, PartialEq)]
struct HfOn {
    freq: Freq10,
    mode: u8,
    /// The SI4732 (below 18 MHz, and AM up to 30 MHz: HF-10); otherwise chip 1 through the energised relay.
    si: bool,
}

struct Radio {
    /// The MCU temperature (SBAR-20): when it was last read, and its smoothed tenths.
    temp: (Option<u32>, Option<i16>),
    si: Si4732<SiBus>,
    wire: Wire,
    cs: [Out; 2],
    chips: u8,
    live: LiveAll,
    save: RxSleep,
    path: Path,
    /// The path machine muted the receiver (around a relay move or while keyed): the squelch doesn't open the
    /// speaker meanwhile.
    muted: bool,
    /// The SI4732 has had the antenna since boot: leaving it sets stock's radio state, PA7 = 1, PC5 = 0 (BCR-20).
    si_used: bool,
    slots: [SlotRx; SLOTS],
    active: u8,
    /// The watch set from the UI, a bit per slot.
    watch_set: u8,
    watcher: Watch,
    hop: Hop,
    scans: [Scan; SLOTS],
    ranges: [Range<'static>; SLOTS],
    /// Channel scans: each slot's set of channel numbers and the frequencies asked for (SCAN-3, SCAN-12,
    /// SCAN-22; decision 86).
    chans: scan::ChanScans,
    /// What each chip is tuned to now: the slot and its receive frequency; None asleep.
    tuned: [Option<(u8, Freq10)>; 2],
    /// Each chip's noise floor (dBm), a slow average of its RSSI with the squelch closed (the fast pass's
    /// reference).
    floor: [i16; 2],
    /// Monitor latched, a bit per slot (SQL-2).
    monitor: u8,
    /// Radio-wide settings the UI sends: scan inputs (SCAN-22, for channel lists) and the subtone level (TONE-16,
    /// the `tx` build's).
    scan_tx: bool,
    tone_level: u8,
    hf: Option<HfOn>,
    /// The frequency counter (TSRCH-10), while on.
    counter: Option<Counter>,
    /// SAME weather alerts (WX-4, DEC-19): on from the UI, and the BK4829 receiver's state.
    same_on: bool,
    same: SameRx,
    #[cfg(feature = "tx")]
    tx: Tx,
    /// The crossband repeater while it runs (168 + 8 bytes).
    #[cfg(feature = "tx")]
    xb: Option<XbRun>,
}

/// The crossband repeater running: the engine, each side's slot, and the receive path's turn while ready.
#[cfg(feature = "tx")]
struct XbRun {
    rep: Repeater,
    sides: [u8; 2],
    hop: u8,
    hop_t: u32,
}

/// The frequency counter: which chip measures now, when its scan started, and its last reading (stock keeps a
/// reading only when the next one agrees, `tonegen::counter_agree`).
#[derive(Clone, Copy)]
struct Counter {
    chip: u8,
    since: u32,
    last: Option<Freq10>,
}

/// The BK4829's SAME receiver (DEC-19): on the chip of a slot tuned to a weather channel, the burst being put
/// together, and the headers already raised. 280 bytes.
struct SameRx {
    chip: Option<u8>,
    /// The slot and frequency it was set up for: a retune sets it up again.
    at: Option<(u8, Freq10)>,
    /// The modem is set up and listening for the sync word.
    armed: bool,
    asm: Assembler,
    repeats: Repeats,
}

/// The weather channels, 162.400-162.550 MHz (WX-1).
const WX: core::ops::RangeInclusive<Freq10> = mhz(162) + 40_000..=mhz(162) + 55_000;

/// The modem in its NOAA SAME receive mode (BK4819 application note; nobody's firmware uses it yet, so every
/// value is bench-confirm, roadmap 6b): REG_58 receive mode 000 with bandwidth 010 and receive gain 3 (bits 7:6
/// set as in both of stock's modem modes), the rate in Tone 2's word (520.83 baud x 10.32444), the four sync
/// bytes `ZCZC`, no CRC (stock's REG_5C with bit 6 clear), and the longest header's length.
const SAME_FSK: Fsk = Fsk {
    mode: 0x03C4,
    tone_word: ((same_rx::BAUD_MILLI as u64 * 1_032_444 + 50_000_000) / 100_000_000) as u16,
    tone_gain: 0,
    preamble: 16,
    sync: same_rx::SYNC,
    sync4: true,
    crc: 0xA730 & !(1 << 6),
    scramble: false,
    len: same_rx::MAX_LEN as u16,
};

/// A chip gives up measuring after this long with no carrier, and the other chip takes a turn (stock's scan time
/// is 0.8 s per reading).
const COUNTER_TURN_MS: u32 = 2_000;
/// Where each chip sits while counting: its band's middle, for the front end the path selects.
const COUNTER_TUNE: [Freq10; 2] = [mhz(146), mhz(446)];

/// The `tx` build's transmit state.
#[cfg(feature = "tx")]
struct Tx {
    seq: TxSeq,
    /// From the UI task once it has read the calibration (`TX_CAL`); None until then, or when blank.
    cal: Option<TxCal>,
    gauge: Gauge,
    /// Each slot's transmit tone word and power level (index 3: HF mode's).
    tone: [u16; 4],
    power: [u8; 4],
    /// The UI's [`RadioCmd::TxPlan`] for the next key-up.
    plan: Option<(Freq10, Region, bool)>,
    /// The key-up now is a data one ([`RadioCmd::TransmitData`]: APRS) at this power level: no end tone or CW ID
    /// after it (TONE-11), the APRS power (0 5 5 4) instead of the slot's.
    data: Option<u8>,
    /// TX-42: every PTT input has read released since boot.
    ptt_ok: bool,
    /// The calibrated PA drive for the key-up being run (`Act::PaRamp`).
    pa: u8,
    /// The read-back failed during a key-up (TX-7): the plan stops and the safe state follows.
    fault: bool,
    adc: bool,
    /// Each slot's end tone (TONE-10: an index into `tonegen::END_TONES`, 0 off) and transmit tail elimination
    /// (TONE-5), the channel's over the radio's as the UI resolved them; the slot keyed now (index 3: HF mode).
    end: [u8; 4],
    tail: [bool; 4],
    slot: u8,
    /// 0 8 5 2's phase (0 55 Hz, 1-3 the CTCSS phase shifts), 0 8 7 2's CW ID (0 off, 1 every 10 min, 2 on unkey)
    /// with the callsign (0 8 7 1; empty: no ID, VOX-10), 0 8 6 6's DTMF sidetone (DTMF-8).
    phase: u8,
    cw: u8,
    cw_call: [u8; 10],
    cw_len: u8,
    sidetone: bool,
    /// The chip keyed now: the tones at unkey play on it after the path machine has planned the unkey.
    chip: Option<u8>,
    /// The region and Extend TX from the UI's last [`RadioCmd::TxPlan`]: the crossband repeater's key-ups use them.
    region: Option<(Region, bool)>,
    /// DTMF-3: each digit's on and off time (ms) and the first digit delay (10 ms); a dialled sequence, and whether
    /// this transmission is one (no end tone or CW ID after it: TONE-11).
    dtmf: (u8, u8),
    digits: [u8; 16],
    dialled: bool,
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Bring-up log: a tag and a number, as plain bytes (no formatter: this task stays in the fast flash).
fn log(tag: &[u8], v: u32) {
    let mut b = [0u8; 48];
    let n = tag.len().min(36);
    b[..n].copy_from_slice(&tag[..n]);
    let mut d = [0u8; 10];
    let mut k = 0;
    let mut x = v;
    loop {
        d[k] = b'0' + (x % 10) as u8;
        k += 1;
        x /= 10;
        if x == 0 || k == 10 {
            break;
        }
    }
    let mut i = n;
    while k > 0 && i < b.len() {
        k -= 1;
        b[i] = d[k];
        i += 1;
    }
    crate::bringup::line(&b[..i]);
}

impl Radio {
    /// The terminal's debug requests (TERM-21 to TERM-24): registers read and written as asked, but never a
    /// transmit or PA bit, nor the chip's GPIO outputs (`bk_write_refused`, checked here as well as in the
    /// terminal). Memory requests are the UI task's.
    fn debug(&mut self, d: DebugReq) -> DebugAns {
        match d {
            DebugReq::BkRead { chip, .. } | DebugReq::BkWrite { chip, .. } if chip > 1 || self.chips & 1 << chip == 0 => {
                DebugAns::Refused("that BK4829 didn't answer at start-up")
            }
            DebugReq::BkRead { chip, reg } if reg < 0x80 => DebugAns::Reg { chip, reg, val: self.chip(chip).read(reg) },
            DebugReq::BkWrite { chip, reg, val } => {
                if let Some(why) = bk_write_refused(reg, val) {
                    return DebugAns::Refused(why);
                }
                self.chip(chip).write(reg, val);
                DebugAns::Reg { chip, reg, val: self.chip(chip).read(reg) }
            }
            DebugReq::Si { cmd, n, reply } => {
                let (n, rn) = ((n as usize).min(8), (reply as usize).clamp(1, 16));
                let mut data = [0u8; 16];
                match self.si.command(&cmd[..n], &mut data[..rn], &mut BusyDelay) {
                    Ok(()) => DebugAns::Si { data, n: rn as u8 },
                    Err(_) => DebugAns::Refused("the SI4732 didn't answer: it is held in reset until FM broadcast or HF mode starts it"),
                }
            }
            _ => DebugAns::Refused("not a Radio task request"),
        }
    }

    fn chip(&mut self, n: u8) -> Bk4829<bk4829::wire::Chip<'_, Out, Io, BusyDelay, Out>> {
        Bk4829::new(self.wire.chip(&mut self.cs[n as usize]))
    }

    fn has(&self, n: u8) -> bool {
        self.chips & 1 << n != 0
    }

    /// Carry out a path plan, waits included, before anything else reaches the pins.
    async fn run(&mut self, steps: &Steps) {
        for &a in steps.as_slice() {
            match a {
                Act::Sel(s, on) => {
                    RF_PINS[s as usize].write(on);
                    if on {
                        log(b"path select ", s as u32);
                    }
                }
                Act::PaZero => {
                    #[cfg(feature = "tx")]
                    crate::hal::pa::zero();
                    #[cfg(not(feature = "tx"))]
                    RF_PINS[6].write(false);
                }
                Act::Relay(on) => {
                    if on != RELAY.read() {
                        log(b"relay ", on as u32);
                    }
                    RELAY.write(on);
                }
                // BCR-20, stock's states: the mode's own state is set when the SI4732 is started (`si_input`);
                // away from it, 1/0 once it has been used, as stock leaves broadcast. Boot and the safe state
                // before any broadcast keep stock's 0/0.
                Act::SiInput(true) => self.si_used = true,
                Act::SiInput(false) => {
                    if self.si_used {
                        SI_IN[0].write(true);
                        SI_IN[1].write(false);
                    }
                }
                Act::Mute(m) => {
                    self.muted = m;
                    if m {
                        speaker(false);
                    }
                }
                Act::Wait(ms) => Timer::after_millis(ms as u64).await,
                // XB-6, XB-8: stock's crossband route, PA15 low with the audio stage (PB8) on; back high after.
                Act::RepeatAudio(on) => {
                    REPEAT_AUDIO.write(!on);
                    if on {
                        AMP_EN[0].write(true);
                    }
                    log(b"repeat audio ", on as u32);
                }
                #[cfg(feature = "tx")]
                Act::ChipTx(n) => {
                    if !self.key_chip(n) {
                        self.tx.fault = true;
                        log(b"tx read-back failed, chip ", n as u32);
                        break; // no PA ramp: the caller puts the safe state next
                    }
                }
                #[cfg(feature = "tx")]
                Act::ChipRx(n) => {
                    if self.has(n) {
                        self.chip(n).tx_off();
                    }
                }
                #[cfg(feature = "tx")]
                Act::PaRamp => {
                    // TX-23's ramp: the calibrated level in four steps over 4 ms.
                    let v = self.tx.pa as u32;
                    for k in 1..=4u32 {
                        crate::hal::pa::set((v * k / 4) as u8);
                        Timer::after_millis(1).await;
                    }
                    log(b"tx pa ", v);
                }
                // TONE-8 order at unkey, still keyed: end tone, CW ID when due, tail elimination (the sequencer
                // plans each only when its setting is on).
                #[cfg(feature = "tx")]
                Act::EndTone => {
                    let e = self.tx.end[self.tx.slot as usize % 4];
                    let t = tonegen::END_TONES.get(e as usize).copied().unwrap_or(&[]);
                    self.play_keyed(t.iter().copied()).await;
                }
                #[cfg(feature = "tx")]
                Act::CwId => {
                    let (call, len) = (self.tx.cw_call, self.tx.cw_len as usize);
                    let cw = tonegen::Cw::new(&call[..len.min(10)], tonegen::CW_WPM);
                    self.play_keyed(cw.map(|(on, ms)| (if on { tonegen::CW_HZ } else { 0 }, ms))).await;
                }
                #[cfg(feature = "tx")]
                Act::TailElim => self.tail_elim().await,
                // Transmit steps: only a key-up plans them, and this build never asks for one.
                #[cfg(not(feature = "tx"))]
                Act::ChipTx(_) | Act::ChipRx(_) | Act::PaRamp | Act::EndTone | Act::CwId | Act::TailElim => {}
            }
        }
        if self.path.take_alarm() {
            // SAFE-22: the event log's "Relay switching too often".
            let mut e = Event::new(Kind::Fault, Source::Radio);
            e.payload = *b"relay switch";
            EVENTS.immediate_publisher().publish_immediate(e);
        }
    }

    /// The receive path for `f`. `ActiveSlot` (tuning or switching the active slot) may move the relay
    /// (SAFE-20); `Auto` (watch, scan, the hop) never does and is refused instead.
    async fn select_rx(&mut self, f: Freq10, cause: Cause) {
        let want = Rx::Bk(Sel::rx(f));
        if self.path.rx_now() == want {
            return;
        }
        let mut s = Steps::default();
        let _ = self.path.rx(want, cause, now_ms() as u32, &mut s); // refused: stays as it is
        self.run(&s).await;
    }

    /// Tune chip `n` to a slot (or sleep it): only when it changes.
    fn tune_chip(&mut self, n: u8, want: Option<(u8, Freq10)>) {
        if !self.has(n) || self.tuned[n as usize] == want {
            return;
        }
        self.tuned[n as usize] = want;
        let Some((s, f)) = want else {
            self.chip(n).sleep();
            return;
        };
        let slot = match s {
            s if s < SLOTS as u8 => self.slots[s as usize],
            // HF mode on chip 1 (18-64 MHz): AM is the mode's own (Param::Mode 2); FM otherwise.
            _ => SlotRx { freq: f, am: self.hf.is_some_and(|h| h.mode == 2), ..SlotRx::DEFAULT },
        };
        let mut bk = self.chip(n);
        bk.wake();
        bk.set_freq(f * 10);
        bk.bandwidth(if slot.narrow { Bandwidth::Narrow } else { Bandwidth::Wide });
        bk.demod(if slot.am { Demod::Am } else { Demod::Fm }); // stock's modes 6 and 7 (VFO-20: AM keeps wide)
        match slot.rx_tone {
            // TONE-6: no tone to lose, so the detector listens for the 55 Hz tail instead (stock: REG_07 0x21CD).
            0 if slot.rx_tail => {
                bk.ctcss_rx(550);
                bk.tail_detect();
            }
            0 => bk.subtone_off(),
            t if t & 0x8000 != 0 => bk.dcs_rx(t & 0x3FFF, t & 0x4000 != 0),
            t => bk.ctcss_rx(t),
        }
        bk.af_out(AfOut::Mute); // the audio decision unmutes the one heard
        bk.rx_on();
    }

    /// SAME (DEC-19, WX-4): the BK4829's own receiver on the chip of a slot tuned to a weather channel, as
    /// long as alerts are on. Each burst's FIFO words go through [`Assembler`]; the first that parses raises
    /// the alert, through the UI task ([`ALERTS`]), and the same text again within `REPEAT_S` doesn't (the
    /// header's other two bursts). The modem is set up again whenever its chip retunes or wakes (Battery Save
    /// keeps its cycle, WX-11).
    fn same_tick(&mut self, now: u32) {
        let tuned = self.tuned;
        let wx = |n: u8| tuned[n as usize].filter(|&(s, f)| (s as usize) < SLOTS && WX.contains(&f));
        let chip = if self.same_on { (0..2u8).find(|&n| wx(n).is_some()) } else { None };
        let at = chip.and_then(wx);
        if chip != self.same.chip || at != self.same.at {
            if let Some(old) = self.same.chip.filter(|&o| self.tuned[o as usize].is_some()) {
                self.chip(old).fsk_off();
            }
            (self.same.chip, self.same.at) = (chip, at);
            self.same.armed = false;
        }
        let (Some(n), Some((slot, freq))) = (chip, at) else { return };
        if !self.same.armed {
            let mut bk = self.chip(n);
            bk.fsk_setup(&SAME_FSK);
            bk.fsk_rx(&SAME_FSK);
            self.same.armed = true;
            return;
        }
        let flags = self.chip(n).interrupts();
        if flags & int::FSK_SYNC != 0 {
            self.same.asm.start();
        }
        if flags & (int::FSK_FIFO_FULL | int::FSK_RX_DONE) == 0 {
            return;
        }
        // Almost full is 4 words (REG_5E's default); done leaves at most the FIFO's 8.
        let words = if flags & int::FSK_RX_DONE != 0 { 8 } else { 4 };
        for _ in 0..words {
            let w = self.chip(n).fsk_read();
            match self.same.asm.push(w) {
                Step::More => continue,
                Step::Done => {
                    if let Some(header) = self.same.repeats.take(self.same.asm.text(), now / 1000) {
                        let _ = ALERTS.try_send(SameDecoded { header, slot, freq });
                    }
                    self.same.armed = false; // listen for the next burst
                }
                Step::Bad => self.same.armed = false,
            }
            break;
        }
        if flags & int::FSK_RX_DONE != 0 {
            self.same.armed = false;
        }
    }

    /// Battery Save's phase change: both chips sleep together (WATCH-8); waking, the next tick tunes them.
    async fn apply(&mut self, a: Action) {
        match a {
            Action::None => return,
            Action::Sleep => {
                speaker(false);
                for n in 0..2u8 {
                    self.tune_chip(n, None);
                }
                for s in &mut self.live.slots {
                    s.squelch_open = false;
                }
                let mut s = Steps::default();
                let _ = self.path.rx_off(&mut s); // refused only while keyed (TX-29)
                self.run(&s).await;
            }
            Action::Wake => {}
        }
        self.live.rx_asleep = self.save.asleep();
    }

    /// Something running that keeps the engines ticking fast: a slot sharing the chips, or a scan.
    fn busy(&self) -> bool {
        let set = self.watch_set | 1 << self.active;
        let xb = {
            #[cfg(feature = "tx")]
            {
                self.xb.is_some()
            }
            #[cfg(not(feature = "tx"))]
            {
                false
            }
        };
        set.count_ones() > 1 || self.scans.iter().any(|s| s.running()) || self.path.keyed().is_some() || self.counter.is_some() || xb
    }

    /// The engines' tick: each listening chip's squelch, the watch turn, the scans, the chips and the path, and
    /// the audio.
    /// SBAR-20: the MCU's die temperature every 10 s, for the Status page (informational; read here because the
    /// Radio task owns the ADC's software reads).
    fn mcu_temp(&mut self) {
        use crate::hal::adc;
        let now = now_ms() as u32;
        if self.temp.0.is_some_and(|t| now.wrapping_sub(t) < 10_000) {
            return;
        }
        self.temp.0 = Some(now);
        adc::init();
        if let Some(c10) = ferrite_core::mcutemp::temp_c10(adc::read_avg(adc::CH_TEMP, 4), adc::read_avg(adc::CH_VREF, 4)) {
            let s = ferrite_core::mcutemp::smooth(self.temp.1, c10);
            self.temp.1 = Some(s);
            crate::chan::MCU_TEMP.store(s, core::sync::atomic::Ordering::Relaxed);
        }
    }

    async fn tick(&mut self) {
        self.mcu_temp();
        if self.hf.is_some() {
            self.hf_poll();
            return;
        }
        // The crossband repeater owns both chips while it runs (XB-13: no watch, scan or Battery Save); local PTT
        // holds it (XB-14), and that transmission runs as any other.
        #[cfg(feature = "tx")]
        if let Some(held) = self.xb.as_ref().map(|x| x.rep.held()) {
            let now = now_ms() as u32;
            if !held {
                return self.xb_tick(now).await;
            }
            if self.path.keyed().is_some() {
                return self.tx_tick(now).await;
            }
            return;
        }
        if self.save.asleep() {
            return;
        }
        let now = now_ms() as u32;
        if self.counter.is_some() {
            return self.counter_tick(now).await;
        }
        // Keyed: the chips and paths are the transmission's (TX-29); only the sequencer's timeout, duty limit
        // and battery under load run.
        #[cfg(feature = "tx")]
        if self.path.keyed().is_some() {
            return self.tx_tick(now).await;
        }
        let listening = self.hop.chip();
        // 1. What each chip heard for the slot it was on: the carrier squelch (watch) and the tone (audio); for a
        // scan's fast step, the RSSI over the floor (SCAN-21).
        let mut sq = [false; 2];
        let mut toned = [false; 2];
        let mut scan_open = [false; SLOTS];
        for n in 0..2u8 {
            let Some((s, _)) = self.tuned[n as usize] else { continue };
            if listening != Some(n) || s >= SLOTS as u8 {
                continue;
            }
            let mut bk = self.chip(n);
            let (rssi, st) = (bk.rssi_dbm(), bk.status());
            let open = st.squelch_open || self.slots[s as usize].sq == 0;
            let fl = &mut self.floor[n as usize];
            if !st.squelch_open {
                *fl += (rssi - *fl) / 8; // a slow average of the quiet RSSI
            }
            sq[n as usize] = open;
            // A tone channel goes quiet when its tone stops (the tail's phase shift or turn-off code ends it, TONE-6);
            // with no tone and receive tail on, the 55 Hz tail mutes.
            let sl = self.slots[s as usize];
            toned[n as usize] = open && if sl.rx_tone == 0 { !(sl.rx_tail && st.tail) } else { st.ctcss || st.cdcss };
            let sl = &mut self.live.slots[s as usize];
            sl.rssi = rssi;
            scan_open[s as usize] = match self.scans[s as usize].check() {
                Check::Rssi => rssi > *fl + FAST_MARGIN_DB,
                Check::Squelch => open,
            };
        }
        // SAME on a weather channel's chip (DEC-19).
        self.same_tick(now);
        // 2. The watch turn.
        let cfg = watch::Cfg { set: self.watch_set, active: self.active, freq: self.slots.map(|s| s.freq), muted: 0 };
        let keyed = self.path.keyed().is_some();
        let out = self.watcher.tick(now, &cfg, sq, keyed);
        // 3. Scans step while their slot has a chip, on the answer from that chip.
        for i in 0..SLOTS {
            if !self.scans[i].running() || !self.tuned.iter().any(|t| t.is_some_and(|(s, _)| s as usize == i)) {
                continue;
            }
            let o = if self.chans.is_on(i) {
                self.chans.tick(i, &mut self.scans[i], now, scan_open[i])
            } else {
                self.scans[i].tick(&self.ranges[i], now, scan_open[i])
            };
            if o.ended {
                self.chans.off(i);
            }
            let sl = &mut self.live.slots[i];
            if let Some(f) = o.tune {
                self.slots[i].freq = f;
                sl.freq = f;
            }
            if o.hit.is_some() {
                sl.hits = sl.hits.saturating_add(1);
                sl.last_hit = sl.freq;
            }
            sl.skips = self.scans[i].skips.max(sl.skips);
            if o.ended {
                sl.scanning = false;
            }
        }
        // The channel the channel scans need next, for the UI to answer (LiveAll::want).
        self.live.set_want(self.chans.want(&self.scans));
        // 4. The chips: each on the slot the engine names (or asleep), the path on the one listening now.
        for n in 0..2u8 {
            let want = out.chip[n as usize].map(|s| (s, self.slots[s as usize].rx_freq()));
            self.tune_chip(n, want);
        }
        if let Some(n) = self.hop.next(now, &out, self.active) {
            if let Some((_, f)) = self.tuned[n as usize] {
                self.select_rx(f, Cause::Auto).await;
            }
        }
        // 5. Audio (WATCH-3): the slot the engine picks, if its tone matches; a latched monitor on the active
        // slot opens it whatever it hears (SQL-2).
        let heard_chip = |s: u8| (0..2u8).find(|&n| self.tuned[n as usize].is_some_and(|(t, _)| t == s));
        let monitor_active = self.monitor & 1 << self.active != 0;
        let audio = if monitor_active { Some(self.active) } else { out.audio };
        let audio_chip = audio.and_then(heard_chip).filter(|&n| monitor_active || toned[n as usize]);
        for n in 0..2u8 {
            if self.tuned[n as usize].is_some() {
                self.chip(n).af_out(if Some(n) == audio_chip { AfOut::Normal } else { AfOut::Mute });
            }
        }
        speaker(audio_chip.is_some() && !self.muted);
        // 6. The snapshot.
        let active_relay = Sel::rx(self.slots[self.active as usize].freq).relay();
        for (i, sl) in self.live.slots.iter_mut().enumerate() {
            let open = self.watcher.open(i as u8) || self.monitor & 1 << i != 0;
            if !open {
                sl.rssi = sl.rssi.min(-121);
            }
            sl.squelch_open = open;
            sl.relay = out.cancelled && self.watch_set & 1 << i != 0 && Sel::rx(self.slots[i].freq).relay() != active_relay;
        }
        // Battery Save: anything open or scanning keeps the receivers awake.
        let busy = self.live.slots.iter().any(|s| s.squelch_open || s.scanning);
        let a = self.save.tick(now_ms(), busy);
        self.apply(a).await;
    }

    /// HF mode on, retuned or off (HF-3, HF-4, HF-10, HF-11): the SI4732 below 18 MHz and for AM up to 30 MHz,
    /// chip 1 through the energised relay otherwise. Watch, scan and the other chip pause meanwhile (HF-8).
    async fn hf(&mut self, freq: Freq10, mode: u8, on: bool) {
        if !on {
            if self.hf.take().is_some_and(|h| h.si) {
                let _ = broadcast(&mut self.si, BroadcastCmd::Off).await;
            }
            self.tuned = [None; 2];
            self.live.mode = ModeLive::Off;
            let f = self.slots[self.active as usize].freq;
            let mut s = Steps::default();
            let _ = self.path.rx(Rx::Bk(Sel::rx(f)), Cause::HfReceiver, now_ms() as u32, &mut s);
            self.run(&s).await;
            log(b"hf off", 0);
            return;
        }
        let si = freq < mhz(18) || (mode == 2 && freq <= mhz(30));
        let was = self.hf.replace(HfOn { freq, mode, si });
        for n in 0..2u8 {
            if !(n == 1 && !si) {
                self.tune_chip(n, None);
            }
        }
        speaker(false);
        let mut s = Steps::default();
        let rx = if si { Rx::Si } else { Rx::Bk(Sel::rx(freq)) };
        let _ = self.path.rx(rx, Cause::HfReceiver, now_ms() as u32, &mut s);
        self.run(&s).await;
        if si {
            // AM only until the SSB patch is imported (HF-24; the UI asks for AM without it).
            let khz = (freq / 100) as u16;
            let ok = broadcast(&mut self.si, BroadcastCmd::Am(khz)).await.is_ok();
            speaker(ok && !self.muted);
        } else {
            if was.is_some_and(|h| h.si) {
                let _ = broadcast(&mut self.si, BroadcastCmd::Off).await;
            }
            self.tune_chip(1, Some((HF_SLOT, freq)));
            self.chip(1).af_out(AfOut::Normal);
            speaker(!self.muted);
        }
        log(b"hf khz ", freq / 100);
        self.hf_poll();
    }

    /// HF mode's report: frequency, signal and which receiver (ModeLive::Hf).
    fn hf_poll(&mut self) {
        let Some(h) = self.hf else { return };
        let (rssi, snr) = if h.si {
            match self.si.rsq(&mut BusyDelay) {
                Ok(q) => (q.rssi as i16 - 107, q.snr), // dBuV to dBm at 50 ohms
                Err(_) => (-127, 0),
            }
        } else {
            (self.chip(1).rssi_dbm(), 0)
        };
        self.live.mode = ModeLive::Hf { freq: h.freq, rssi, snr, si4732: h.si, loading: 255 };
    }

    /// HF-31: the SI4732's own seek from where it is (AM, below 30 MHz); the frequency it stops on is kept.
    async fn hf_seek(&mut self, up: bool) {
        let Some(h) = self.hf.filter(|h| h.si) else { return };
        let d = &mut BusyDelay;
        if self.si.seek_start(up, d).is_err() {
            return;
        }
        let t0 = now_ms();
        let found = loop {
            match self.si.tune_poll(d) {
                Ok(Some(t)) => break Some(t.freq),
                Ok(None) if now_ms() - t0 < 15_000 => Timer::after_millis(20).await,
                _ => break None,
            }
        };
        if let Some(khz) = found {
            self.hf = Some(HfOn { freq: khz as Freq10 * 100, ..h });
            log(b"hf seek khz ", khz as u32);
        }
        self.hf_poll();
    }

    /// A slot's setting from the UI; the chip it is on retunes on the next tick.
    fn param(&mut self, slot: u8, param: Param, value: i32) {
        let Some(s) = self.slots.get_mut(slot as usize) else { return };
        match param {
            Param::Mode => (s.narrow, s.am) = (value == 1, value == 2),
            Param::Bandwidth => s.narrow = value == 1,
            Param::Squelch => s.sq = value.clamp(0, 9) as u8,
            Param::RxTone => s.rx_tone = value as u16,
            Param::Clarifier => s.clar = value.clamp(-2_500, 2_500) as i16,
            #[cfg(feature = "tx")]
            Param::TxTone => return self.tx.tone[slot as usize % 4] = value as u16,
            #[cfg(feature = "tx")]
            Param::Power => return self.tx.power[slot as usize % 4] = value.clamp(0, 2) as u8,
            #[cfg(feature = "tx")]
            Param::EndTone => return self.tx.end[slot as usize % 4] = (value as u8).min(tonegen::END_TONES.len() as u8 - 1),
            Param::Tail => {
                s.rx_tail = value & 1 != 0;
                #[cfg(feature = "tx")]
                {
                    self.tx.tail[slot as usize % 4] = value & 2 != 0;
                }
            }
            #[cfg(not(feature = "tx"))]
            Param::TxTone | Param::Power | Param::EndTone => return,
            // The transmit frequency comes with each key-up (RadioCmd::TxPlan); busy lockout is the UI's (TX-21).
            Param::Offset | Param::BusyLock | Param::Reverse | Param::Split => return,
        }
        self.retune(slot);
    }

    /// Whichever chip has `slot` tunes it again on the next tick.
    fn retune(&mut self, slot: u8) {
        for t in &mut self.tuned {
            if t.is_some_and(|(s, _)| s == slot) {
                *t = Some((slot, 0)); // a frequency no slot has: the next tick's tune_chip sees a change
            }
        }
    }

    /// A scan on `slot`: its band from where it is, the fast pass (SCAN-21).
    fn scan(&mut self, slot: u8, on: bool) {
        let i = slot as usize % SLOTS;
        let sl = &mut self.live.slots[i];
        sl.scanning = on;
        if on {
            (sl.hits, sl.skips) = (0, 0);
            let r = scan::band_range(self.slots[i].freq);
            self.ranges[i] = r;
            let sc = &mut self.scans[i];
            sc.fast = true; // the resume rule is the last ScanResume's (SCAN-5, SCAN-24)
            sc.start(scan::range_pos(&r, self.slots[i].freq), now_ms() as u32);
            log(b"scan start slot ", i as u32);
        } else {
            self.scans[i].stop();
        }
        self.chans.off(i); // a band scan, or none
    }

    /// A scan's answer to ▲▼ or \*: the step it tuned, the end.
    fn scan_out(&mut self, i: usize, o: scan::Out) {
        let sl = &mut self.live.slots[i];
        if let Some(f) = o.tune {
            self.slots[i].freq = f;
            sl.freq = f;
        }
        sl.skips = self.scans[i].skips.max(sl.skips);
        if o.ended {
            sl.scanning = false;
            self.chans.off(i);
        }
    }

    /// SND-11: a local beep from a BK4829's tone generator, as stock does it (0x0801EB8C, 0x0801EB4C): the chip
    /// leaves receive for the beep (REG_30 0x0302: no PLL, no PA) and comes back. It takes the chip that disturbs
    /// least: one nobody is hearing, a sleeping one before a listening one; a heard chip only if both are heard.
    /// Never while keyed. The task waits through it (at most `tonegen::BEEP_MAX_MS`).
    async fn beep(&mut self, hz: u16, ms: u16) {
        if self.path.keyed().is_some() || hz == 0 {
            return;
        }
        let heard = |r: &Radio, n: u8| match r.tuned[n as usize] {
            Some((s, _)) if (s as usize) < SLOTS => r.live.slots[s as usize].squelch_open,
            Some(_) => true, // HF mode's or the counter's chip: always in use
            None => false,
        };
        let Some(n) = (0..2u8).filter(|&n| self.has(n)).min_by_key(|&n| (heard(self, n), self.tuned[n as usize].is_some()))
        else {
            return;
        };
        let asleep = self.tuned[n as usize].is_none();
        let mut bk = self.chip(n);
        if asleep {
            bk.wake();
        }
        let gain = bk.beep_on(hz as u32);
        speaker(true);
        Timer::after_millis(ms.min(tonegen::BEEP_MAX_MS) as u64).await;
        let mut bk = self.chip(n);
        bk.beep_off(gain);
        if asleep {
            bk.sleep();
        } else {
            bk.rx_on(); // the audio decision unmutes it on the next tick, if it is heard
        }
        speaker(false);
        log(b"beep hz ", hz as u32);
    }

    /// The frequency counter on or off (TSRCH-10): an exclusive mode like HF mode; watch and scans pause.
    fn counter(&mut self, on: bool) {
        for n in 0..2u8 {
            if self.has(n) {
                self.chip(n).freq_scan(false);
            }
        }
        self.tuned = [None; 2]; // the counter, or the engines afterwards, tune them again
        let first = (0..2u8).find(|&n| self.has(n)).unwrap_or(0);
        self.counter = on.then_some(Counter { chip: first, since: 0, last: None });
        self.live.mode = if on { ModeLive::Probe { freq: 0, rssi: -127, tone: 0 } } else { ModeLive::Off };
        speaker(false);
        log(b"counter ", on as u32);
    }

    /// The counter's tick: one chip measures with the BK4829's frequency scan (stock's counter, 0x08021BC4), the
    /// other sleeps; a reading counts when the next one agrees within 500 Hz (`tonegen::counter_agree`). After
    /// `COUNTER_TURN_MS` with nothing, the other chip takes a turn. Each chip's path is asked for with
    /// `Cause::Auto`: the relay never moves (SAFE-20), so a chip whose band would need it just isn't used.
    async fn counter_tick(&mut self, now: u32) {
        let Some(mut c) = self.counter else { return };
        let n = c.chip;
        let f = COUNTER_TUNE[n as usize];
        if self.tuned[n as usize].is_none() {
            self.tune_chip(1 - n, None);
            self.tune_chip(n, Some((COUNTER_SLOT, f)));
            self.select_rx(f, Cause::Auto).await;
            if self.path.rx_now() == Rx::Bk(Sel::rx(f)) {
                self.chip(n).freq_scan(true);
            }
            (c.since, c.last) = (now, None);
        } else {
            let mut bk = self.chip(n);
            if let Some(hz) = bk.freq_scan_result() {
                bk.freq_scan(false);
                let got = hz / 10;
                let rssi = bk.rssi_dbm();
                if tonegen::counter_chip(got) == Some(n) {
                    if let Some(freq) = c.last.and_then(|l| tonegen::counter_agree(l, got)) {
                        self.live.mode = ModeLive::Probe { freq, rssi, tone: 0 };
                        log(b"counter khz ", freq / 100);
                    }
                    c.last = Some(got);
                }
                self.chip(n).freq_scan(true);
                c.since = now;
            } else if now.wrapping_sub(c.since) > COUNTER_TURN_MS {
                bk.freq_scan(false);
                self.live.mode = ModeLive::Probe { freq: 0, rssi: -127, tone: 0 };
                let other = 1 - n;
                if self.has(other) {
                    self.tune_chip(n, None);
                    c.chip = other;
                }
                self.tuned[c.chip as usize] = None; // starts its scan on the next tick
            }
        }
        self.counter = Some(c);
    }
}

/// The counter's own index in [`Radio::tuned`] (after the three slots and HF mode's).
const COUNTER_SLOT: u8 = 4;

/// The SI4732's input path for its mode, as stock (BCR-20, `analysis/audio_path.md` §2): FM broadcast PA7 = PC5 = 0,
/// AM (and SW, SSB, CW) PA7 = PC5 = 1, the LNA path.
fn si_input(mode: SiMode) {
    let am = mode != SiMode::Fm;
    SI_IN[0].write(am);
    SI_IN[1].write(am);
}

/// The broadcast receiver: powered up in the mode asked for (reset pulse, power-up, crystal settling), then
/// tuned by polling. FM in 10 kHz units, AM in kHz. PA7/PC5 take stock's state for the mode (BCR-20). SSB (patch from SPI flash) comes with the link's patch import.
async fn broadcast(si: &mut Si4732<SiBus>, cmd: BroadcastCmd) -> Result<(), si4732::Error> {
    let d = &mut BusyDelay;
    let (mode, freq) = match cmd {
        BroadcastCmd::Off => {
            if si.mode().is_some() {
                si.power_down(d)?;
            }
            return Ok(());
        }
        BroadcastCmd::Fm(f) => (SiMode::Fm, Some(f)),
        BroadcastCmd::Am(f) => (SiMode::Am, Some(f)),
        BroadcastCmd::Seek { up } => {
            si.seek_start(up, d)?;
            (si.mode().unwrap_or(SiMode::Fm), None)
        }
    };
    if si.mode() != Some(mode) {
        if si.mode().is_some() {
            si.power_down(d)?;
        }
        si_input(mode);
        SI_RST.write(false);
        Timer::after_millis(10).await;
        SI_RST.write(true);
        Timer::after_millis(10).await;
        si.power_up_start(mode)?;
        while !si.ready()? {
            Timer::after_millis(5).await;
        }
        Timer::after_millis(500).await; // crystal
        si.volume(63, d)?; // the volume knob is analog, after the chip
    }
    if let Some(f) = freq {
        si.tune_start(f, Sideband::Usb, d)?;
    }
    while si.tune_poll(d)?.is_none() {
        Timer::after_millis(10).await;
    }
    Ok(())
}

#[embassy_executor::task]
pub async fn task() {
    for p in AMP_EN {
        p.output(false, Mode::OutPp);
    }
    let bus = I2c { scl: SI_SCL, sda: SI_SDA, half_us: 5 }; // about 50 kHz
    bus.init();
    let mut r = Radio {
        si: Si4732::new(SiBus(bus), SI4732_ADDR),
        wire: ThreeWire::new(Out(BK_SCK), Io(BK_SDA), BusyDelay),
        cs: [Out(BK_CS[0]), Out(BK_CS[1])],
        chips: 0,
        live: LiveAll::default(),
        save: RxSleep::new(DEFAULT_SAVE, now_ms()),
        temp: (None, None),
        path: Path::new(),
        muted: false,
        si_used: false,
        slots: [SlotRx::DEFAULT; SLOTS],
        active: 0,
        watch_set: 0,
        watcher: Watch::new(),
        hop: Hop::new(),
        scans: [Scan::new(); SLOTS],
        ranges: [scan::band_range(DEFAULT); SLOTS],
        chans: scan::ChanScans::new(),
        tuned: [None; 2],
        floor: [-125; 2],
        monitor: 0,
        scan_tx: false,
        tone_level: 1,
        hf: None,
        counter: None,
        same_on: false,
        same: SameRx { chip: None, at: None, armed: false, asm: Assembler::new(), repeats: Repeats::new() },
        #[cfg(feature = "tx")]
        tx: Tx {
            seq: TxSeq::new(),
            cal: None,
            gauge: Gauge::default(),
            tone: [0; 4],
            power: [0; 4],
            plan: None,
            data: None,
            ptt_ok: false,
            pa: 0,
            fault: false,
            adc: false,
            end: [0; 4],
            tail: [false; 4],
            slot: 0,
            phase: 0,
            cw: 0,
            cw_call: [0; 10],
            cw_len: 0,
            sidetone: true,
            dtmf: (100, 30),
            digits: [0; 16],
            dialled: false,
            chip: None,
            region: None,
        },
        #[cfg(feature = "tx")]
        xb: None,
    };
    // Start from the safe state: every path low, the relay released (board start-up set them low already).
    let mut s = Steps::default();
    r.path.safe(now_ms() as u32, &mut s);
    r.run(&s).await;
    // Each chip: find the read alignment from its ID, then stock's start-up.
    for n in 0..2u8 {
        let found = r.wire.detect(&mut r.cs[n as usize]).is_some();
        if found {
            r.chips |= 1 << n;
            r.chip(n).init();
            r.chip(n).sleep(); // until the engines give it a slot
        }
        check(Source::Radio, if n == 0 { CHECK_BK0 } else { CHECK_BK1 }, found);
    }
    r.live.chips = r.chips;
    for (i, sl) in r.live.slots.iter_mut().enumerate() {
        (sl.freq, sl.rssi) = (r.slots[i].freq, -121);
    }
    // The SI4732 is held in reset (PA6) until FM broadcast or HF mode powers it: not tested at boot.
    crate::tasks::untested(crate::tasks::CHECK_SI);
    r.select_rx(DEFAULT, Cause::ActiveSlot).await;
    r.tick().await;
    let live = LIVE.sender();
    live.send(r.live);

    loop {
        // Wake for the next command, the next Battery Save phase change, or (awake) the next tick.
        let mut at = r.save.deadline();
        if !r.save.asleep() || r.hf.is_some() {
            at = at.min(now_ms() + if r.busy() { TICK_MS } else { POLL_MS });
        }
        match select3(RADIO.receive(), Timer::at(Instant::from_millis(at)), DEBUG_REQ.receive()).await {
            Either3::First(c) => r.cmd(c).await,
            Either3::Second(()) => r.tick().await,
            Either3::Third(d) => {
                let a = r.debug(d);
                DEBUG_ANS.send(a).await;
                TERM_WAKE.signal(()); // the UI task runs the terminal
                continue;
            }
        }
        let snap = r.live;
        // The GPS task's view (GPS-4, GPS-22): keyed, and any slot receiving.
        let rx = snap.slots.iter().any(|s| s.squelch_open);
        let rf = if snap.tx.keyed { crate::chan::RF_BUSY_KEYED } else { 0 } | if rx { crate::chan::RF_BUSY_RX } else { 0 };
        crate::chan::RF_STATE.store(rf, core::sync::atomic::Ordering::Relaxed);
        live.send_if_modified(|v| {
            let changed = *v != Some(snap);
            if changed {
                *v = Some(snap);
            }
            changed
        });
    }
}

impl Radio {
    /// One command from the UI.
    async fn cmd(&mut self, c: RadioCmd) {
        let now = now_ms() as u32;
        // The single-receiver command is the active slot's tune.
        let c = if let RadioCmd::Tune(freq) = c { RadioCmd::TuneSlot { slot: self.active, freq } } else { c };
        // Channel scans: the set, priorities, resume rule, start, and the frequencies the UI answers with; one
        // a stopped scan waited for is tuned now.
        if let Some((i, f)) = self.chans.cmd(&c, &mut self.scans, now) {
            self.slots[i].freq = f;
            self.live.slots[i].freq = f;
        }
        match c {
            RadioCmd::ScanChans { slot, .. } => {
                let sl = &mut self.live.slots[slot as usize % SLOTS];
                (sl.scanning, sl.hits, sl.skips) = (true, 0, 0);
                log(b"scan start (channels) slot ", slot as u32);
            }
            RadioCmd::ScanSet { .. }
            | RadioCmd::ScanPrio { .. }
            | RadioCmd::ScanResume { .. }
            | RadioCmd::ScanEntry { .. }
            | RadioCmd::ScanEntryTx { .. } => {}
            RadioCmd::TuneSlot { slot, freq } => {
                let i = slot as usize % SLOTS;
                (self.slots[i].freq, self.slots[i].clar) = (freq, 0); // VFO-22: a retune clears the clarifier
                self.scans[i].stop();
                self.chans.off(i);
                let sl = &mut self.live.slots[i];
                (sl.freq, sl.scanning) = (freq, false);
                if slot == self.active && self.hf.is_none() {
                    self.select_rx(freq, Cause::ActiveSlot).await; // the active slot may move the relay
                }
                self.retune(slot);
                self.save.activity(now_ms());
            }
            RadioCmd::SlotParam { slot, param, value } => self.param(slot, param, value),
            RadioCmd::Active(a) => {
                self.active = a % SLOTS as u8;
                if self.hf.is_none() {
                    let f = self.slots[self.active as usize].freq;
                    self.select_rx(f, Cause::ActiveSlot).await;
                }
            }
            RadioCmd::Watch(m) => self.watch_set = m,
            RadioCmd::PauseWatch(secs) => self.watcher.pause(now, secs),
            RadioCmd::Scan { slot, on } => self.scan(slot, on),
            RadioCmd::ScanSkip { slot } => {
                let i = slot as usize % SLOTS;
                let o = if self.chans.is_on(i) {
                    self.chans.skip_once(i, &mut self.scans[i], now)
                } else {
                    self.scans[i].skip_once(&self.ranges[i], now)
                };
                self.scan_out(i, o);
            }
            RadioCmd::ScanDir { slot, up, .. } => {
                let i = slot as usize % SLOTS;
                let o = if self.chans.is_on(i) {
                    self.chans.dir(i, &mut self.scans[i], up, now)
                } else {
                    self.scans[i].dir(up, &self.ranges[i], now)
                };
                self.scan_out(i, o);
            }
            RadioCmd::Monitor { slot, on } => self.monitor = self.monitor & !(1 << slot) | (on as u8) << slot,
            RadioCmd::ScanTx(on) => self.scan_tx = on,
            RadioCmd::ToneLevel(l) => self.tone_level = l.min(2),
            RadioCmd::BatterySave(level) => {
                let a = self.save.set_level(level, now_ms());
                self.apply(a).await;
            }
            RadioCmd::Activity => {
                let a = self.save.activity(now_ms());
                self.apply(a).await;
            }
            RadioCmd::Squelch(l) => self.param(self.active, Param::Squelch, l as i32),
            RadioCmd::Broadcast(b) => {
                let _ = broadcast(&mut self.si, b).await; // errors: the UI shows no station (reporting: next)
            }
            RadioCmd::Hf { freq, mode, on } => self.hf(freq, mode, on).await,
            RadioCmd::HfSeek { up } => self.hf_seek(up).await,
            RadioCmd::Shutdown => {
                for n in 0..2u8 {
                    self.tune_chip(n, None);
                }
                let _ = broadcast(&mut self.si, BroadcastCmd::Off).await;
                let mut s = Steps::default();
                self.path.safe(now_ms() as u32, &mut s);
                self.run(&s).await;
                speaker(false);
            }
            RadioCmd::Counter(on) => self.counter(on),
            RadioCmd::Same(on) => self.same_on = on,
            RadioCmd::Beep { hz, ms } => self.beep(hz, ms).await,
            #[cfg(feature = "tx")]
            RadioCmd::ToneBurst { on } => self.keyed_tone(if on { Some(1750) } else { None }, None),
            #[cfg(feature = "tx")]
            RadioCmd::Dtmf { key, on } => self.keyed_tone(None, if on { tonegen::dtmf_symbol(key) } else { None }),
            // Without the `tx` build nothing is keyed, so there is nothing to send them on.
            #[cfg(not(feature = "tx"))]
            RadioCmd::ToneBurst { .. } | RadioCmd::Dtmf { .. } => {}
            #[cfg(feature = "tx")]
            RadioCmd::TxTones { phase, cw, sidetone } => (self.tx.phase, self.tx.cw, self.tx.sidetone) = (phase.min(3), cw.min(2), sidetone),
            #[cfg(feature = "tx")]
            RadioCmd::CwCall { chunk, text } => {
                let at = (chunk as usize).min(1) * 5;
                self.tx.cw_call[at..at + 5].copy_from_slice(&text);
                self.tx.cw_len = self.tx.cw_call.iter().position(|&c| c == 0).unwrap_or(10) as u8;
            }
            #[cfg(feature = "tx")]
            RadioCmd::DtmfTiming { ms, delay_10ms } => self.tx.dtmf = (ms.clamp(50, 200), delay_10ms.min(100)),
            #[cfg(feature = "tx")]
            RadioCmd::DtmfSeq { chunk, keys } => {
                let at = (chunk as usize).min(3) * 5;
                let n = (16 - at).min(5);
                self.tx.digits[at..at + n].copy_from_slice(&keys[..n]);
            }
            #[cfg(feature = "tx")]
            RadioCmd::DtmfSend { len } => self.dial(len).await,
            // Nothing transmits in this build.
            #[cfg(not(feature = "tx"))]
            RadioCmd::TxTones { .. }
            | RadioCmd::CwCall { .. }
            | RadioCmd::DtmfTiming { .. }
            | RadioCmd::DtmfSeq { .. }
            | RadioCmd::DtmfSend { .. } => {}
            // Not on the radio yet, each waiting on something named in the manual's Status lines: band scope,
            // tone search, crossband (chapters 21, 22, 19): their own exclusive modes, after 2d.
            RadioCmd::Sweep { .. }
            | RadioCmd::ScopeCfg { .. }
            | RadioCmd::ScopeListen { .. }
            | RadioCmd::ToneSearch { .. } => {}
            #[cfg(feature = "tx")]
            RadioCmd::Crossband(c) => self.crossband(c).await,
            // XB-2: no transmit in this build; the UI refuses first, and the radio says so too.
            #[cfg(not(feature = "tx"))]
            RadioCmd::Crossband(c) => {
                let stopped = ferrite_core::xb::Stop::NoTx as u8;
                self.live.mode = if c.is_some() { ModeLive::Xb(ferrite_core::msg::XbLive { stopped, ..Default::default() }) } else { ModeLive::Off };
            }
            #[cfg(feature = "tx")]
            RadioCmd::TxPlan { tx, region, extend } => {
                let r = Region::ALL.get(region as usize).copied().unwrap_or(Region::Other);
                self.tx.plan = Some((tx, r, extend));
                self.tx.region = Some((r, extend));
            }
            #[cfg(feature = "tx")]
            RadioCmd::Transmit { slot, on } => {
                // XB-14: local PTT interrupts a pass (unkeyed first) and holds the repeater until it's released.
                if on {
                    self.xb_hold(true).await;
                }
                self.transmit(slot, on, None).await;
                if !on {
                    self.xb_hold(false).await;
                }
            }
            #[cfg(feature = "tx")]
            RadioCmd::TransmitData { slot, power, on } => self.transmit(slot, on, Some(power)).await,
            // Transmit code is out of this build: the UI refuses first (RX-only build).
            #[cfg(not(feature = "tx"))]
            RadioCmd::Transmit { .. } | RadioCmd::TransmitData { .. } | RadioCmd::TxPlan { .. } => {}
            RadioCmd::Tune(_) => {}
        }
    }
}

/// The `tx` build's transmit side (manual chapter 15).
#[cfg(feature = "tx")]
impl Radio {
    /// TX-42: every PTT input (PTT A PE3, PTT B PE2, the connector's PE6; low when pressed) has read released
    /// since boot.
    fn ptt_seen_released(&mut self) -> bool {
        use crate::board::{KEY_PTT, KEY_SIDE1, KEY_SIDE2};
        if !self.tx.ptt_ok && KEY_PTT.read() && KEY_SIDE1.read() && KEY_SIDE2.read() {
            self.tx.ptt_ok = true;
            log(b"ptt released since boot", 0);
        }
        self.tx.ptt_ok
    }

    /// The battery for transmit (PWR-20): the PA1 reading against the calibration's gauge.
    fn battery_ok(&mut self) -> bool {
        use crate::hal::adc;
        if !self.tx.adc {
            adc::init();
            self.tx.adc = true;
        }
        self.tx.gauge.tx_ok(adc::read(adc::CH_PA1))
    }

    /// `Act::ChipTx`: the chip already holds the TX frequency and tone; key it with the microphone, read REG_30
    /// back (TX-7), and stock's REG_36 for the level ({0x7F, 0x57, 0x57}[level], `analysis/calibration.md`).
    fn key_chip(&mut self, n: u8) -> bool {
        if !self.has(n) {
            return false;
        }
        let high = self.tx.data.unwrap_or(self.tx.power[self.live.tx.slot as usize % 4]) == 2;
        let mut bk = self.chip(n);
        bk.tx_on(bk4829::tx::Source::Mic);
        if bk.read(0x30) != REG30_TX_MIC {
            return false;
        }
        bk.write(0x36, if high { 0x7F } else { 0x57 });
        true
    }

    /// Key up or unkey a slot (3 = HF mode's frequency); `data`: a data key-up at that power level (APRS).
    async fn transmit(&mut self, slot: u8, on: bool, data: Option<u8>) {
        let now = now_ms() as u32;
        let mut st = Steps::default();
        if !on {
            let cfg = self.tx_cfg();
            self.tx.seq.release(&cfg, &mut self.path, now, &mut st);
            self.run(&st).await;
            self.unkeyed();
            return;
        }
        if let Some(c) = crate::chan::TX_CAL.try_take() {
            (self.tx.cal, self.tx.gauge) = c;
        }
        let Some((f, region, extend)) = self.tx.plan.take() else {
            return self.refuse(slot, Refusal::Locked);
        };
        if !self.ptt_seen_released() {
            return self.refuse(slot, Refusal::ReleasePtt);
        }
        let s = slot as usize % 4;
        let rx = if slot < SLOTS as u8 { self.slots[s].freq } else { f };
        self.tx.data = data;
        let req = Req {
            tx: f,
            rx,
            kind: if data.is_some() { TxKind::Data } else { TxKind::Voice },
            person: data.is_none(),
            confirmed: true, // the UI asked for the emergency prompt's second PTT before this (TX-3)
            am: slot < SLOTS as u8 && self.slots[s].am,
            region,
            extend,
            cal_ok: self.tx.cal.is_some(),
            batt_ok: self.battery_ok(),
            power: data.unwrap_or(self.tx.power[s]).min(2),
        };
        if let Err(r) = self.tx.seq.check(&req, &self.path, now) {
            return self.refuse(slot, r);
        }
        // The chip for the TX frequency: tuned there with the slot's tone, before the plan keys it.
        let n = Sel::tx(f).chip();
        if !self.has(n) {
            return self.refuse(slot, Refusal::Locked);
        }
        speaker(false);
        self.tuned[n as usize] = Some((slot, f)); // not the receive frequency: retuned after the unkey
        let tone = self.tx.tone[s];
        let mut bk = self.chip(n);
        bk.wake();
        bk.af_out(AfOut::Mute);
        bk.set_freq(f * 10);
        match tone {
            0 => bk.subtone_off(),
            t if t & 0x8000 != 0 => bk.dcs_tx(t & 0x3FFF, t & 0x4000 != 0),
            t => bk.ctcss_tx(t),
        }
        self.tx.pa = self.tx.cal.map_or(0, |c| c.dac(f, req.power));
        self.tx.fault = false;
        self.live.tx = TxLive { keyed: false, slot, mic: 0, refused: None, dtmf_sent: self.live.tx.dtmf_sent };
        (self.tx.slot, self.tx.dialled) = (slot, false);
        let r = self.tx.seq.key(&req, &mut self.path, now, &mut st);
        if let Err(r) = r {
            self.unkeyed();
            return self.refuse(slot, r);
        }
        log(b"tx key khz ", f / 100);
        self.run(&st).await;
        if self.tx.fault {
            let mut st = Steps::default();
            self.tx.seq.safe(&mut self.path, now_ms() as u32, &mut st);
            self.run(&st).await;
            self.unkeyed();
            return self.refuse(slot, Refusal::Locked);
        }
        self.live.tx.keyed = true;
        self.tx.chip = Some(n);
    }

    /// While keyed: the sequencer's own unkeys (timeout backstop, duty limit, battery under load).
    async fn tx_tick(&mut self, now: u32) {
        let batt = self.battery_ok();
        // TX-12's mic meter: PA0, the mic-level tap stock's VOX reads (decision 76).
        self.live.tx.mic = tonegen::mic_percent(crate::hal::adc::read(crate::hal::adc::CH_PA0));
        let mut st = Steps::default();
        let cfg = self.tx_cfg();
        if let Some(e) = self.tx.seq.tick(&cfg, batt, &mut self.path, now, &mut st) {
            self.run(&st).await;
            if e != txseq::Event::PreAlert {
                log(b"tx unkeyed by the sequencer ", e as u32);
                self.unkeyed();
                self.live.tx.refused = Some(match e {
                    txseq::Event::Hot => Refusal::Hot,
                    txseq::Event::Battery => Refusal::Battery,
                    _ => Refusal::ReleasePtt,
                });
            }
        }
    }

    /// The sequencer's settings from what the UI has sent: the keyed slot's end tone and tail, CW ID with a
    /// callsign (VOX-10); none of the end tone or ID after a dialled sequence (TONE-11).
    fn tx_cfg(&self) -> txseq::Cfg {
        let s = self.tx.slot as usize % 4;
        let voice = !self.tx.dialled;
        txseq::Cfg {
            end_tone: voice && self.tx.end[s] != 0,
            cw_id: voice && self.tx.cw != 0 && self.tx.cw_len != 0,
            cw_each: self.tx.cw == 2,
            tail: self.tx.tail[s],
            ..TX_CFG
        }
    }

    /// TONE-5: the tail at unkey, still keyed, for stock's 400 ms: the 55 Hz tone with no sub-tone, the phase
    /// shift (0 8 5 2) on CTCSS, the 134.4 Hz turn-off code on DCS. The 55 Hz tone at the driver's sub-tone gain
    /// (74, as its CTCSS) where stock takes the calibration's byte (0xF210 byte 1): to confirm on the bench.
    async fn tail_elim(&mut self) {
        use bk4829::tx::Tail;
        let Some(n) = self.tx.chip.filter(|&n| self.has(n)) else { return };
        let kind = match tonegen::tail_kind(self.tx.tone[self.tx.slot as usize % 4], self.tx.phase) {
            tonegen::TailKind::Tone55 => Tail::Tone55 { dev: 74 },
            tonegen::TailKind::Shift(p) => Tail::Shift { phase: p },
        };
        self.chip(n).tail_tx(Some(kind));
        Timer::after_millis(tonegen::TAIL_MS as u64).await;
        self.chip(n).tail_tx(None);
        log(b"tx tail", 0);
    }

    /// DTMF-1, DTMF-3: the dialled sequence on the transmission just keyed: the first digit delay, then each
    /// symbol on and off for the speed; then the UI is told and unkeys. The Radio task waits through it (at most
    /// 1 s + 16 x 400 ms).
    async fn dial(&mut self, len: u8) {
        if self.path.keyed().is_none() {
            return;
        }
        self.tx.dialled = true;
        let (ms, delay) = (self.tx.dtmf.0 as u64, self.tx.dtmf.1 as u64 * 10);
        Timer::after_millis(delay).await;
        for k in 0..(len as usize).min(16) {
            if self.path.keyed().is_none() {
                return; // cut off (timeout, battery)
            }
            if let Some(c) = tonegen::dtmf_symbol(self.tx.digits[k]) {
                self.keyed_tone(None, Some(c));
                Timer::after_millis(ms).await;
            }
            if let Some(n) = self.path.keyed().map(|s| s.chip()).filter(|&n| self.has(n)) {
                self.chip(n).tone_tx(0);
            }
            Timer::after_millis(ms).await;
        }
        self.keyed_tone(None, None);
        self.live.tx.dtmf_sent = self.live.tx.dtmf_sent.wrapping_add(1);
        log(b"tx dtmf sent ", len as u32);
    }

    /// While keyed (TONE-13, DTMF-7, DTMF-11): the tone burst or a DTMF symbol on the keyed chip, as stock sends any
    /// internal tone: the generator set, REG_30 to tone transmit (mode 4, 0xC3FA: the microphone off) and the
    /// sidetone heard (DTMF-8, for DTMF as 0 8 6 6 says); both None goes back to
    /// the microphone (0xC1FE).
    fn keyed_tone(&mut self, hz: Option<u16>, dtmf: Option<char>) {
        use bk4829::tx::Source as TxSource;
        let Some(sel) = self.path.keyed() else { return };
        let n = sel.chip();
        if !self.has(n) {
            return;
        }
        let heard = dtmf.is_none() || self.tx.sidetone; // DTMF-8's setting; the burst is always heard
        let mut bk = self.chip(n);
        let sending = match (hz, dtmf) {
            (Some(hz), _) => {
                bk.tone_tx(hz as u32);
                true
            }
            (None, Some(d)) => bk.dtmf_tx(d),
            (None, None) => false,
        };
        if sending {
            bk.tx_on(TxSource::Tones);
            bk.sidetone(heard);
        } else {
            bk.tone_tx(0);
            bk.sidetone(false);
            bk.tx_on(TxSource::Mic);
            if bk.read(0x30) != REG30_TX_MIC {
                log(b"tx mic read-back failed, chip ", n as u32);
            }
        }
        speaker(sending && heard);
    }

    /// A tone sequence on the keyed chip, still keyed (TONE-8: end tone, CW ID): tone transmit with the microphone
    /// off, each step's tone (0 = silence) for its time.
    async fn play_keyed(&mut self, steps: impl Iterator<Item = tonegen::Step>) {
        use bk4829::tx::Source as TxSource;
        let Some(n) = self.tx.chip.filter(|&n| self.has(n)) else { return };
        self.chip(n).tx_on(TxSource::Tones);
        for (hz, ms) in steps {
            self.chip(n).tone_tx(hz as u32);
            Timer::after_millis(ms as u64).await;
        }
        self.chip(n).tone_tx(0);
    }

    /// After any unkey: the chips back to their slots on the next tick, the UI told.
    fn unkeyed(&mut self) {
        self.tuned = [None; 2];
        self.tx.chip = None;
        self.live.tx.keyed = false;
        log(b"tx unkey", 0);
    }

    fn refuse(&mut self, slot: u8, why: Refusal) {
        self.live.tx = TxLive { keyed: false, slot, mic: 0, refused: Some(why), dtmf_sent: self.live.tx.dtmf_sent };
        log(b"tx refused ", why as u32);
    }
}

/// The crossband repeater (manual chapter 19, decision 93), in the `tx` build only.
#[cfg(feature = "tx")]
impl Radio {
    /// Start or stop. XB-2 on the radio: two sides on different chips, neither on HF, the relay released as it is
    /// now (the repeater never moves it); a refusal goes back in `XbLive::stopped` for the UI to show.
    async fn crossband(&mut self, c: Option<ferrite_core::msg::XbCfg>) {
        let now = now_ms() as u32;
        let Some(cfg) = c else {
            if let Some(x) = self.xb.take() {
                if self.path.keyed().is_some() {
                    self.xb_unkey(&x, false, now).await;
                }
            }
            self.tuned = [None; 2];
            self.live.mode = ModeLive::Off;
            speaker(false);
            log(b"xb off", 0);
            return;
        };
        let sides = [cfg.side1 % SLOTS as u8, cfg.side2 % SLOTS as u8];
        let f = sides.map(|s| self.slots[s as usize].freq);
        if let Err(why) = xb::sides_ok(f, self.path.relay()) {
            self.live.mode = ModeLive::Xb(ferrite_core::msg::XbLive { stopped: why as u8, ..Default::default() });
            log(b"xb refused ", why as u32);
            return;
        }
        // XB-13: Battery Save's receivers awake; watch and scans simply don't run while it does.
        let a = self.save.activity(now_ms());
        self.apply(a).await;
        self.tuned = [None; 2];
        speaker(false);
        let rep = Repeater::new(cfg, now);
        self.live.mode = ModeLive::Xb(rep.live);
        self.xb = Some(XbRun { rep, sides, hop: 0, hop_t: now });
        log(b"xb on, side 1 khz ", f[0] / 100);
    }

    /// XB-14: local PTT holds the repeater, unkeying a pass first; released, it carries on.
    async fn xb_hold(&mut self, on: bool) {
        let now = now_ms() as u32;
        let Some(mut x) = self.xb.take() else { return };
        if on && x.rep.from().is_some() && self.path.keyed().is_some() {
            self.xb_unkey(&x, false, now).await;
        }
        x.rep.hold(on, now);
        if !on {
            self.tuned = [None; 2]; // back on the sides on the next tick
        }
        self.xb = Some(x);
    }

    /// The repeater's tick (10 ms): while ready the receive path takes turns on the two sides; a pass keeps its
    /// side's. Each listened side's squelch, with its tone when the tone gate is on (XB-4), goes to the engine; its
    /// key-ups and unkeys are carried out; the sequencer's duty limit, battery and backstop run while keyed.
    async fn xb_tick(&mut self, now: u32) {
        let Some(mut x) = self.xb.take() else { return };
        let keyed = self.path.keyed().is_some();
        if keyed {
            let batt = self.battery_ok();
            let cfg = self.xb_seq_cfg(&x, false);
            let mut st = Steps::default();
            if let Some(e) = self.tx.seq.tick(&cfg, batt, &mut self.path, now, &mut st) {
                self.run(&st).await;
                if e != txseq::Event::PreAlert {
                    x.rep.cut(now);
                    self.xb_after_unkey(&x);
                    log(b"xb cut by the sequencer ", e as u32);
                }
            }
        }
        let keyed_chip = self.path.keyed().map(|s| s.chip());
        // Each side's chip on its slot (the transmitting one stays as keyed: simplex, the same frequency).
        for s in x.sides {
            let n = Sel::rx(self.slots[s as usize].freq).chip();
            if keyed_chip != Some(n) {
                self.tune_chip(n, Some((s, self.slots[s as usize].rx_freq())));
            }
        }
        let listen = match x.rep.from() {
            Some(from) => from,
            None => {
                if now.wrapping_sub(x.hop_t) >= XB_TURN_MS {
                    (x.hop, x.hop_t) = (x.hop ^ 1, now);
                }
                x.hop
            }
        };
        let ls = x.sides[listen as usize];
        let lf = self.slots[ls as usize].freq;
        if keyed_chip.is_none() {
            self.select_rx(lf, Cause::Auto).await; // never the relay: refused instead (checked at start)
        }
        let mut heard = [None; 2];
        let n = Sel::rx(lf).chip();
        if self.path.rx_now() == Rx::Bk(Sel::rx(lf)) && self.has(n) && keyed_chip != Some(n) {
            let mut bk = self.chip(n);
            let (rssi, st) = (bk.rssi_dbm(), bk.status());
            let sl = self.slots[ls as usize];
            let gate = x.rep.cfg.flags & ferrite_core::msg::XbCfg::TONE_GATE != 0 && sl.rx_tone != 0;
            let open = st.squelch_open && (!gate || st.ctcss || st.cdcss);
            heard[listen as usize] = Some(open);
            self.live.slots[ls as usize].rssi = rssi;
            if x.rep.from() == Some(listen) || x.rep.from().is_none() {
                x.rep.set_rssi(rssi);
            }
        }
        match x.rep.tick(now, heard) {
            Some(XbCmd::Key { from }) => self.xb_key(&mut x, from, now).await,
            Some(XbCmd::Unkey { id }) => self.xb_unkey(&x, id, now).await,
            None => {}
        }
        // Audio (XB-15): the passing side's chip always (its audio is what goes out); the speaker per the
        // setting, or SIDE1's monitor on that side.
        let from = x.rep.from();
        let spk_set = x.rep.cfg.flags & ferrite_core::msg::XbCfg::SPEAKER != 0;
        let mut heard_slot = None;
        for i in 0..2u8 {
            let s = x.sides[i as usize];
            let n = Sel::rx(self.slots[s as usize].freq).chip();
            let open = from == Some(i) || heard[i as usize] == Some(true);
            if open {
                heard_slot = Some(s);
            }
            if self.has(n) && keyed_chip != Some(n) {
                self.chip(n).af_out(if open { AfOut::Normal } else { AfOut::Mute });
            }
            let sl = &mut self.live.slots[s as usize];
            sl.squelch_open = open;
            if !open {
                sl.rssi = sl.rssi.min(-121);
            }
        }
        let spk = heard_slot.is_some_and(|s| spk_set || self.monitor & 1 << s != 0);
        if self.path.keyed().is_some() {
            AMP_EN[0].write(true); // the audio stage carries the repeat audio (stock: PB8 on)
            AMP_EN[1].write(spk);
        } else {
            speaker(spk);
        }
        self.live.mode = ModeLive::Xb(x.rep.live);
        self.xb = Some(x);
    }

    /// The sequencer's settings for a pass: its timeout a little after the repeater's own (XB-9: the repeater
    /// unkeys first), the CW ID when the repeater asks for it (XB-12), tail elimination as the transmitting
    /// slot's; no end tone.
    fn xb_seq_cfg(&self, x: &XbRun, id: bool) -> txseq::Cfg {
        let to = x.rep.from().map_or(0, |f| x.sides[1 - f as usize]);
        txseq::Cfg {
            timeout_s: (x.rep.timeout_ms() / 1000 + 5).min(600) as u16,
            prealert_s: 0,
            end_tone: false,
            cw_id: id && self.tx.cw_len != 0,
            cw_each: true,
            tail: self.tx.tail[to as usize % 4],
        }
    }

    /// A pass: the other side's chip keyed on its own receive frequency (XB-18) with its tone, at the repeater's
    /// power (XB-3: Low or Mid), through the sequencer's checks; the receiving chip stays in receive (XB-6).
    async fn xb_key(&mut self, x: &mut XbRun, from: u8, now: u32) {
        if let Some(c) = crate::chan::TX_CAL.try_take() {
            (self.tx.cal, self.tx.gauge) = c;
        }
        let to = x.sides[1 - from as usize];
        let rx = self.slots[x.sides[from as usize] as usize].freq;
        let f = self.slots[to as usize].freq;
        let power = x.rep.cfg.power.min(1);
        let Some((region, extend)) = self.tx.region else {
            x.rep.refused(now);
            return log(b"xb refused: no region from the UI", 0);
        };
        let req = Req {
            tx: f,
            rx,
            kind: TxKind::Voice,
            person: false,
            confirmed: false,
            am: self.slots[to as usize].am,
            region,
            extend,
            cal_ok: self.tx.cal.is_some(),
            batt_ok: self.battery_ok(),
            power,
        };
        if let Err(r) = self.tx.seq.check(&req, &self.path, now) {
            x.rep.refused(now);
            return log(b"xb refused ", r as u32);
        }
        let n = Sel::tx(f).chip();
        if !self.has(n) {
            x.rep.refused(now);
            return;
        }
        let tone = self.tx.tone[to as usize % 4];
        let mut bk = self.chip(n);
        bk.wake();
        bk.af_out(AfOut::Mute);
        bk.set_freq(f * 10);
        match tone {
            0 => bk.subtone_off(),
            t if t & 0x8000 != 0 => bk.dcs_tx(t & 0x3FFF, t & 0x4000 != 0),
            t => bk.ctcss_tx(t),
        }
        self.tuned[n as usize] = Some((to, f));
        self.tx.pa = self.tx.cal.map_or(0, |c| c.dac(f, power));
        (self.tx.fault, self.tx.data, self.tx.slot, self.tx.dialled) = (false, Some(power), to, false);
        let mut st = Steps::default();
        if let Err(r) = self.tx.seq.key_xb(&req, &mut self.path, now, &mut st) {
            x.rep.refused(now);
            return log(b"xb refused ", r as u32);
        }
        self.run(&st).await;
        if self.tx.fault {
            let mut st = Steps::default();
            self.tx.seq.safe(&mut self.path, now_ms() as u32, &mut st);
            self.run(&st).await;
            x.rep.cut(now);
            self.xb_after_unkey(x);
            return log(b"xb read-back failed", 0);
        }
        self.tx.chip = Some(n);
        self.live.tx = TxLive { keyed: true, slot: to, mic: 0, refused: None, dtmf_sent: self.live.tx.dtmf_sent };
        log(b"xb pass to khz ", f / 100);
    }

    /// The end of a pass (XB-8): the CW ID first when due (XB-12) and tail elimination as the slot's, then the path
    /// machine's crossband unkey (PA 0 with the repeat audio on, the unkey wait, the TX path off, the mic back).
    async fn xb_unkey(&mut self, x: &XbRun, id: bool, now: u32) {
        let cfg = self.xb_seq_cfg(x, id);
        let mut st = Steps::default();
        self.tx.seq.release(&cfg, &mut self.path, now, &mut st);
        self.run(&st).await;
        self.xb_after_unkey(x);
        log(b"xb unkey, id ", id as u32);
    }

    /// After any unkey of a pass: the transmitting chip back to its side's receive settings on the next tick.
    fn xb_after_unkey(&mut self, _x: &XbRun) {
        if let Some(n) = self.tx.chip.take() {
            self.tuned[n as usize] = None;
        }
        self.tx.data = None;
        self.live.tx.keyed = false;
    }
}
