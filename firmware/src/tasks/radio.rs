//! Radio: the only task that touches the BK4829s and their bus, the SI4732, the RF path pins and the PA. Runs on
//! the highest-priority interrupt executor.
//!
//! Three slots on two chips (manual chapters 6, 13, 14): the host-tested engines decide, this task carries out.
//! - [`Watch`] names the slot each chip listens to (WATCH-2), whose audio is heard (WATCH-3) and when watch is
//!   paused or cancelled (WATCH-6, WATCH-11, WATCH-16). The active slot never changes here (WATCH-5).
//! - [`Hop`] moves the one receive path select between the two chips while both have a slot, as stock's dual
//!   watch hops: a chip without its band's front end is taken as hearing nothing (to confirm on the bench).
//! - [`Scan`] walks a slot's band (a range at its step, the fast pass of SCAN-21) while the slot has a chip;
//!   channel, zone and preset lists need the UI's lists handed over (not yet: the band scan meanwhile, as the
//!   simulators' fake radio scans).
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

use crate::board::{AMP_EN, BK_CS, BK_SCK, BK_SDA, RELAY, RF_PINS, SI4732_ADDR, SI_IN, SI_RST, SI_SCL, SI_SDA};
use crate::bus::{BusyDelay, Io, Out, SiBus};
use crate::chan::{DEBUG_ANS, DEBUG_REQ, EVENTS, LIVE, RADIO, TERM_WAKE};
use crate::hal::gpio::Mode;
use crate::hal::i2c::I2c;
use crate::tasks::{check, CHECK_BK0, CHECK_BK1};
use bk4829::wire::ThreeWire;
use bk4829::{AfOut, Bandwidth, Bk4829};
use embassy_futures::select::{select3, Either3};
use embassy_time::{Instant, Timer};
use ferrite_core::freq::{self, mhz, Freq10};
use ferrite_core::msg::{bk_write_refused, BroadcastCmd, DebugAns, DebugReq, Event, Kind, LiveAll, ModeLive, Param, RadioCmd, Source, SLOTS};
use ferrite_core::path::{Act, Cause, Path, Rx, Sel, Steps};
use ferrite_core::rxsleep::{Action, Level, RxSleep};
use ferrite_core::scan::{self, Check, Range, Resume, Scan};
use ferrite_core::watch::{self, Hop, Watch};
use si4732::{Mode as SiMode, Si4732, Sideband};
#[cfg(feature = "tx")]
use ferrite_core::{
    msg::TxLive,
    txcal::{Gauge, TxCal},
    txplan::{Refusal, Region},
    txseq::{self, Kind as TxKind, Req, TxSeq},
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
/// A scan's resume until the UI hands over the scan settings (0 1 5, SCAN-24): the engine's default.
const SCAN_RESUME: Resume = Resume::Time(5);
/// The HF mode's internal slot index (the chips' `tuned` entry for it).
const HF_SLOT: u8 = 3;
/// The sequencer's settings in the `tx` build: the timeout a little after the UI's own 120 s (the UI unkeys
/// first; this is the backstop), no pre-alert (the UI shows it). End tone, CW ID and tail elimination wait for
/// the tone generator (TONE-8, VOX-12, TONE-9): not sent yet.
#[cfg(feature = "tx")]
const TX_CFG: txseq::Cfg = txseq::Cfg { timeout_s: 125, prealert_s: 0, end_tone: false, cw_id: false, tail: false };
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
}

