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
//! with stock's timing (decision 95: PA15 low 10-20 ms after the PA is up, high again at the receive drop while
//! still keyed; PB8 cleared at TX setup unless PC15 says an accessory; `analysis/crossband_repeat.md`, to confirm
//! on the bench).

use crate::board::{AMP_EN, BK_CS, BK_SCK, BK_SDA, RELAY, REPEAT_AUDIO, RF_PINS, SI4732_ADDR, SI_IN, SI_RST, SI_SCL, SI_SDA};
use crate::bus::{BusyDelay, Io, Out, SiBus};
use crate::chan::{ALERTS, DEBUG_ANS, DEBUG_REQ, EVENTS, LIVE, RADIO, TERM_WAKE};
use crate::hal::gpio::Mode;
use crate::hal::i2c::I2c;
use crate::tasks::{check, CHECK_BK0, CHECK_BK1};
use bk4829::wire::ThreeWire;
use bk4829::{int, AfOut, Bk4829};
use embassy_futures::select::{select3, Either3};
use embassy_time::{Instant, Timer};
use ferrite_core::freq::{self, mhz, Freq10};
use ferrite_core::msg::{bk_write_refused, BroadcastCmd, DebugAns, DebugReq, Event, Kind, LiveAll, ModeLive, Param, RadioCmd, SameDecoded, Source, SLOTS};
use ferrite_core::same_rx::{Assembler, Repeats, Step};
use ferrite_core::path::{Cause, Path, Rx, Sel, Steps};
use ferrite_radio::chip::{self as radio_chip, RxSetup};
use ferrite_radio::Pin;
use ferrite_core::rxsleep::{Action, Level, RxSleep};
use ferrite_core::scan::{self, Check, Range, Scan};
use ferrite_core::tonegen;
use ferrite_core::tonehint::{Act as HintAct, Hint};
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
    /// MODE-3: the UI's lease on the exclusive mode running here.
    lease: ferrite_core::lease::Lease,
    /// SQL-1: the calibration's squelch tables (stock's defaults until the UI task has read the sector), and what
    /// each chip was last given (REG_78, REG_4F), so a retune writes them only when they change.
    sqcal: ferrite_core::sqcal::SqCal,
    sq_regs: [Option<(u16, u16)>; 2],
    /// When the battery was last read (PWR-20).
    bat_at: Option<u32>,
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
    /// FM broadcast's receiver state for the live report (BCR-2, BCR-5).
    fm: Option<FmRx>,
    /// BCR-6: the SI4732 is the source being heard (broadcast on, no slot heard); its hard mute follows this.
    si_heard: bool,
    /// BCR-24: the broadcast's turn on the speaker (back 2 s after a slot heard goes quiet).
    bturn: watch::BcastTurn,
    /// BCR-23: the slot playing broadcast FM (no chip listens to it while FM plays).
    bcast_slot: Option<u8>,
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
    /// Quick mute from the UI, a bit per slot (HOME-15): still received, never on the speaker.
    user_mute: u8,
    /// Each slot's tone hint (TSRCH-12 to TSRCH-15): when to measure the tone on a carrier its tone squelch keeps
    /// shut, and what was heard.
    hints: [Hint; SLOTS],
    /// Radio-wide settings the UI sends: scan inputs (SCAN-22, for channel lists) and the subtone level (TONE-16,
    /// the `tx` build's).
    scan_tx: bool,
    tone_level: u8,
    /// The beep volume's tone gain (SND-3, 0 7 5 2: `tonegen::beep_gain`).
    beep_gain: u8,
    /// The DAC beep's level (1-11, `hal::tone`), from the beep volume (SND-3: Low, Mid, High -> 5, 7, 9).
    beep_level: u8,
    hf: Option<HfOn>,
    /// The frequency counter (TSRCH-10), while on.
    counter: Option<Counter>,
    /// The band scope (SCOPE-4), while it runs; its points per sweep and dwell (0.1 ms) from the last ScopeCfg.
    scope: Option<ScopeRun>,
    scope_cfg: (u8, u8),
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

/// The band scope sweeping (SCOPE-4, SCOPE-5): one chip, its audio off, one point per wait. As stock (V0.27
/// 0x08023808 / 0x080235E8): the frequency (REG_38/39), REG_30 0x0200 then 0xBFF1 (the VCO recalibration), the
/// dwell, then REG_67. Stock dwells about 10 ms a point (its 10 ms tick); the dwell is 0 3 2 3's.
struct ScopeRun {
    chip: u8,
    centre: Freq10,
    step: Freq10,
    n: u8,
    dwell_us: u32,
    /// The point measured at the next wait (tuned already), and the raw level of each point last sweep.
    i: u8,
    last: [u8; ferrite_core::msg::SCOPE_BARS],
    sweeps: u8,
    started: u32,
    /// Listening on a frequency (SCOPE-11) instead of sweeping; its demod (0 FM, 1 NFM, 2 AM).
    listen: Freq10,
    mode: u8,
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

use radio_chip::{COUNTER_TUNE, WX};

/// A chip gives up measuring after this long with no carrier, and the other chip takes a turn (stock's scan time
/// is 0.8 s per reading).
const COUNTER_TURN_MS: u32 = 2_000;

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
    ptt: ferrite_radio::ptt::PttGuard,
    /// The calibrated PA drive for the key-up being run (`Act::PaRamp`).
    pa: u8,
    /// The read-back failed during a key-up (TX-7): the plan stops and the safe state follows.
    fault: bool,
    /// Each chip's REG_3B as read after its start-up (TX-7: Ferrite never writes it, so a key-up expects it unchanged).
    reg3b: [u16; 2],
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
    /// VOX (VOX-2, VOX-5, VOX-14): level 1-9 or 0 off, tail in 100 ms, the detector, and when PA0 was last read.
    vox: (u8, u8),
    vox_det: ferrite_core::vox::Vox,
    vox_at: u32,
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

