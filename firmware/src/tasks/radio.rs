//! Radio: the only task that touches the BK4829s and their bus, the SI4732, the RF path pins and the PA. Runs on
//! the highest-priority interrupt executor.
//!
//! Battery Save (`ferrite_core::rxsleep`) cycles the active BK4829 between deep sleep and receive; the other chip
//! sleeps whenever it is not the one in use.
//!
//! This build receives only: the bk4829 crate's transmit methods do not exist without the `tx` feature, and no
//! transmit path pin is ever raised here.

use crate::board::{AMP_EN, BK_CS, BK_SCK, BK_SDA, RF_PINS, SI4732_ADDR, SI_RST, SI_SCL, SI_SDA};
use crate::bus::{BusyDelay, Io, Out, SiBus};
use crate::hal::i2c::I2c;
use crate::chan::{LIVE, RADIO};
use crate::hal::gpio::Mode;
use crate::tasks::{check, CHECK_BK0, CHECK_BK1};
use bk4829::wire::ThreeWire;
use bk4829::{AfOut, Bandwidth, Bk4829};
use embassy_futures::select::{select, Either};
use embassy_time::{Instant, Timer};
use ferrite_core::freq::{self, Band, Freq10};
use ferrite_core::msg::{BroadcastCmd, Live, RadioCmd, Source};
use ferrite_core::rxsleep::{Action, Level, RxSleep};
use si4732::{Mode as SiMode, Si4732, Sideband};

type Wire = ThreeWire<Out, Io, BusyDelay>;

/// Start-up frequency until settings exist: 2 m calling.
const DEFAULT: Freq10 = freq::mhz(146) + 52_000;
/// Battery Save until settings exist.
const DEFAULT_SAVE: Level = Level::L1;
/// Signal meter and squelch poll while the receiver is awake.
const POLL_MS: u64 = 50;

/// Receive path for a band: RF_PINS index (0 VHF RX, 2 UHF RX, 4 HF RX). The 200-300 MHz band's path is not
/// known yet; it uses the VHF front end, as chip 0 does.
fn rx_path(band: Band) -> usize {
    match band {
        Band::Hf => 4,
        Band::Vhf | Band::B200 => 0,
        Band::B300 | Band::Uhf => 2,
    }
}

/// Break before make: every path pin (and the PA drive) low, then the one receive path.
fn select_rx_path(band: Band) {
    for p in RF_PINS {
        p.write(false);
    }
    RF_PINS[rx_path(band)].write(true);
}

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
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

impl Radio {
    fn chip(&mut self, n: u8) -> Bk4829<bk4829::wire::Chip<'_, Out, Io, BusyDelay, Out>> {
        Bk4829::new(self.wire.chip(&mut self.cs[n as usize]))
    }

    fn tune(&mut self, f: Freq10) {
        let (band, n) = freq::band(f);
        if self.chips & 1 << n == 0 {
            return;
        }
        // The other chip sleeps (the background receiver comes later).
        let other = n ^ 1;
        if self.chips & 1 << other != 0 {
            self.chip(other).sleep();
        }
        select_rx_path(band);
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
    fn apply(&mut self, a: Action) {
        let (band, n) = freq::band(self.live.freq);
        if self.chips & 1 << n == 0 {
            return;
        }
        match a {
            Action::None => return,
            Action::Sleep => {
                speaker(false);
                self.live.squelch_open = false;
                self.chip(n).sleep();
                for p in RF_PINS {
                    p.write(false);
                }
            }
            Action::Wake => {
                let mut bk = self.chip(n);
                bk.wake();
                bk.rx_on();
                select_rx_path(band);
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
            speaker(open);
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
    };
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
    r.tune(DEFAULT);
    let live = LIVE.sender();
    live.send(r.live);

    loop {
        // Wake for the next command, the next Battery Save phase change, or (awake) the next meter poll.
        let mut at = r.save.deadline();
        if !r.save.asleep() {
            at = at.min(now_ms() + POLL_MS);
        }
        match select(RADIO.receive(), Timer::at(Instant::from_millis(at))).await {
            Either::First(RadioCmd::Tune(f)) => {
                r.tune(f);
                live.send(r.live);
            }
            Either::First(RadioCmd::BatterySave(level)) => {
                let a = r.save.set_level(level, now_ms());
                r.apply(a);
                live.send(r.live);
            }
            Either::First(RadioCmd::Activity) => {
                let a = r.save.activity(now_ms());
                r.apply(a);
                live.send(r.live);
            }
            Either::First(RadioCmd::Squelch(_)) => {} // the advanced squelch: milestone 2
            Either::First(RadioCmd::Broadcast(b)) => {
                let _ = broadcast(&mut r.si, b).await; // errors: the UI shows no station (reporting: next)
            }
            Either::First(RadioCmd::TuneSlot { slot: 0, freq }) => {
                r.tune(freq); // one receiver for now: slot A is the one the chips follow
                live.send(r.live);
            }
            // Three slots, watch, scan, the slot settings and the exclusive modes (scope, tone search, counter, HF,
            // crossband): milestone 2 and later (manual chapters 6, 13, 14, 19-22).
            Either::First(
                RadioCmd::TuneSlot { .. }
                | RadioCmd::SlotParam { .. }
                | RadioCmd::Active(_)
                | RadioCmd::Watch(_)
                | RadioCmd::Scan { .. }
                | RadioCmd::PauseWatch(_)
                | RadioCmd::ScanSkip { .. }
                | RadioCmd::ScanZones { .. }
                | RadioCmd::Sweep { .. }
                | RadioCmd::ToneSearch { .. }
                | RadioCmd::Counter(_)
                | RadioCmd::Hf { .. }
                | RadioCmd::Crossband(_),
            ) => {}
            // Transmit code is out of the build until milestone 4: the UI refuses first (RX-only build).
            Either::First(RadioCmd::Transmit { .. }) => {}
            // HF mode: milestone 2e (manual chapter 20a).
            Either::First(RadioCmd::HfMode(_) | RadioCmd::HfTune { .. }) => {}
            Either::First(RadioCmd::Shutdown) => {
                for n in 0..2u8 {
                    if r.chips & 1 << n != 0 {
                        r.chip(n).sleep();
                    }
                }
                let _ = broadcast(&mut r.si, BroadcastCmd::Off).await;
                speaker(false);
                for p in RF_PINS {
                    p.write(false);
                }
            }
            Either::Second(()) => {
                let mut changed = !r.save.asleep() && r.poll();
                // Busy: the squelch is open (decoders, scan and transmit join this as they arrive).
                let a = r.save.tick(now_ms(), r.live.squelch_open);
                changed |= a != Action::None;
                r.apply(a);
                if changed {
                    live.send(r.live);
                }
            }
        }
    }
}
