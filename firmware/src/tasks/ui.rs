//! UI: the only task that draws. The boot screen with the self-test as the other tasks report it, then the app
//! (`ferrite-app`, the same state machine the simulator runs): keys and the knob in, the Radio task's snapshot
//! in, its commands out to the Radio task, and the screen redrawn where it changed.

use crate::board::BACKLIGHT;
use crate::chan::{Input, ALERTS, INPUT, LIVE, RADIO};
use crate::display::Screen;
use crate::hal::gpio::Mode;
use crate::store;
use crate::tasks::{check, CHECK_MCU, CHECK_STATE, FAILED, PASSED};
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{select4, Either4};
use embassy_time::{Duration, Instant, Timer};
use ferrite_app::App;
use ferrite_core::chipid;
use ferrite_core::keys;
use ferrite_core::msg::{Device, Live, LiveAll, SlotLive, Source};
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State};
use ferrite_ui::gfx::BG;

/// How often the app is ticked: often while a key is held (long presses fire at 700 ms, repeats every 100 ms:
/// KEY-3, KEY-4), rarely otherwise (toasts, undo, timeouts, the clock), so the MCU sleeps.
const TICK_HELD_MS: u64 = 20;
const TICK_IDLE_MS: u64 = 250;

/// The milliseconds the app counts in (wrapping).
fn now_ms() -> u32 {
    Instant::now().as_millis() as u32
}

/// The single receiver's snapshot as slot A's (the Radio task follows slot A until it runs three slots).
fn slot_a(all: &mut LiveAll, l: &Live) {
    all.slots[0] = SlotLive { freq: l.freq, rssi: l.rssi, squelch_open: l.squelch_open, ..all.slots[0] };
    all.rx_asleep = l.rx_asleep;
    all.chips = l.chips;
}

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

    // Hand-off: the whole screen cleared, then the app draws everything once (HOME-2).
    lcd.0.fill(0, 0, 240, 320, BG);
    let store = store::channels();
    let mut app = App::new(store::settings(), store::last_alert());
    // After the self-test: setup on first boot (BOOT-12), else the PIN if one is set (BOOT-13).
    if store::first_boot() {
        app.first_boot();
    } else {
        app.ask_pin();
    }
    app.redraw_all();
    // Radio-wide state for the status bar and Status page; GPS, battery and the link fill it in later.
    let dev = Device { tx_built: cfg!(feature = "tx"), battery_pct: 100, ..Default::default() };
    let mut live_rx = LIVE.receiver().unwrap();
    let mut live = LiveAll::default();
    if let Some(l) = live_rx.try_get() {
        slot_a(&mut live, &l);
    }
    // Key codes down (mapped ones only): the app is ticked often while any is held.
    let mut down = 0u32;
    loop {
        while let Some(c) = app.pop_cmd() {
            RADIO.send(c).await; // the Radio task ignores what it doesn't run yet
        }
        // Channel writes (save as channel, the channel editor): dropped until the Storage task keeps channels
        // in SPI flash; the table in MCU flash can't change.
        while app.pop_write().is_some() {}
        app.render(&mut lcd, &live, &dev, &store);
        let period = if down != 0 { TICK_HELD_MS } else { TICK_IDLE_MS };
        match select4(live_rx.changed(), INPUT.receive(), Timer::after(Duration::from_millis(period)), ALERTS.receive()).await {
            Either4::First(l) => slot_a(&mut live, &l),
            // WX-4: a decoded SAME header opens the alert over any screen.
            Either4::Fourth(a) => {
                app.same_alert(&a.header, a.slot, a.freq);
            }
            Either4::Second(Input::Key(keypad::Event::Press(code))) => {
                app.raw_key(code, true); // every code, mapped or not: the key test names them (KEY-2)
                if let Some(k) = keys::key(code) {
                    down |= 1 << code;
                    app.key_down(k, now_ms(), &store);
                }
            }
            Either4::Second(Input::Key(keypad::Event::Release(code))) => {
                app.raw_key(code, false);
                if let Some(k) = keys::key(code) {
                    down &= !(1 << code);
                    app.key_up(k, now_ms(), &store);
                }
            }
            Either4::Second(Input::Key(_)) => {} // the sampler's own long press and repeat are off
            Either4::Second(Input::Knob(d)) => app.knob(d, now_ms(), &store),
            Either4::Third(()) => {}
        }
        app.tick(now_ms(), &live, &store);
    }
}
