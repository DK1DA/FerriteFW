//! Radio: the only task that touches the BK4829s and their bus, the SI4732, the RF path pins and the PA. Runs on
//! the highest-priority interrupt executor.
//!
//! Battery Save (`ferrite_core::rxsleep`) cycles the active BK4829 between deep sleep and receive; the other chip
//! sleeps whenever it is not the one in use.
//!
//! Every path select, the PA drive and the band relay change through the path machine (`ferrite_core::path`,
//! SAFE-11): it plans, [`Radio::run`] carries the plan out to the end.
//!
//! This build receives only: the bk4829 crate's transmit methods do not exist without the `tx` feature, and no
//! transmit path pin is ever raised here (only a key-up plans one, and nothing here asks for a key-up).

use crate::board::{AMP_EN, BK_CS, BK_SCK, BK_SDA, RELAY, RF_PINS, SI4732_ADDR, SI_RST, SI_SCL, SI_SDA};
use crate::bus::{BusyDelay, Io, Out, SiBus};
use crate::hal::i2c::I2c;
use crate::chan::{DEBUG_ANS, DEBUG_REQ, EVENTS, LIVE, RADIO, TERM_WAKE};
use crate::hal::gpio::Mode;
use crate::tasks::{check, CHECK_BK0, CHECK_BK1};
use bk4829::wire::ThreeWire;
use bk4829::{AfOut, Bandwidth, Bk4829};
use embassy_futures::select::{select3, Either, Either3};
use embassy_time::{Instant, Timer};
use ferrite_core::freq::{self, Freq10};
use ferrite_core::msg::{bk_write_refused, BroadcastCmd, DebugAns, DebugReq, Event, Kind, Live, RadioCmd, Source};
use ferrite_core::path::{Act, Cause, Path, Rx, Sel, Steps};
use ferrite_core::rxsleep::{Action, Level, RxSleep};
use si4732::{Mode as SiMode, Si4732, Sideband};

type Wire = ThreeWire<Out, Io, BusyDelay>;

/// Start-up frequency until settings exist: 2 m calling.
const DEFAULT: Freq10 = freq::mhz(146) + 52_000;
/// Battery Save until settings exist.
const DEFAULT_SAVE: Level = Level::L1;
/// Signal meter and squelch poll while the receiver is awake.
const POLL_MS: u64 = 50;

/// Speaker amplifier and the shared audio stage (both roles likely, from stock).
fn speaker(on: bool) {
    for p in AMP_EN {
        p.write(on);
    }
}

struct Radio {
    si: Si4732<SiBus>,
    wire: Wire,
    cs: [Out; 2],
    chips: u8,
    live: Live,
    save: RxSleep,
    path: Path,
    /// The path machine muted the receiver (around a relay move or while keyed): the squelch doesn't open the
    /// speaker meanwhile.
    muted: bool,
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
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

    /// Carry out a path plan, waits included, before anything else reaches the pins.
    async fn run(&mut self, steps: &Steps) {
        for &a in steps.as_slice() {
            match a {
                Act::Sel(s, on) => RF_PINS[s as usize].write(on),
                Act::PaZero => RF_PINS[6].write(false),
                Act::Relay(on) => RELAY.write(on),
                Act::SiInput(_) => {} // PA7/PC5 stay low until the bench says which state is which
                Act::Mute(m) => {
                    self.muted = m;
                    speaker(!m && self.live.squelch_open);
                }
                Act::Wait(ms) => Timer::after_millis(ms as u64).await,
                // Transmit steps: only a key-up plans them, and this build never asks for one.
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

    /// The receive path for `f`, through the path machine: tuning the active slot is a deliberate cause, so it
    /// may move the relay (SAFE-20).
    async fn select_rx(&mut self, f: Freq10) {
        let mut s = Steps::default();
        let _ = self.path.rx(Rx::Bk(Sel::rx(f)), Cause::ActiveSlot, now_ms() as u32, &mut s); // refused: stays as it is
        self.run(&s).await;
    }

    async fn tune(&mut self, f: Freq10) {
        let n = freq::band(f).1;
        if self.chips & 1 << n == 0 {
            return;
        }
        // The other chip sleeps (the background receiver comes later).
        let other = n ^ 1;
        if self.chips & 1 << other != 0 {
            self.chip(other).sleep();
        }
        self.select_rx(f).await;
        let mut bk = self.chip(n);
        bk.wake();
        bk.set_freq(f * 10);
        bk.bandwidth(Bandwidth::Wide);
        bk.af_out(AfOut::Normal);
        bk.rx_on();
        self.live.freq = f;
        self.live.rx_asleep = false;
        self.save.activity(now_ms());
    }

    /// Battery Save's phase change on the active chip.
    async fn apply(&mut self, a: Action) {
        let (f, n) = (self.live.freq, freq::band(self.live.freq).1);
        if self.chips & 1 << n == 0 {
            return;
        }
        match a {
            Action::None => return,
            Action::Sleep => {
                speaker(false);
                self.live.squelch_open = false;
                self.chip(n).sleep();
                let mut s = Steps::default();
                let _ = self.path.rx_off(&mut s); // refused only while keyed (TX-29)
                self.run(&s).await;
            }
            Action::Wake => {
                let mut bk = self.chip(n);
                bk.wake();
                bk.rx_on();
                self.select_rx(f).await;
            }
        }
        self.live.rx_asleep = self.save.asleep();
    }

    /// RSSI and squelch from the active chip; true if anything changed.
    fn poll(&mut self) -> bool {
        let n = freq::band(self.live.freq).1;
        if self.chips & 1 << n == 0 {
            return false;
        }
        let mut bk = self.chip(n);
        let (rssi, open) = (bk.rssi_dbm(), bk.status().squelch_open);
        let changed = rssi != self.live.rssi || open != self.live.squelch_open;
        if open != self.live.squelch_open {
            speaker(open && !self.muted);
            // Opening and closing both count as activity: the receiver stays fully on for the hold time after.
            self.save.activity(now_ms());
        }
        self.live.rssi = rssi;
        self.live.squelch_open = open;
        changed
    }
}

/// The broadcast receiver: powered up in the mode asked for (reset pulse, power-up, crystal settling), then
/// tuned by polling. FM in 10 kHz units, AM in kHz. Which antenna path PA7/PC5 select is still a bench
/// question, so both stay low. SSB (patch from SPI flash) comes with the link's patch import.
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
        live: Live::default(),
        save: RxSleep::new(DEFAULT_SAVE, now_ms()),
        path: Path::new(),
        muted: false,
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
        }
        check(Source::Radio, if n == 0 { CHECK_BK0 } else { CHECK_BK1 }, found);
    }
    r.live.chips = r.chips;
    // The SI4732 is held in reset (PA6) until FM broadcast or HF mode powers it: not tested at boot.
    crate::tasks::untested(crate::tasks::CHECK_SI);
    r.tune(DEFAULT).await;
    let live = LIVE.sender();
    live.send(r.live);