    /// Carry out a path plan, waits included, before anything else reaches the pins (`ferrite_radio::run`,
    /// host-tested on a recording mock through [`ferrite_radio::Hw`] below).
    async fn run(&mut self, steps: &Steps) {
        ferrite_radio::run(self, steps).await;
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
        // A probe on the slot this chip had, or gets, ends: tuning sets the slot's own detector (TSRCH-12).
        for (s, _) in [self.tuned[n as usize], want].into_iter().flatten() {
            if let Some(h) = self.hints.get_mut(s as usize) {
                h.retuned();
            }
        }
        self.tuned[n as usize] = want;
        let Some((s, f)) = want else {
            self.chip(n).sleep();
            self.sq_regs[n as usize] = None; // written again on waking
            return;
        };
        let slot = match s {
            s if s < SLOTS as u8 => self.slots[s as usize],
            SCOPE_SLOT => {
                let m = self.scope.as_ref().map_or(0, |sc| sc.mode);
                SlotRx { freq: f, am: m == 2, narrow: m == 1, ..SlotRx::DEFAULT }
            }
            // HF mode on chip 1 (18-64 MHz): AM is the mode's own (Param::Mode 2); FM otherwise.
            _ => SlotRx { freq: f, am: self.hf.is_some_and(|h| h.mode == 2), ..SlotRx::DEFAULT },
        };
        // Stock's modes 6 and 7, the sub-tone detector, muted until the audio decision (`ferrite_radio::chip`).
        let setup = RxSetup { freq: f, narrow: slot.narrow, am: slot.am, rx_tone: slot.rx_tone, rx_tail: slot.rx_tail };
        radio_chip::tune(&mut self.chip(n), &setup);
        // SQL-1: stock's thresholds for this frequency and the slot's level (stock writes them at each receive
        // setup); only when they differ from what the chip has.
        let regs = self.sqcal.regs(f, slot.sq);
        if self.sq_regs[n as usize] != Some(regs) {
            radio_chip::squelch(&mut self.chip(n), regs);
            self.sq_regs[n as usize] = Some(regs);
        }
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
            radio_chip::same_arm(&mut bk);
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
                // BCR-19: only the BK4829s sleep; a broadcast playing keeps the SI4732 and the speaker.
                speaker(self.si_heard && !self.muted);
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
        set.count_ones() > 1 || self.scans.iter().any(|s| s.running()) || self.path.keyed().is_some() || self.counter.is_some() || self.scope.is_some() || xb
    }

