//! UI: the only task that draws. The boot screen with the self-test as the other tasks report it, then the app
//! (`ferrite-app`, the same state machine the simulator runs): keys and the knob in, the Radio task's snapshot
//! in, its commands out to the Radio task, and the screen redrawn where it changed.

use crate::board::{BACKLIGHT, KEYPAD_LIGHT, LED_ACTIVE_HIGH, LED_GREEN, LED_RED, POWER_HOLD, POWER_SW, SCREEN_LIGHT};
use crate::bringup;
use crate::chan::{Input, ALERTS, CABLE_BAUD, DEBUG_ANS, DEBUG_REQ, INPUT, LINK_BAUD, LINK_HELLOS, LINK_ON, LIVE, RADIO, TERM_IN, TERM_OUT, TERM_WAKE};
use crate::display::Screen;
use crate::hal::gpio::Mode;
use crate::store;
use crate::tasks::{check, CHECK_CAL, CHECK_FLASH, CHECK_MCU, CHECK_STATE, FAILED, PASSED, UNTESTED};
use core::sync::atomic::Ordering::Relaxed;
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_time::{Duration, Instant, Timer};
use ferrite_app::term::{Sink, Term};
use ferrite_app::App;
use ferrite_core::chipid;
use ferrite_core::keys;
use ferrite_core::msg::{Device, Live, LiveAll, RadioCmd, SlotLive, Source};
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State};
use ferrite_ui::gfx::BG;

/// How often the app is ticked: often while a key is held (long presses fire at 700 ms, repeats every 100 ms:
/// KEY-3, KEY-4), rarely otherwise (toasts, undo, timeouts, the clock), so the MCU sleeps.
const TICK_HELD_MS: u64 = 20;
const TICK_IDLE_MS: u64 = 250;

/// The terminal's output: the cable's transmit ring, emptied by the Link task.
struct Cable;

impl Sink for Cable {
    fn room(&self) -> usize {
        TERM_OUT.room() as usize
    }
    fn put(&mut self, b: &[u8]) {
        for &c in b {
            TERM_OUT.put(c);
        }
    }
}

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