impl SlotRx {
    const DEFAULT: SlotRx = SlotRx { freq: DEFAULT, narrow: false, rx_tone: 0, clar: 0, sq: 3, am: false };

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
    #[cfg(feature = "tx")]
    tx: Tx,
}

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
    /// TX-42: every PTT input has read released since boot.
    ptt_ok: bool,
    /// The calibrated PA drive for the key-up being run (`Act::PaRamp`).
    pa: u8,
    /// The read-back failed during a key-up (TX-7): the plan stops and the safe state follows.
    fault: bool,
    adc: bool,
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
                // End tone, CW ID, tail elimination: TX_CFG never asks for them yet.
                #[cfg(feature = "tx")]
                Act::EndTone | Act::CwId | Act::TailElim => {}
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
        let slot = if s < SLOTS as u8 { self.slots[s as usize] } else { SlotRx { freq: f, ..SlotRx::DEFAULT } };
        let mut bk = self.chip(n);
        bk.wake();
        bk.set_freq(f * 10);
        bk.bandwidth(if slot.narrow { Bandwidth::Narrow } else { Bandwidth::Wide });
        match slot.rx_tone {
            0 => bk.subtone_off(),
            t if t & 0x8000 != 0 => bk.dcs_rx(t & 0x3FFF, t & 0x4000 != 0),
            t => bk.ctcss_rx(t),
        }
        bk.af_out(AfOut::Mute); // the audio decision unmutes the one heard
        bk.rx_on();
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
        set.count_ones() > 1 || self.scans.iter().any(|s| s.running()) || self.path.keyed().is_some()
    }

    /// The engines' tick: each listening chip's squelch, the watch turn, the scans, the chips and the path, and
    /// the audio.
    async fn tick(&mut self) {
        if self.hf.is_some() {
            self.hf_poll();
            return;
        }
        if self.save.asleep() {
            return;
        }
        let now = now_ms() as u32;
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
            toned[n as usize] = open && (self.slots[s as usize].rx_tone == 0 || st.ctcss || st.cdcss);
            let sl = &mut self.live.slots[s as usize];
            sl.rssi = rssi;
            scan_open[s as usize] = match self.scans[s as usize].check() {
                Check::Rssi => rssi > *fl + FAST_MARGIN_DB,
                Check::Squelch => open,
            };
        }
        // 2. The watch turn.
        let cfg = watch::Cfg { set: self.watch_set, active: self.active, freq: self.slots.map(|s| s.freq), muted: 0 };
        let keyed = self.path.keyed().is_some();
        let out = self.watcher.tick(now, &cfg, sq, keyed);
        // 3. Scans step while their slot has a chip, on the answer from that chip.
        for i in 0..SLOTS {
            if !self.scans[i].running() || !self.tuned.iter().any(|t| t.is_some_and(|(s, _)| s as usize == i)) {
                continue;
            }
            let o = self.scans[i].tick(&self.ranges[i], now, scan_open[i]);
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
            #[cfg(not(feature = "tx"))]
            Param::TxTone | Param::Power => return,
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
            (sc.fast, sc.resume) = (true, SCAN_RESUME);
            sc.start(scan::range_pos(&r, self.slots[i].freq), now_ms() as u32);
            log(b"scan start slot ", i as u32);
        } else {
            self.scans[i].stop();
        }
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
        }
    }
}

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
        tuned: [None; 2],
        floor: [-125; 2],
        monitor: 0,
        scan_tx: false,
        tone_level: 1,
        hf: None,
        #[cfg(feature = "tx")]
        tx: Tx { seq: TxSeq::new(), cal: None, gauge: Gauge::default(), tone: [0; 4], power: [0; 4], plan: None, ptt_ok: false, pa: 0, fault: false, adc: false },
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
        match c {
            RadioCmd::TuneSlot { slot, freq } => {
                let i = slot as usize % SLOTS;
                (self.slots[i].freq, self.slots[i].clar) = (freq, 0); // VFO-22: a retune clears the clarifier
                self.scans[i].stop();
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
                let o = self.scans[i].skip_once(&self.ranges[i], now);
                self.scan_out(i, o);
            }
            RadioCmd::ScanDir { slot, up, .. } => {
                let i = slot as usize % SLOTS;
                let o = self.scans[i].dir(up, &self.ranges[i], now);
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
            // Not on the radio yet, each waiting on something named in the manual's Status lines:
            // - the zone set (SCAN-3) and channel lists: the UI's lists handed to the Radio task;
            // - the frequency counter (TSRCH-10): the BK4829's frequency-scan registers in the bk4829 crate;
            // - the beep (SND-11), tone burst, live DTMF: the tone generator, with the `tx` build;
            // - band scope, tone search, crossband (chapters 21, 22, 19): their own exclusive modes, after 2d.
            RadioCmd::ScanZones { .. }
            | RadioCmd::Counter(_)
            | RadioCmd::Beep { .. }
            | RadioCmd::ToneBurst { .. }
            | RadioCmd::Dtmf { .. }
            | RadioCmd::Sweep { .. }
            | RadioCmd::ScopeCfg { .. }
            | RadioCmd::ScopeListen { .. }
            | RadioCmd::ToneSearch { .. }
            | RadioCmd::Crossband(_) => {}
            #[cfg(feature = "tx")]
            RadioCmd::TxPlan { tx, region, extend } => {
                self.tx.plan = Some((tx, Region::ALL.get(region as usize).copied().unwrap_or(Region::Other), extend));
            }
            #[cfg(feature = "tx")]
            RadioCmd::Transmit { slot, on } => self.transmit(slot, on).await,
            // Transmit code is out of this build: the UI refuses first (RX-only build).
            #[cfg(not(feature = "tx"))]
            RadioCmd::Transmit { .. } | RadioCmd::TxPlan { .. } => {}
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
        let high = self.tx.power[self.live.tx.slot as usize % 4] == 2;
        let mut bk = self.chip(n);
        bk.tx_on(bk4829::tx::Source::Mic);
        if bk.read(0x30) != REG30_TX_MIC {
            return false;
        }
        bk.write(0x36, if high { 0x7F } else { 0x57 });
        true
    }

    /// Key up or unkey a slot (3 = HF mode's frequency).
    async fn transmit(&mut self, slot: u8, on: bool) {
        let now = now_ms() as u32;
        let mut st = Steps::default();
        if !on {
            self.tx.seq.release(&TX_CFG, &mut self.path, now, &mut st);
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
        let req = Req {
            tx: f,
            rx,
            kind: TxKind::Voice,
            person: true,
            confirmed: true, // the UI asked for the emergency prompt's second PTT before this (TX-3)
            am: slot < SLOTS as u8 && self.slots[s].am,
            region,
            extend,
            cal_ok: self.tx.cal.is_some(),
            batt_ok: self.battery_ok(),
            power: self.tx.power[s],
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
        self.live.tx = TxLive { keyed: false, slot, mic: 0, refused: None };
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
    }

    /// While keyed: the sequencer's own unkeys (timeout backstop, duty limit, battery under load).
    async fn tx_tick(&mut self, now: u32) {
        let batt = self.battery_ok();
        let mut st = Steps::default();
        if let Some(e) = self.tx.seq.tick(&TX_CFG, batt, &mut self.path, now, &mut st) {
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

    /// After any unkey: the chips back to their slots on the next tick, the UI told.
    fn unkeyed(&mut self) {
        self.tuned = [None; 2];
        self.live.tx.keyed = false;
        log(b"tx unkey", 0);
    }

    fn refuse(&mut self, slot: u8, why: Refusal) {
        self.live.tx = TxLive { keyed: false, slot, mic: 0, refused: Some(why) };
        log(b"tx refused ", why as u32);
    }
}