    /// The engines' tick: each listening chip's squelch, the watch turn, the scans, the chips and the path, and
    /// the audio.
    /// SBAR-20: the MCU's die temperature every 10 s, for the Status page (informational; read here because the
    /// Radio task owns the ADC's software reads).
    fn mcu_temp(&mut self) {
        use crate::hal::adc;
        let now = now_ms() as u32;
        // PWR-20: the battery (PA1) every 2 s, here too, as the Radio task owns the ADC's software reads.
        if self.bat_at.is_none_or(|t| now.wrapping_sub(t) >= 2_000) {
            self.bat_at = Some(now);
            adc::init();
            crate::chan::BATTERY_RAW.store(adc::read_avg(adc::CH_PA1, 8), core::sync::atomic::Ordering::Relaxed);
        }
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

    /// VOX (VOX-2, VOX-3, VOX-5, VOX-14): every 20 ms, PA0's mic level against the level's threshold; speech
    /// (with its tail) goes to the UI in `TxLive::voice`, which keys the active slot (VOX-13). Blocked while the
    /// active slot receives and nothing is keyed (VOX-3). The accessory-change debounce isn't modelled yet.
    #[cfg(feature = "tx")]
    fn vox_poll(&mut self) {
        use crate::hal::adc;
        let (level, tail) = self.tx.vox;
        if level == 0 {
            return;
        }
        let now = now_ms() as u32;
        if now.wrapping_sub(self.tx.vox_at) < 20 {
            return;
        }
        self.tx.vox_at = now;
        adc::init();
        let mic = adc::read(adc::CH_PA0);
        let blocked = self.path.keyed().is_none() && self.live.slots[self.active as usize].squelch_open;
        self.live.tx.voice = self.tx.vox_det.step(mic, level, tail as u32 * 100, blocked, now);
    }

    async fn tick(&mut self) {
        self.mcu_temp();
        self.fm_poll();
        // SQL-1: the calibration's squelch tables, once the UI task has read them: every tuned chip again.
        if let Some(c) = crate::chan::SQ_CAL.try_take() {
            self.sqcal = c;
            for n in 0..2u8 {
                if let Some((s, f)) = self.tuned[n as usize] {
                    let level = self.slots.get(s as usize).map_or(self.slots[self.active as usize].sq, |sl| sl.sq);
                    let regs = self.sqcal.regs(f, level);
                    radio_chip::squelch(&mut self.chip(n), regs);
                    self.sq_regs[n as usize] = Some(regs);
                }
            }
        }
        #[cfg(feature = "tx")]
        self.vox_poll();
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
        // Battery Save asleep: only its phase change runs (PWR-*). Returning before it, as this did, left its wake
        // time in the past, so the main loop's wait fired at once forever: the Radio task spun above the UI and the
        // receivers never woke (on the black radio, Oct 12, 2026, once real squelch thresholds let it sleep).
        if self.save.asleep() {
            let a = self.save.tick(now_ms(), false);
            self.apply(a).await;
            if self.save.asleep() {
                self.bcast_audio(now_ms() as u32, false); // BCR-19: the broadcast plays on, and returns on time
                return;
            }
        }
        let now = now_ms() as u32;
        if self.scope.is_some() {
            return self.scope_tick(now).await;
        }
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
            // TSRCH-12: while the tone hint probes, the detectors scan: their reading, not a match, so the audio
            // stays shut this tick.
            let probing = self.hints[s as usize].probing();
            toned[n as usize] =
                !probing && open && if sl.rx_tone == 0 { !(sl.rx_tail && st.tail) } else { st.ctcss || st.cdcss };
            let sl = &mut self.live.slots[s as usize];
            sl.rssi = rssi;
            scan_open[s as usize] = match self.scans[s as usize].check() {
                Check::Rssi => rssi > *fl + FAST_MARGIN_DB,
                Check::Squelch => open,
            };
            if probing {
                let (c, d) = self.chip(n).subtone_scan();
                let heard = radio_chip::heard_tone(c, d, &ferrite_app::slot::CTCSS, &ferrite_app::slot::DCS);
                if heard.is_some_and(|w| self.hints[s as usize].found(now, w) == HintAct::Restore) {
                    let sl = self.slots[s as usize];
                    radio_chip::subtone(&mut self.chip(n), sl.rx_tone, sl.rx_tail);
                }
            }
        }
        // SAME on a weather channel's chip (DEC-19).
        self.same_tick(now);
        // 2. The watch turn.
        let cfg = watch::Cfg { set: self.watch_set, active: self.active, freq: self.slots.map(|s| s.freq), muted: self.user_mute, off: self.bcast_off() };
        let keyed = self.path.keyed().is_some();
        let out = self.watcher.tick(now, &cfg, sq, keyed);
        // 2a. The tone hint (TSRCH-12, TSRCH-15): on the chip listening, for the slot the watch holds for audio.
        self.hint_tick(now, listening, &sq, &toned, out.audio);
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
        let monitor_active = self.monitor & !self.user_mute & 1 << self.active != 0; // HOME-15: muted, not heard
        let audio = if monitor_active { Some(self.active) } else { out.audio };
        let audio_chip = audio.and_then(heard_chip).filter(|&n| monitor_active || toned[n as usize]);
        for n in 0..2u8 {
            if self.tuned[n as usize].is_some() {
                self.chip(n).af_out(if Some(n) == audio_chip { AfOut::Normal } else { AfOut::Mute });
            }
        }
        self.bcast_audio(now, audio_chip.is_some());
        // 6. The snapshot.
        let active_relay = Sel::rx(self.slots[self.active as usize].freq).relay();
        for (i, sl) in self.live.slots.iter_mut().enumerate() {
            let open = self.watcher.open(i as u8) || self.monitor & 1 << i != 0;
            if !open {
                sl.rssi = sl.rssi.min(-121);
            }
            sl.squelch_open = open;
            sl.heard_tone = self.hints[i].heard;
            sl.relay = out.cancelled && self.watch_set & 1 << i != 0 && Sel::rx(self.slots[i].freq).relay() != active_relay;
        }
        // Battery Save: anything open or scanning keeps the receivers awake. Broadcast doesn't (BCR-19): the
        // BK4829s keep the level's cycle for the slots watched, and the sleep step leaves the SI4732 playing.
        let busy = self.live.slots.iter().any(|s| s.squelch_open || s.scanning);
        let a = self.save.tick(now_ms(), busy);
        self.apply(a).await;
    }

    /// BCR-6: in FM or AM broadcast the SI4732 is the source while no slot is heard (radio monitor); a heard slot
    /// mutes it and plays the BK4829 instead, and the broadcast returns 2 s after it goes quiet (BCR-24). A
    /// broadcast slot muted with \* stays muted (HOME-15). Then the speaker for whichever plays. Without this the
    /// amp stayed off in FM broadcast on the radio (Oct 12, 2026): only the BK4829s were ever counted as a source.
    /// Also run while Battery Save has the BK4829s asleep (BCR-19), so the broadcast comes back on time.
    fn bcast_audio(&mut self, now: u32, slot_heard: bool) {
        let broadcast = self.si.mode().is_some() && self.hf.is_none();
        let turn = self.bturn.tick(now, slot_heard);
        let muted = self.bcast_slot.is_some_and(|b| self.user_mute & 1 << b != 0);
        let si_heard = broadcast && turn && !muted;
        if self.si_heard != si_heard {
            if broadcast {
                let _ = self.si.mute(!si_heard, &mut BusyDelay);
            }
            self.si_heard = si_heard;
        }
        speaker((slot_heard || si_heard) && !self.muted);
    }