/// MENU pressed or released: whether it is held, for the settings reset at power on (BOOT-14).
fn menu_state(i: &Input, held: &mut bool) {
    if let Input::Key(keypad::Event::Press(c) | keypad::Event::Release(c)) = *i {
        if keys::key(c) == Some(ferrite_core::keys::Key::Menu) {
            *held = matches!(i, Input::Key(keypad::Event::Press(_)));
        }
    }
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
        Check { label: "SI4732", value: "not tested", state: State::Pending },
        Check { label: "Calibration", value: "", state: State::Pending },
    ];
    // The store: mounted before the boot screen, as the settings (callsign, owner card: BOOT-9, MENU 0 7 3 3)
    // are in it. A first boot formats the chip here (about a second, once). Its writes go to the Storage task.
    // The bring-up build never mounts it: nothing on the chip is taken as ours or written, and the settings
    // are the defaults, in RAM for this power-on.
    let (flash, store) = store::make();
    if !cfg!(feature = "bringup") {
        store.mount_async().await;
    }
    check(Source::Storage, CHECK_FLASH, flash.is_some());
    // Calibration present: the factory sector isn't blank (read only, CAL-1). Its CRC check comes with 1b.
    let mut cal = [0u8; 32];
    let cal_ok = flash.is_some()
        && ferrite_store::Nor::read(&mut *store.nor(), ferrite_core::layout::CALIBRATION.start, &mut cal).is_ok()
        && cal.iter().any(|&b| b != 0xFF)
        && cal.iter().any(|&b| b != 0);
    check(Source::Storage, CHECK_CAL, cal_ok);
    let set = store::settings();
    store.load_settings(set);
    let card = set.boot_owner().is_some();
    // The logo for at least 1.5 s (the owner card 4 s; any key skips it), until every check has reported (at
    // most 5 s: a check still pending then counts as failed).
    let least = Duration::from_millis(if card { 4000 } else { 1500 });
    let (mut menu_held, mut skip) = (false, false);
    // Bring-up: a key pressed during the boot screen opens the key test (the key map isn't trusted yet).
    let mut key_seen = false;
    ferrite_ui::theme::set(set.theme); // the boot screen in the saved theme (0 7 1)
    let start = Instant::now();
    loop {
        for (c, s) in checks.iter_mut().zip(&CHECK_STATE) {
            c.state = match s.load(Relaxed) {
                PASSED => State::Ok,
                FAILED => State::Fail,
                UNTESTED => State::Off,
                _ => State::Pending,
            };
        }
        boot::draw(&mut lcd, &BootInfo { version: VERSION, callsign: set.boot_callsign(), line: set.boot_line(), owner: set.boot_owner(), checks: &checks });
        let done = checks.iter().all(|c| c.state != State::Pending);
        let t = start.elapsed();
        if (done && (t >= least || skip)) || t >= Duration::from_secs(5) {
            break;
        }
        if card || cfg!(feature = "bringup") {
            // Keys while the card shows: any press skips the rest of it; MENU's state still counts (BOOT-14).
            if let Either::First(i) = select(INPUT.receive(), Timer::after_millis(100)).await {
                skip |= card && matches!(i, Input::Key(keypad::Event::Press(_)));
                key_seen |= matches!(i, Input::Key(keypad::Event::Press(_)));
                log_input(&i);
                menu_state(&i, &mut menu_held);
            }
        } else {
            Timer::after_millis(100).await;
        }
    }
    for c in checks.iter_mut().filter(|c| c.state == State::Pending) {
        c.state = State::Fail;
    }
    if checks.iter().any(|c| c.state == State::Fail) {
        // The failed check stays on screen until a key.
        boot::draw(&mut lcd, &BootInfo { version: VERSION, callsign: set.boot_callsign(), line: set.boot_line(), owner: None, checks: &checks });
        while !matches!(INPUT.receive().await, Input::Key(keypad::Event::Press(_))) {}
    }

    // Keys pressed during the boot screen: only whether MENU is still held counts (BOOT-14); the rest are
    // dropped, as a PTT held at power on is (KEY-8).
    while let Ok(i) = INPUT.try_receive() {
        key_seen |= matches!(i, Input::Key(keypad::Event::Press(_)));
        log_input(&i);
        menu_state(&i, &mut menu_held);
    }
    boot_report(flash, &checks, key_seen);

    // Hand-off: the whole screen cleared, then the app draws everything once (HOME-2).
    lcd.0.fill(0, 0, 240, 320, BG);
    let mut app = App::new(set, store::last_alert());
    // The slots as they were saved (frequencies, VFO settings, the channel each is on, watch, Battery Save).
    store.restore(&mut app);
    // After the self-test: setup on first boot (BOOT-12); MENU held: the settings reset (PIN-6, BOOT-14), which
    // asks before the PIN; else the PIN if one is set (BOOT-13).
    if cfg!(feature = "bringup") {
        // No setup, reset or PIN: nothing is stored. A key at boot: straight to the key test (hold BACK leaves).
        if key_seen {
            app.open(ferrite_app::screens::nav::Dest::KeyTest);
        }
        app.notice(b"Bring-up build", b"nothing is saved");
    } else if store.first_boot() {
        app.first_boot();
    } else if menu_held {
        app.power_on_reset();
    } else {
        app.ask_pin();
    }
    app.redraw_all();
    // Radio-wide state for the status bar and Status page; GPS, battery and the link fill it in later.
    let mut dev = Device { tx_built: cfg!(feature = "tx"), battery_pct: 100, ..Default::default() };
    let mut live_rx = LIVE.receiver().unwrap();
    let mut live = LiveAll::default();
    if let Some(l) = live_rx.try_get() {
        slot_a(&mut live, &l);
    }
    // Key codes down (mapped ones only): the app is ticked often while any is held.
    let mut down = 0u32;
    // The terminal over the cable (chapter 37): it reads and edits the same App, so it runs here.
    let mut term = Term::new();
    // The link protocol on the same cable (chapter 29), trusted as the cable is (LINK-7); TERM-2 routes the bytes.
    let mut link = ferrite_app::link::Session::new(true);
    let fastest = ferrite_core::link::RATES.iter().copied().filter(|&r| crate::hal::uart::rate_ok(crate::report::UART_CPS, r)).max();
    link.set_max_baud(fastest.unwrap_or(115_200));
    // Bring-up: the receiver on the cable log once a second while it changes.
    let mut logged = (Live::default(), Instant::now());
    loop {
        // Bring-up: the first byte from the PC ends the log; the terminal has the cable from then on.
        if bringup::ON.load(Relaxed) && TERM_IN.count() > 0 {
            bringup::line(b"log off: the terminal has the cable");
            bringup::ON.store(false, Relaxed);
        }
        if bringup::ON.load(Relaxed) && logged.1.elapsed() >= Duration::from_secs(1) {
            if let Some(l) = live_rx.try_get() {
                if l != logged.0 {
                    log_live(&l);
                    logged.0 = l;
                }
            }
            logged.1 = Instant::now();
        }
        // A whole line waits until the store has caught up (TERM-13): the bytes after it stay in the ring.
        while !bringup::ON.load(Relaxed) && term.ready() && link.ready() {
            let Some(b) = TERM_IN.get() else { break };
            match ferrite_app::link::route(app.settings().cable_port, link.active(now_ms()), b) {
                ferrite_app::link::Route::Link => link.feed(b, now_ms()),
                ferrite_app::link::Route::Term => term.feed(&mut app, b, now_ms(), store),
                ferrite_app::link::Route::Drop => {} // KISS has the cable (chapter 26)
            }
            while let Some(w) = app.pop_write() {
                store.apply(w);
            }
        }
        if !bringup::ON.load(Relaxed) {
            // Debug commands (TERM-21): memory here, registers by the Radio task (it signals TERM_WAKE).
            if let Ok(a) = DEBUG_ANS.try_receive() {
                term.debug_answer(a);
            }
            term.poll(&mut app, store, &dev, &live, now_ms(), &mut Cable);
            // The link: one waiting request answered when the cable and the store have room (LINK-1).
            link.poll(&mut app, store, now_ms(), &mut Cable);
            if let Some(r) = link.take_baud() {
                LINK_BAUD.signal(r); // the Link task switches once this answer has gone (LINK-11)
            }
            if link.take_reboot() {
                // LINK-24: the radio quiet, everything written (at most 3 s), the answer out, then a system
                // reset: the stock bootloader listens for the updater first, as after hwtest's `reboot`.
                RADIO.send(RadioCmd::Shutdown).await;
                store.flush(&app);
                store::wake(store);
                let t = Instant::now();
                while !store.idle() && t.elapsed() < Duration::from_secs(3) {
                    Timer::after_millis(10).await;
                }
                Timer::after_millis(50).await; // the answer's bytes leave the UART
                cortex_m::peripheral::SCB::sys_reset();
            }
            LINK_HELLOS.store(link.hellos(), Relaxed);
            LINK_ON.store(link.active(now_ms()), Relaxed);
            dev.link = ferrite_core::msg::LinkLive {
                cable: link.active(now_ms()),
                baud_100: (CABLE_BAUD.load(Relaxed) / 100) as u16,
                write_pct: link.progress(),
            };
            if let Some(r) = term.debug_req() {
                match r {
                    ferrite_core::msg::DebugReq::Mem { .. } => term.debug_answer(crate::debug::mem(r)),
                    _ if DEBUG_REQ.try_send(r).is_err() => term.debug_answer(ferrite_core::msg::DebugAns::Refused("the Radio task is busy")),
                    _ => {}
                }
                TERM_WAKE.signal(()); // print the answer, or wait for it, on the next turn
            }
        }
        while let Some(c) = app.pop_cmd() {
            RADIO.send(c).await; // the Radio task ignores what it doesn't run yet
        }
        // Channel writes (save as channel, the channel editor, the browser) and the settings and slots (2 s after
        // their last change) queue for the Storage task.
        while let Some(w) = app.pop_write() {
            store.apply(w);
        }
        store.autosave(now_ms(), &app);
        store::wake(store);
        // PWR-17: the power switch off (PE0 high, twice 50 ms apart): "Saving...", the radio off, the pending
        // writes, then the power hold released; if the switch is back on by then, a reset, as stock does.
        // Auto power off (0 7 8) runs out like the switch going off.
        let auto_off = app.take_power_off();
        if POWER_SW.read() || auto_off {
            if !auto_off {
                Timer::after_millis(50).await;
            }
            if auto_off || POWER_SW.read() {
                app.power_off();
                app.render(&mut lcd, &live, &dev, store);
                RADIO.send(RadioCmd::Shutdown).await;
                // Everything queued, then the settings, written before the power goes (at most 3 s).
                store.flush(&app);
                store::wake(store);
                let t = Instant::now();
                while !store.idle() && t.elapsed() < Duration::from_secs(3) {
                    Timer::after_millis(10).await;
                }
                POWER_HOLD.write(false);
                Timer::after_millis(500).await;
                if auto_off && !POWER_SW.read() {
                    // Still powered: the switch is on, and only it cuts the power. Standby until a key: the
                    // chips are off (Shutdown above), the screen, keypad light and LEDs too; then start again.
                    for p in BACKLIGHT {
                        p.write(false);
                    }
                    LED_RED.write(!LED_ACTIVE_HIGH);
                    LED_GREEN.write(!LED_ACTIVE_HIGH);
                    while !matches!(INPUT.receive().await, Input::Key(keypad::Event::Press(_))) && !POWER_SW.read() {}
                }
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
        app.render(&mut lcd, &live, &dev, store);
        // The lights (decision 63): screen and keypad, and the LEDs with their next change.
        SCREEN_LIGHT.write(!app.dark());
        KEYPAD_LIGHT.write(app.keypad_lit());
        let leds = app.leds(&live, &dev);
        LED_RED.write(leds.red == LED_ACTIVE_HIGH);
        LED_GREEN.write(leds.green == LED_ACTIVE_HIGH);
        let mut period = if down != 0 || term.busy() || !link.ready() { TICK_HELD_MS } else { TICK_IDLE_MS };
        if leds.next_ms != 0 {
            period = period.min(leds.next_ms as u64);
        }
        let wait = select(Timer::after(Duration::from_millis(period)), TERM_WAKE.wait());
        match select4(live_rx.changed(), INPUT.receive(), wait, ALERTS.receive()).await {
            Either4::First(l) => slot_a(&mut live, &l),
            // WX-4: a decoded SAME header opens the alert over any screen.
            Either4::Fourth(a) => {
                app.same_alert(&a.header, a.slot, a.freq);
            }
            Either4::Second(Input::Key(keypad::Event::Press(code))) => {
                log_input(&Input::Key(keypad::Event::Press(code)));
                app.raw_key(code, true); // every code, mapped or not: the key test names them (KEY-2)
                if let Some(k) = keys::key(code) {
                    down |= 1 << code;
                    app.key_down(k, now_ms(), store);
                }
            }
            Either4::Second(Input::Key(keypad::Event::Release(code))) => {
                log_input(&Input::Key(keypad::Event::Release(code)));
                app.raw_key(code, false);
                if let Some(k) = keys::key(code) {
                    down &= !(1 << code);
                    app.key_up(k, now_ms(), store);
                }
            }
            Either4::Second(Input::Key(_)) => {} // the sampler's own long press and repeat are off
            Either4::Second(Input::Knob(d)) => {
                log_input(&Input::Knob(d));
                app.knob(d, now_ms(), store)
            }
            Either4::Third(_) => {} // the tick, or bytes for the terminal
        }
        app.tick(now_ms(), &live, store);
    }
}

/// Bring-up log: a key code down or up (with the key it maps to, if any), or a knob step.
fn log_input(i: &Input) {
    bringup::linef(|b| match *i {
        Input::Key(keypad::Event::Press(c) | keypad::Event::Release(c)) => {
            let down = matches!(i, Input::Key(keypad::Event::Press(_)));
            let _ = ufmt::uwrite!(b, "key {} {}", c, if down { "down" } else { "up" });
            match keys::key(c) {
                Some(k) => {
                    let mut n = ferrite_ui::text::Buf::<16>::new();
                    ferrite_app::screens::keytest::key_name(k, &mut n);
                    b.push(b" = ");
                    b.push(n.as_bytes());
                }
                None => b.push(b" (not mapped)"),
            }
        }
        Input::Knob(d) => {
            let _ = ufmt::uwrite!(b, "knob {}", d);
        }
        Input::Key(_) => b.push(b"key event"),
    });
}

/// Bring-up log: the receiver's snapshot.
fn log_live(l: &Live) {
    bringup::linef(|b| {
        let f = l.freq % 100_000;
        let _ = ufmt::uwrite!(b, "rx {}.", l.freq / 100_000);
        for d in [10_000, 1_000, 100, 10, 1] {
            b.push(&[b'0' + (f / d % 10) as u8]); // five decimals, zeros kept: 146.05200
        }
        let _ = ufmt::uwrite!(b, " MHz  rssi {} dBm  squelch {}  chips ", l.rssi, if l.squelch_open { "open" } else { "closed" });
        bringup::hex(b, l.chips as u32);
        if l.rx_asleep {
            b.push(b"  asleep");
        }
    });
}

/// Bring-up log: what the boot found.
fn boot_report(flash: Option<(u32, u32)>, checks: &[Check], key_seen: bool) {
    bringup::linef(|b| {
        let _ = ufmt::uwrite!(b, "Ferrite {} bring-up build: nothing is written to the SPI flash", VERSION);
    });
    bringup::linef(|b| {
        b.push(b"MCU idcode ");
        bringup::hex(b, crate::hal::sys::idcode());
        b.push(b" (expected ");
        bringup::hex(b, chipid::EXPECTED_PID);
        b.push(b")");
    });
    bringup::linef(|b| match flash {
        Some((id, size)) => {
            b.push(b"SPI flash JEDEC ");
            bringup::hex(b, id);
            let _ = ufmt::uwrite!(b, ", {} KB", size / 1024);
        }
        None => b.push(b"SPI flash: no answer"),
    });
    for c in checks {
        bringup::linef(|b| {
            b.push(c.label.as_bytes());
            b.push(match c.state {
                State::Ok => b": ok",
                State::Fail => b": FAILED",
                State::Off => b": not tested at boot",
                State::Pending => b": no report",
            });
        });
    }
    bringup::line(if key_seen { b"key at boot: opening the key test (hold BACK leaves)" } else { b"no key at boot" });
    bringup::line(b"keys, knob and receiver follow; send any byte for the terminal");
}
