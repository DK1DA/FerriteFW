//! UI: the only task that draws. The boot screen with the self-test as the other tasks report it, then home.
//! The knob tunes in 12.5 kHz steps (a placeholder for VFO mode with its step setting).

use crate::board::BACKLIGHT;
use crate::chan::{Input, INPUT, LIVE, RADIO};
use crate::display::Screen;
use crate::hal::gpio::Mode;
use crate::tasks::{check, CHECK_MCU, CHECK_STATE, FAILED, PASSED};
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use ferrite_core::chipid;
use ferrite_core::msg::{Live, RadioCmd, Source};
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State};
use ferrite_ui::gfx::BG;
use ferrite_ui::home;

const STEP: u32 = 1_250; // 12.5 kHz in 10 Hz units

#[embassy_executor::task]
pub async fn task() {
    let mut lcd = Screen::new();
    // Start-up as awaited steps (up to 150 ms each), then black, then the panel on.
    for &s in st7789::STARTUP {
        if let Some(ms) = lcd.0.step(s) {
            Timer::after_millis(ms as u64).await;
        }
    }
    lcd.0.fill(0, 0, 240, 320, BG);
    lcd.0.on(true);
    for p in BACKLIGHT {
        p.output(true, Mode::OutPp);
    }

    // The MCU check is ours to make: transmit (in the tx build) needs the expected part; receive runs regardless.
    check(Source::Ui, CHECK_MCU, crate::hal::sys::idcode() == chipid::EXPECTED_PID);
    let mut checks = [
        Check { label: "MCU", value: "", state: State::Pending },
        Check { label: "SPI flash", value: "", state: State::Pending },
        Check { label: "BK4829 VHF", value: "", state: State::Pending },
        Check { label: "BK4829 UHF", value: "", state: State::Pending },
    ];
    // The logo for at least 1.5 s, until every check has reported (at most 5 s: a check still pending then
    // counts as failed).
    let start = Instant::now();
    loop {
        for (c, s) in checks.iter_mut().zip(&CHECK_STATE) {
            c.state = match s.load(Relaxed) {
                PASSED => State::Ok,
                FAILED => State::Fail,
                _ => State::Pending,
            };
        }
        boot::draw(&mut lcd, &BootInfo { version: VERSION, callsign: None, checks: &checks });
        let done = checks.iter().all(|c| c.state != State::Pending);
        let t = start.elapsed();
        if (done && t >= Duration::from_millis(1500)) || t >= Duration::from_secs(5) {
            break;
        }
        Timer::after_millis(100).await;
    }
    for c in checks.iter_mut().filter(|c| c.state == State::Pending) {
        c.state = State::Fail;
    }
    if checks.iter().any(|c| c.state == State::Fail) {
        // The failed check stays on screen until a key.
        boot::draw(&mut lcd, &BootInfo { version: VERSION, callsign: None, checks: &checks });
        while !matches!(INPUT.receive().await, Input::Key(keypad::Event::Press(_))) {}
    }

    let mut live_rx = LIVE.receiver().unwrap();
    let mut live: Live = live_rx.try_get().unwrap_or_default();
    home::draw(&mut lcd, &live);
    loop {
        match select(live_rx.changed(), INPUT.receive()).await {
            Either::First(l) => {
                live = l;
                home::update(&mut lcd, &live);
            }
            Either::Second(Input::Knob(d)) => {
                let f = (live.freq as i32 + d as i32 * STEP as i32) as u32;
                RADIO.send(RadioCmd::Tune(f)).await;
            }
            Either::Second(Input::Key(_)) => {
                // Menus: milestone 2. A key still wakes the receiver, as on stock.
                RADIO.send(RadioCmd::Activity).await;
            }
        }
    }
}