    /// The tone hint's step for the slot on the listening chip (TSRCH-12, TSRCH-15): eligible while the watch
    /// holds it for audio, it has a receive tone, and no scan or latched monitor runs on it (transmit, HF mode and
    /// the exclusive modes never reach the receive tick). The chip's detectors switch to scan, or back.
    fn hint_tick(&mut self, now: u32, listening: Option<u8>, sq: &[bool; 2], toned: &[bool; 2], audio: Option<u8>) {
        for h in &mut self.hints {
            h.age(now);
        }
        let Some(n) = listening else { return };
        let Some((s, _)) = self.tuned[n as usize].filter(|&(s, _)| (s as usize) < SLOTS) else { return };
        let i = s as usize;
        let sl = self.slots[i];
        let eligible = audio == Some(s) && !self.scans[i].running() && self.monitor & 1 << s == 0;
        match self.hints[i].tick(now, sl.freq, sl.rx_tone, sq[n as usize], toned[n as usize], eligible) {
            HintAct::Start => radio_chip::tone_probe(&mut self.chip(n)),
            HintAct::Restore => radio_chip::subtone(&mut self.chip(n), sl.rx_tone, sl.rx_tail),
            HintAct::None => {}
        }
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
            log(if ok { b"hf si am ok, khz " } else { b"hf si FAILED, khz " }, khz as u32);
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
    /// BCR-2, BCR-5: FM broadcast's report ten times a second (the signal, stereo pilot and SNR: about 2 ms of I2C
    /// each) and, every other time, up to 8 RDS groups into the decoder (station name, radio text). Nothing in HF
    /// mode or with FM off.
    #[inline(never)] // in slow flash (memory.x): ten times a second at most
    fn fm_poll(&mut self) {
        let now = now_ms() as u32;
        let Some(fm) = self.fm.as_mut() else { return };
        if self.hf.is_some() || now.wrapping_sub(fm.poll_at) > u32::MAX / 2 {
            return;
        }
        fm.poll_at = now.wrapping_add(100);
        fm.rds_turn = !fm.rds_turn;
        let d = &mut BusyDelay;
        let q = self.si.rsq(d).ok();
        for _ in 0..if fm.rds_turn { 8 } else { 0 } {
            match self.si.rds_read(d) {
                Ok(Some(g)) => fm.rds.feed(g.blocks[0], g.blocks[1], g.blocks[2], g.blocks[3], g.errors),
                _ => break,
            }
        }
        // BCR-30: the radio text whole, once complete; until then (and through an A/B change) the last one shown.
        let rt = match self.live.mode {
            _ if fm.rds.rt_complete() => fm.rds.rt,
            ModeLive::Fm { rt, .. } => rt,
            _ => [0; 64],
        };
        self.live.mode = ModeLive::Fm {
            freq: fm.freq,
            ps: fm.rds.ps,
            rt,
            rssi: q.map_or(0, |q| q.rssi),
            snr: q.map_or(0, |q| q.snr),
            stereo: q.is_some_and(|q| q.stereo),
            heard: self.si_heard,
        };
    }

    /// BCR-23: while FM broadcast plays no chip listens to the broadcast slot, nor to a slot needing the band relay,
    /// which FM leaves released (BCR-15). In slow flash (memory.x): once a receive tick.
    #[inline(never)]
    fn bcast_off(&self) -> u8 {
        if self.fm.is_none() {
            return 0;
        }
        let relay = (0..SLOTS).filter(|&i| Sel::rx(self.slots[i].freq).relay()).fold(0u8, |m, i| m | 1 << i);
        relay | self.bcast_slot.map_or(0, |s| 1 << s)
    }

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
        // The cable log (TERM-28): the HF receiver's level once a second.
        static LOGGED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let now = now_ms() as u32;
        if now.wrapping_sub(LOGGED.load(core::sync::atomic::Ordering::Relaxed)) >= 1000 {
            LOGGED.store(now, core::sync::atomic::Ordering::Relaxed);
            log(b"hf rssi -dbm ", (-(rssi as i32)).max(0) as u32);
            log(b"hf snr db ", snr as u32);
        }
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
        // As stock (V0.27, Oct 12, 2026 trace): key beeps come from the MCU's DAC1 (PA4) into the speaker amp, not
        // a BK4829's tone generator (taking a receiving chip into tone mode and back squawked like a squelch break
        // on the radio). The chips keep receiving; the one passing audio is only muted for the beep (REG_47), and
        // the audio decision unmutes it on the next tick. Stock's order: DAC on, 30 ms, amp on; amp off, 30 ms,
        // DAC off.
        for n in 0..2u8 {
            if self.has(n) && self.tuned[n as usize].is_some() {
                self.chip(n).af_out(AfOut::Mute);
            }
        }
        crate::hal::tone::start(hz as u32, self.beep_level);
        Timer::after_millis(30).await;
        speaker(true);
        Timer::after_millis(ms.min(tonegen::BEEP_MAX_MS) as u64).await;
        speaker(false);
        Timer::after_millis(30).await;
        crate::hal::tone::stop();
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

/// The scope's own index in [`Radio::tuned`] (after the counter's).
const SCOPE_SLOT: u8 = 5;

impl Radio {
    /// SCOPE-4, SCOPE-13: the scope on (the centre's chip sweeps, the other sleeps, the relay stays) or off (the
    /// engines retune on the next tick).
    async fn scope(&mut self, centre: Freq10, step: u16, on: bool) {
        self.tuned = [None; 2];
        speaker(false);
        if !on {
            for n in 0..2u8 {
                if self.has(n) {
                    self.chip(n).af_out(AfOut::Mute);
                }
            }
            self.scope = None;
            self.live.mode = ModeLive::Off;
            log(b"scope ", 0);
            return;
        }
        let chip = Sel::rx(centre).chip();
        let (n, tenths) = self.scope_cfg;
        let n = n.clamp(1, ferrite_core::msg::SCOPE_BARS as u8);
        let keep = self.scope.as_ref().filter(|sc| sc.centre == centre && sc.step == step as Freq10 && sc.n == n);
        let last = keep.map_or([0; ferrite_core::msg::SCOPE_BARS], |sc| sc.last);
        self.scope = Some(ScopeRun {
            chip,
            centre,
            step: step as Freq10,
            n,
            dwell_us: tenths.max(1) as u32 * 100,
            i: 0,
            last,
            sweeps: 0,
            started: now_ms() as u32,
            listen: 0,
            mode: 0,
        });
        self.live.mode = ModeLive::Scope { centre, bars: last, n, sweeps: 0, listen: 0, open: false };
        if self.has(1 - chip) {
            self.tune_chip(1 - chip, None);
        }
        // The chip's full receive setup once on the centre (filter, demod, squelch), muted; the path asked for with
        // Cause::Auto, so the relay never moves (SCOPE-5).
        self.tune_chip(chip, Some((SCOPE_SLOT, centre)));
        self.chip(chip).af_out(AfOut::Mute);
        self.select_rx(centre, Cause::Auto).await;
        self.scope_point();
        log(b"scope points ", n as u32);
    }

    /// SCOPE-11: listen on `freq` with the scope's mode (the path set first); 0 goes back to sweeping.
    async fn scope_listen(&mut self, freq: Freq10, mode: u8) {
        let Some(sc) = self.scope.as_mut() else { return };
        (sc.listen, sc.mode) = (freq, mode);
        let chip = sc.chip;
        let back = sc.centre;
        self.tuned[chip as usize] = None;
        self.tune_chip(chip, Some((SCOPE_SLOT, if freq != 0 { freq } else { back })));
        self.chip(chip).af_out(AfOut::Mute);
        if freq != 0 {
            self.select_rx(freq, Cause::Auto).await;
        } else {
            speaker(false);
            self.scope_point();
        }
    }

    /// The frequency of point `i`, or None when it needs the other chip or relay state (SCOPE-5) or no chip
    /// receives it.
    fn scope_freq(sc: &ScopeRun, i: u8) -> Option<Freq10> {
        let low = sc.centre as i64 - sc.step as i64 * sc.n as i64 / 2;
        let f = low + sc.step as i64 * i as i64;
        let f = Freq10::try_from(f).ok()?;
        let (here, centre) = (Sel::rx(f), Sel::rx(sc.centre));
        (here.chip() == centre.chip() && here.relay() == centre.relay() && freq::receivable(f)).then_some(f)
    }

    /// Tune the scope's chip to its current point: stock's per-point sequence, frequency then the REG_30 retune.
    fn scope_point(&mut self) {
        let Some(sc) = self.scope.as_ref() else { return };
        if let Some(f) = Self::scope_freq(sc, sc.i) {
            let chip = sc.chip;
            let mut bk = self.chip(chip);
            bk.set_freq(f * 10);
            bk.rx_on();
        }
    }

    /// The scope's tick: while listening, the squelch decides the audio; while sweeping, the point tuned at the
    /// last wait is read (after the dwell) and the next is tuned. Each sweep's time goes to the bring-up log.
    async fn scope_tick(&mut self, now: u32) {
        let Some(mut sc) = self.scope.take() else { return };
        let chip = sc.chip;
        if sc.listen != 0 {
            let st = self.chip(chip).status();
            let open = st.squelch_open;
            self.chip(chip).af_out(if open { AfOut::Normal } else { AfOut::Mute });
            speaker(open && !self.muted);
            if let ModeLive::Scope { listen, open: o, .. } = &mut self.live.mode {
                (*listen, *o) = (sc.listen, open);
            }
            self.scope = Some(sc);
            return;
        }
        let i = sc.i as usize;
        let level = if Self::scope_freq(&sc, sc.i).is_some() {
            let dbm = self.chip(chip).rssi_dbm();
            (dbm + 160).clamp(1, 255) as u8
        } else {
            0
        };
        let bar = if level != 0 && sc.last[i] != 0 { ((level as u16 + sc.last[i] as u16) / 2) as u8 } else { level };
        sc.last[i] = level;
        if let ModeLive::Scope { bars, listen, open, .. } = &mut self.live.mode {
            bars[i] = bar;
            (*listen, *open) = (0, false);
        }
        sc.i += 1;
        if sc.i >= sc.n {
            sc.i = 0;
            sc.sweeps = sc.sweeps.wrapping_add(1);
            if let ModeLive::Scope { sweeps, .. } = &mut self.live.mode {
                *sweeps = sc.sweeps;
            }
            log(b"scope sweep ms ", now.wrapping_sub(sc.started));
            sc.started = now;
        }
        self.scope = Some(sc);
        self.scope_point();
    }
}

/// The counter's own index in [`Radio::tuned`] (after the three slots and HF mode's).
const COUNTER_SLOT: u8 = 4;

/// The pins, waits and chip steps a path plan moves ([`ferrite_radio::run`]).
impl ferrite_radio::Hw for Radio {
    fn pin(&mut self, p: Pin, high: bool) {
        match p {
            Pin::Sel(s) => RF_PINS[s as usize].write(high),
            Pin::PaDrive => RF_PINS[6].write(high),
            Pin::Relay => RELAY.write(high),
            Pin::SiIn(i) => SI_IN[i as usize & 1].write(high),
            Pin::RepeatAudio => REPEAT_AUDIO.write(high),
            Pin::Amp(i) => AMP_EN[i as usize & 1].write(high),
        }
    }
    fn relay_is(&mut self) -> bool {
        RELAY.read()
    }
    async fn wait_ms(&mut self, ms: u32) {
        Timer::after_millis(ms as u64).await
    }
    fn muted(&mut self, m: bool) {
        self.muted = m;
    }
    fn si_used(&mut self) -> bool {
        self.si_used
    }
    fn set_si_used(&mut self) {
        self.si_used = true;
    }
    fn log(&mut self, tag: &[u8], v: u32) {
        log(tag, v);
    }
    #[cfg(feature = "tx")]
    fn pa_zero(&mut self) {
        crate::hal::pa::zero();
    }
    #[cfg(feature = "tx")]
    fn pa_set(&mut self, v: u8) {
        crate::hal::pa::set(v);
    }
    #[cfg(feature = "tx")]
    fn pa_level(&mut self) -> u8 {
        self.tx.pa
    }
    #[cfg(feature = "tx")]
    fn key_chip(&mut self, n: u8) -> bool {
        Radio::key_chip(self, n)
    }
    #[cfg(feature = "tx")]
    fn chip_fault(&mut self, _n: u8) {
        self.tx.fault = true;
    }
    #[cfg(feature = "tx")]
    fn unkey_chip(&mut self, n: u8) {
        if self.has(n) {
            self.chip(n).tx_off();
        }
    }
    // TONE-8's order at the unkey, still keyed: end tone, CW ID when due, tail elimination.
    #[cfg(feature = "tx")]
    async fn end_tone(&mut self) {
        let e = self.tx.end[self.tx.slot as usize % 4];
        let t = tonegen::END_TONES.get(e as usize).copied().unwrap_or(&[]);
        self.play_keyed(t.iter().copied()).await;
    }
    #[cfg(feature = "tx")]
    async fn cw_id(&mut self) {
        let (call, len) = (self.tx.cw_call, self.tx.cw_len as usize);
        let cw = tonegen::Cw::new(&call[..len.min(10)], tonegen::CW_WPM);
        self.play_keyed(cw.map(|(on, ms)| (if on { tonegen::CW_HZ } else { 0 }, ms))).await;
    }
    #[cfg(feature = "tx")]
    async fn tail_elim(&mut self) {
        Radio::tail_elim(self).await
    }
}

/// The SI4732's input path for its mode, as stock (BCR-20, `analysis/audio_path.md` §2): FM broadcast PA7 = PC5 = 0,
/// AM (and SW, SSB, CW) PA7 = PC5 = 1, the LNA path.
fn si_input(mode: SiMode) {
    let [a, b] = radio_chip::si_input(mode != SiMode::Fm);
    SI_IN[0].write(a);
    SI_IN[1].write(b);
}

/// The boot check of the SI4732: reset pulse, FM power-up, ready within 500 ms, its revision read, then powered
/// down and held in reset. True when it answered.
async fn si_probe(si: &mut Si4732<SiBus>) -> bool {
    let d = &mut BusyDelay;
    SI_RST.write(false);
    Timer::after_millis(10).await;
    SI_RST.write(true);
    Timer::after_millis(10).await;
    let mut ok = si.power_up_start(SiMode::Fm).is_ok();
    if ok {
        let t0 = now_ms();
        ok = loop {
            match si.ready() {
                Ok(true) => break true,
                Ok(false) if now_ms() - t0 < 500 => Timer::after_millis(5).await,
                _ => break false,
            }
        };
    }
    if ok {
        ok = si.revision(d).is_ok();
    }
    let _ = si.power_down(d); // also clears the driver's mode when the chip didn't answer
    SI_RST.write(false);
    ok
}

/// The broadcast receiver: powered up in the mode asked for (reset pulse, power-up, crystal settling), then
/// tuned by polling. FM in 10 kHz units, AM in kHz. PA7/PC5 take stock's state for the mode (BCR-20). SSB (patch from SPI flash) comes with the link's patch import.
/// FM broadcast while on: the tuned frequency (10 kHz), the RDS decoder, the next report time.
struct FmRx {
    freq: u16,
    rds: si4732::rds::Rds,
    poll_at: u32,
    /// RDS is read on every other report (5 times a second).
    rds_turn: bool,
}

/// Returns the frequency the receiver tuned to (FM 10 kHz units, AM kHz; 0 when switched off).
async fn broadcast(si: &mut Si4732<SiBus>, cmd: BroadcastCmd) -> Result<u16, si4732::Error> {
    let d = &mut BusyDelay;
    let (mode, freq) = match cmd {
        BroadcastCmd::Off => {
            if si.mode().is_some() {
                si.power_down(d)?;
            }
            return Ok(0);
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
        if mode == SiMode::Fm {
            let _ = si.rds_on(d); // BCR-2: station name and radio text
        }
    }
    if let Some(f) = freq {
        si.tune_start(f, Sideband::Usb, d)?;
    }
    loop {
        if let Some(t) = si.tune_poll(d)? {
            return Ok(t.freq);
        }
        Timer::after_millis(10).await;
    }
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
        lease: ferrite_core::lease::Lease::new(),
        bat_at: None,
        sqcal: ferrite_core::sqcal::SqCal::DEFAULT,
        sq_regs: [None; 2],
        path: Path::new(),
        muted: false,
        si_used: false,
        si_heard: false,
        bturn: watch::BcastTurn::new(),
        bcast_slot: None,
        fm: None,
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
        user_mute: 0,
        hints: [Hint::new(); SLOTS],
        scan_tx: false,
        tone_level: 1,
        beep_gain: tonegen::beep_gain(3), // Mid
        beep_level: 7,
        hf: None,
        counter: None,
        scope: None,
        scope_cfg: (60, 100),
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
            ptt: ferrite_radio::ptt::PttGuard::default(),
            pa: 0,
            fault: false,
            reg3b: [0; 2],
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
            vox: (0, 20),
            vox_det: ferrite_core::vox::Vox::new(),
            vox_at: 0,
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
            #[cfg(feature = "tx")]
            {
                r.tx.reg3b[n as usize] = r.chip(n).read(0x3B); // TX-7's reference
            }
            r.chip(n).sleep(); // until the engines give it a slot
        }
        check(Source::Radio, if n == 0 { CHECK_BK0 } else { CHECK_BK1 }, found);
    }
    r.live.chips = r.chips;
    for (i, sl) in r.live.slots.iter_mut().enumerate() {
        (sl.freq, sl.rssi) = (r.slots[i].freq, -121);
    }
    // SELF-*: the SI4732 at boot: out of reset (PA6), FM power-up, answering within 500 ms, then powered down and
    // held in reset again until FM broadcast or HF mode needs it. Silent (no audio path changes).
    let si_ok = si_probe(&mut r.si).await;
    check(Source::Radio, crate::tasks::CHECK_SI, si_ok);
    r.select_rx(DEFAULT, Cause::ActiveSlot).await;
    r.tick().await;
    let live = LIVE.sender();
    live.send(r.live);

    loop {
        // Wake for the next command, the next Battery Save phase change, or (awake) the next tick.
        let mut at = r.save.deadline();
        // Broadcast FM's report (fm_poll) runs on the tick, also while Battery Save has the BK4829s asleep (BCR-19).
        if !r.save.asleep() || r.hf.is_some() || r.fm.is_some() {
            at = at.min(now_ms() + if r.busy() { TICK_MS } else { POLL_MS });
        }
        // MODE-3: a held exclusive mode is checked by its lease's end at the latest.
        r.lease_check().await;
        if let Some(d) = r.lease.deadline() {
            at = at.min(d as u64 + 1);
        }
        // Never wait on a time already past: that wait fires at once, and this task runs above the UI.
        at = at.max(now_ms() + 1);
        // The band scope waits its dwell between points (0.1 ms steps), listening or not at the tick.
        let wake = match &r.scope {
            // At least 2 ms between points whatever the dwell: each point is about 1.25 ms of BK4829 writes on a task
            // that runs above the UI, and 1 ms points starved it (keys dead on the radio, Oct 12, 2026).
            Some(sc) if sc.listen == 0 => Instant::now() + embassy_time::Duration::from_micros(sc.dwell_us.max(2_000) as u64),
            _ => Instant::from_millis(at),
        };
        match select3(RADIO.receive(), Timer::at(wake), DEBUG_REQ.receive()).await {
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
    /// MODE-3: the UI stopped renewing the exclusive mode (an app bug, or a UI too busy to run): stop it here and
    /// go back to normal receive, and tell the UI through the snapshot's drop count.
    async fn lease_check(&mut self) {
        // Commands still queued (this task was busy, say loading the SSB patch) may hold the renewals.
        if !crate::chan::RADIO.is_empty() {
            return;
        }
        let Some(h) = self.lease.expired(now_ms() as u32) else { return };
        log(b"mode lease ran out, stopped ", h as u32);
        self.cmd(h.stop_cmd()).await;
        self.live.lease_drops = self.live.lease_drops.wrapping_add(1);
    }

    /// One command from the UI.
    async fn cmd(&mut self, c: RadioCmd) {
        let now = now_ms() as u32;
        // The single-receiver command is the active slot's tune.
        let c = if let RadioCmd::Tune(freq) = c { RadioCmd::TuneSlot { slot: self.active, freq } } else { c };
        self.lease.note(&c, now); // MODE-3
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
            RadioCmd::Mute(m) => self.user_mute = m,
            RadioCmd::ScanTx(on) => self.scan_tx = on,
            RadioCmd::ToneLevel(l) => self.tone_level = l.min(2),
            RadioCmd::BeepVol(v) => {
                // SND-3: Low, Mid, High. The DAC beep at levels 5, 7, 9 (a quiet Low; 11 would be full scale); the DTMF sidetone (on the
                // BK4829, it goes on air) at the matching points of the generator's old 1-5 scale.
                let i = v.clamp(1, 3) as usize - 1;
                self.beep_gain = tonegen::beep_gain([2, 3, 5][i]);
                self.beep_level = [5, 7, 9][i];
            }
            #[cfg(feature = "tx")]
            RadioCmd::Vox { level, tail } => {
                self.tx.vox = (level.min(9), tail.max(1));
                if level == 0 {
                    self.tx.vox_det = ferrite_core::vox::Vox::new();
                    self.live.tx.voice = false;
                }
            }
            // Nothing keys in this build, so VOX has nothing to do.
            #[cfg(not(feature = "tx"))]
            RadioCmd::Vox { .. } => {}
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
                // Errors: the UI shows no station. The cable log (TERM-28) says what the SI4732 did.
                let r = broadcast(&mut self.si, b).await;
                // BCR-2, BCR-5: FM's live report (frequency, signal, stereo, RDS) from here on; the tick polls it.
                self.fm = match (b, &r) {
                    (BroadcastCmd::Off, _) | (BroadcastCmd::Am(_), _) | (_, Err(_)) => None,
                    (_, Ok(f)) => Some(FmRx { freq: *f, rds: si4732::rds::Rds::init(), poll_at: 0, rds_turn: false }),
                };
                // A new station (or none): nothing of the last one's RDS carries over.
                if matches!(self.live.mode, ModeLive::Fm { .. }) {
                    self.live.mode = ModeLive::Off;
                }
                match (b, r.is_ok()) {
                    (BroadcastCmd::Off, _) => log(b"si off", 0),
                    (BroadcastCmd::Fm(f), true) => log(b"si fm ok, 10 khz ", f as u32),
                    (BroadcastCmd::Am(k), true) => log(b"si am ok, khz ", k as u32),
                    (BroadcastCmd::Seek { .. }, true) => log(b"si seek ok", 0),
                    (_, false) => log(b"si FAILED, cmd ", match b { BroadcastCmd::Fm(_) => 1, BroadcastCmd::Am(_) => 2, _ => 3 }),
                }
                if r.is_ok() && b != BroadcastCmd::Off {
                    if let Ok(q) = self.si.rsq(&mut BusyDelay) {
                        log(b"si rssi dbuv ", q.rssi as u32);
                        log(b"si snr db ", q.snr as u32);
                    }
                }
            }
            RadioCmd::BcastSlot(s) => self.bcast_slot = s,
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
            RadioCmd::Lease => {} // MODE-3: renewed above (`Lease::note`)
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
            RadioCmd::ScopeCfg { bars, dwell } => self.scope_cfg = (bars, dwell),
            RadioCmd::Sweep { centre, step, on } => self.scope(centre, step, on).await,
            RadioCmd::ScopeListen { freq, mode } => self.scope_listen(freq, mode).await,
            // Tone search: its own exclusive mode, after 2d (chapter 22).
            RadioCmd::ToneSearch { .. } => {}
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
        let was = self.tx.ptt.seen(false);
        let ok = self.tx.ptt.seen(KEY_PTT.read() && KEY_SIDE1.read() && KEY_SIDE2.read()); // `ferrite_radio::ptt`
        if ok && !was {
            log(b"ptt released since boot", 0);
        }
        ok
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

    /// `Act::ChipTx`: the chip already holds the TX frequency and tone; key it with the microphone and read back
    /// REG_30, the frequency, REG_3B and REG_36 before the PA (TX-7, decision 97), with stock's REG_36 for the level
    /// ({0x7F, 0x57, 0x57}[level], `analysis/calibration.md`).
    fn key_chip(&mut self, n: u8) -> bool {
        if !self.has(n) {
            return false;
        }
        // The frequency both key-up paths wrote just before the plan ran (`transmit`, `xb_key`).
        let Some((_, freq)) = self.tuned[n as usize] else {
            return false;
        };
        let high = self.tx.data.unwrap_or(self.tx.power[self.live.tx.slot as usize % 4]) == 2;
        let want = radio_chip::TxExpect { freq, reg3b: self.tx.reg3b[n as usize] };
        match radio_chip::key_mic(&mut self.chip(n), high, &want) {
            Ok(()) => true,
            Err(r) => {
                log(b"tx read-back wrong, register ", r as u32);
                false
            }
        }
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
        self.live.tx = TxLive { keyed: false, slot, mic: 0, refused: None, dtmf_sent: self.live.tx.dtmf_sent, voice: self.live.tx.voice };
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
        let Some(n) = self.tx.chip.filter(|&n| self.has(n)) else { return };
        let kind = radio_chip::tail(self.tx.tone[self.tx.slot as usize % 4], self.tx.phase);
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
        let Some(sel) = self.path.keyed() else { return };
        let n = sel.chip();
        if !self.has(n) {
            return;
        }
        let heard = dtmf.is_none() || self.tx.sidetone; // DTMF-8's setting; the burst is always heard
        let (sending, mic_ok) = radio_chip::keyed_tone(&mut self.chip(n), hz, dtmf, heard);
        if !mic_ok {
            log(b"tx mic read-back failed, chip ", n as u32);
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
        self.live.tx = TxLive { keyed: false, slot, mic: 0, refused: Some(why), dtmf_sent: self.live.tx.dtmf_sent, voice: self.live.tx.voice };
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
            // Decision 95: stock's RX end while keyed (the mic back), and the repeat audio again on a return.
            Some(XbCmd::Drop) => {
                let mut st = Steps::default();
                self.path.xb_drop(&mut st);
                self.run(&st).await;
            }
            Some(XbCmd::Reopen) => {
                let mut st = Steps::default();
                self.path.xb_reopen(&mut st);
                self.run(&st).await;
            }
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
    /// unkeys first), the CW ID when the repeater asks for it (XB-12), the end tone and tail elimination as the
    /// transmitting slot's (decision 95: stock plays its ROGER and TAIL at a repeat's unkey as for any TX).
    fn xb_seq_cfg(&self, x: &XbRun, id: bool) -> txseq::Cfg {
        let to = x.rep.from().map_or(0, |f| x.sides[1 - f as usize]);
        txseq::Cfg {
            timeout_s: (x.rep.timeout_ms() / 1000 + 5).min(600) as u16,
            prealert_s: 0,
            end_tone: self.tx.end[to as usize % 4] != 0,
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
        // Decision 95: stock's TX setup clears PB8 unless the accessory detect (PC15) is low; its PTT state 1
        // sets it again with the repeat audio (`Act::RepeatAudio`).
        if crate::board::ACC_DET.read() {
            AMP_EN[0].write(false);
        }
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
        self.live.tx = TxLive { keyed: true, slot: to, mic: 0, refused: None, dtmf_sent: self.live.tx.dtmf_sent, voice: self.live.tx.voice };
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