    loop {
        // Wake for the next command, the next Battery Save phase change, or (awake) the next meter poll.
        let mut at = r.save.deadline();
        if !r.save.asleep() {
            at = at.min(now_ms() + POLL_MS);
        }
        let cmd = match select3(RADIO.receive(), Timer::at(Instant::from_millis(at)), DEBUG_REQ.receive()).await {
            Either3::First(c) => Either::First(c),
            Either3::Second(t) => Either::Second(t),
            Either3::Third(d) => {
                let a = r.debug(d);
                DEBUG_ANS.send(a).await;
                TERM_WAKE.signal(()); // the UI task runs the terminal
                continue;
            }
        };
        match cmd {
            Either::First(RadioCmd::Tune(f)) => {
                r.tune(f).await;
                live.send(r.live);
            }
            Either::First(RadioCmd::BatterySave(level)) => {
                let a = r.save.set_level(level, now_ms());
                r.apply(a).await;
                live.send(r.live);
            }
            Either::First(RadioCmd::Activity) => {
                let a = r.save.activity(now_ms());
                r.apply(a).await;
                live.send(r.live);
            }
            Either::First(RadioCmd::Squelch(_)) => {} // the advanced squelch: milestone 2
            Either::First(RadioCmd::Broadcast(b)) => {
                let _ = broadcast(&mut r.si, b).await; // errors: the UI shows no station (reporting: next)
            }
            Either::First(RadioCmd::TuneSlot { slot: 0, freq }) => {
                r.tune(freq).await; // one receiver for now: slot A is the one the chips follow
                live.send(r.live);
            }
            // Three slots, watch, scan, the slot settings and the exclusive modes (scope, tone search, counter, HF
            // mode: milestone 2e, chapter 20a; crossband): milestone 2 and later (manual chapters 6, 13, 14, 19-22).
            Either::First(
                RadioCmd::TuneSlot { .. }
                | RadioCmd::SlotParam { .. }
                | RadioCmd::Active(_)
                | RadioCmd::Watch(_)
                | RadioCmd::Scan { .. }
                | RadioCmd::PauseWatch(_)
                | RadioCmd::ScanSkip { .. }
                | RadioCmd::ScanZones { .. }
                | RadioCmd::ScanDir { .. }
                | RadioCmd::Monitor { .. }
                | RadioCmd::ToneBurst { .. }
                | RadioCmd::Dtmf { .. }
                | RadioCmd::Sweep { .. }
                | RadioCmd::ScopeCfg { .. }
                | RadioCmd::ScopeListen { .. }
                | RadioCmd::ToneSearch { .. }
                | RadioCmd::Counter(_)
                | RadioCmd::Hf { .. }
                | RadioCmd::Crossband(_),
            ) => {}
            // Transmit code is out of the build until milestone 4: the UI refuses first (RX-only build).
            Either::First(RadioCmd::Transmit { .. }) => {}
            Either::First(RadioCmd::Shutdown) => {
                for n in 0..2u8 {
                    if r.chips & 1 << n != 0 {
                        r.chip(n).sleep();
                    }
                }
                let _ = broadcast(&mut r.si, BroadcastCmd::Off).await;
                let mut s = Steps::default();
                r.path.safe(now_ms() as u32, &mut s);
                r.run(&s).await;
                speaker(false);
            }
            Either::Second(()) => {
                let mut changed = !r.save.asleep() && r.poll();
                // Busy: the squelch is open (decoders, scan and transmit join this as they arrive).
                let a = r.save.tick(now_ms(), r.live.squelch_open);
                changed |= a != Action::None;
                r.apply(a).await;
                if changed {
                    live.send(r.live);
                }
            }
        }
    }
}
