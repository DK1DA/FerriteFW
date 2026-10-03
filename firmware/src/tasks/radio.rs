//! Radio: the only task that touches the BK4829s and their bus, the SI4732, the RF path pins and the PA. Runs on
//! the highest-priority interrupt executor.
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
use embassy_time::Timer;
use ferrite_core::freq::{self, Band, Freq10};
use ferrite_core::msg::{BroadcastCmd, Live, RadioCmd, Source};
use si4732::{Mode as SiMode, Si4732, Sideband};

type Wire = ThreeWire<Out, Io, BusyDelay>;

/// Start-up frequency until settings exist: 2 m calling.
const DEFAULT: Freq10 = freq::mhz(146) + 52_000;

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
        // The other chip stops receiving (the background receiver comes later).
        let other = n ^ 1;
        if self.chips & 1 << other != 0 {
            self.chip(other).off();
        }
        select_rx_path(band);
        let mut bk = self.chip(n);
        bk.set_freq(f * 10);
        bk.bandwidth(Bandwidth::Wide);
        bk.af_out(AfOut::Normal);
        bk.rx_on();
        self.live.freq = f;
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
        match select(RADIO.receive(), Timer::after_millis(50)).await {
            Either::First(RadioCmd::Tune(f)) => {
                r.tune(f);
                live.send(r.live);
            }
            Either::First(RadioCmd::Squelch(_)) => {} // the advanced squelch: milestone 2
            Either::First(RadioCmd::Broadcast(b)) => {
                let _ = broadcast(&mut r.si, b).await; // errors: the UI shows no station (reporting: next)
            }
            Either::First(RadioCmd::Shutdown) => {
                for n in 0..2u8 {
                    if r.chips & 1 << n != 0 {
                        r.chip(n).off();
                    }
                }
                let _ = broadcast(&mut r.si, BroadcastCmd::Off).await;
                speaker(false);
                for p in RF_PINS {
                    p.write(false);
                }
            }
            Either::Second(()) => {
                if r.poll() {
                    live.send(r.live);
                }
            }
        }
    }
}
