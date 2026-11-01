//! Satellite mode's screens (manual chapter 28a; the mock-ups' five artboards): the pass list, a pass before it
//! rises, one overhead, keyed up in full duplex, the pointing page, and the alarm picker. The sample satellites
//! (data.ferritefw.dev's fixture) predicted from the sample's fix, the clock set so a high pass is under way.

use crate::fake;
use ferrite_app::App;
use ferrite_core::keys::Key;
use ferrite_core::sat::data::{Live, SatData};
use ferrite_core::sat::look::Observer;
use ferrite_ui::fb::FrameBuffer;

pub fn run(scale: u32) {
    let store = fake::store();
    let mut radio = fake::FakeRadio::new();
    let mut dev = fake::device();
    dev.clock_min = Some(14 * 60 + 5);
    dev.tx_built = true;
    let mut app = fake::app();
    app.settings_mut().backlight = 0;
    fake::sample(&mut app, &store, &mut radio);
    app.settings_mut().tz = 14; // UTC (TIME_ZONES[14]), as the status bar's clock here
    app.sat_attach(Box::leak(Box::new(Live::new(ferrite_app::sat::SimSats::new(ferrite_app::sat::sample())))));
    let here = store.fix.get().map(|f| (f.pos.lat, f.pos.lon, f.alt_m));
    // the first pass over 40° after the sample's day starts: the shots' clock is set around it
    let mut look = Live::new(ferrite_app::sat::SimSats::new(ferrite_app::sat::sample()));
    let obs = here.map(|(a, o, h)| Observer::new(a, o, h));
    look.tick(Some(ferrite_app::sat::SIM_T0), obs, true, u32::MAX);
    let high = (0..look.passes()).filter_map(|i| look.pass(i)).find(|p| p.max_el > 40).expect("a high pass in 24 h");
    let mut fb = FrameBuffer::new();
    let mut t = 0u32;
    let mut base = high.aos - 600; // 10 minutes before it rises
    let mut run = |app: &mut App, fb: &mut FrameBuffer, until: u32, t: &mut u32, base: u32| {
        while *t <= until {
            while let Some(c) = app.pop_cmd() {
                radio.cmd(c);
            }
            radio.step(*t as f32 / 1000.0, app.active());
            app.sat_clock(Some(base + *t / 1000), true, here);
            dev.clock_min = Some(((base + *t / 1000) / 60 % 1440) as u16);
            app.tick(*t, &radio.live, &store);
            app.render(fb, &radio.live, &dev, &store);
            *t += 20;
        }
    };
    let press = |app: &mut App, key: Key, t: &mut u32| {
        app.key_down(key, *t, &store);
        *t += 80;
        app.tick(*t, &Default::default(), &store);
        app.key_up(key, *t, &store);
        *t += 150;
    };
    run(&mut app, &mut fb, 1000, &mut t, base);
    app.open(ferrite_app::screens::nav::Dest::Satellites);
    run(&mut app, &mut fb, t + 3000, &mut t, base); // predicted
    crate::save(&fb, "sat-passes", scale);
    // the high pass, before it rises: down the list to it
    let rows = (0..look.passes()).filter_map(|i| look.pass(i)).filter(|p| p.los() > base && p.aos < high.aos).count();
    for _ in 0..rows {
        press(&mut app, Key::Down, &mut t);
    }
    press(&mut app, Key::Menu, &mut t);
    run(&mut app, &mut fb, t + 500, &mut t, base);
    crate::save(&fb, "sat-pass-before", scale);
    press(&mut app, Key::Hash, &mut t);
    run(&mut app, &mut fb, t + 200, &mut t, base);
    crate::save(&fb, "sat-alarm", scale);
    press(&mut app, Key::Menu, &mut t); // 5 minutes
    // overhead: the clock moved to a little before the top
    base = high.aos + high.tmax as u32 - 40 - t / 1000;
    run(&mut app, &mut fb, t + 1500, &mut t, base);
    crate::save(&fb, "sat-pass", scale);
    press(&mut app, Key::Digit(0), &mut t);
    run(&mut app, &mut fb, t + 500, &mut t, base);
    crate::save(&fb, "sat-point", scale);
    press(&mut app, Key::Digit(0), &mut t);
    // keyed up, full duplex: PTT held
    app.key_down(Key::Ptt(ferrite_core::keys::PTT_A), t, &store);
    run(&mut app, &mut fb, t + 7000, &mut t, base);
    crate::save(&fb, "sat-keyed", scale);
    app.key_up(Key::Ptt(ferrite_core::keys::PTT_A), t, &store);
    run(&mut app, &mut fb, t + 500, &mut t, base);
}
