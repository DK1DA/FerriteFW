//! Renders of APRS mode (manual chapter 25, APRS-15 to APRS-20) into `target/sim/`: slot C's APRS card, small
//! and large, the beacon waiting for the relay, and the messages list with sent messages in their states. The
//! simulated APRS task ([`ferrite_app::sample::Sample::aprs_tick`]) runs once a second as in the simulator.

use ferrite_app::data::{Data, Draft, Op};
use ferrite_app::sample::Sample;
use ferrite_app::screens::nav::Dest;
use ferrite_app::App;
use ferrite_core::keys::Key;
use ferrite_core::msg::Device;
use ferrite_sim::fake;
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::text::Buf;

struct Shot {
    app: App,
    store: Sample,
    dev: Device,
    radio: fake::FakeRadio,
    t: u32,
    scale: u32,
    fb: FrameBuffer,
}

impl Shot {
    fn run(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t <= end {
            while let Some(c) = self.app.pop_cmd() {
                self.radio.cmd(c);
            }
            while let Some(w) = self.app.pop_write() {
                self.store.apply(w);
            }
            self.radio.step(self.t as f32 / 1000.0, self.app.active());
            if self.t.is_multiple_of(1000) {
                let a = &mut self.app;
                let ok = ferrite_core::txplan::relay_ok(a.tuned_freq(a.active()), a.aprs_freq());
                // APRS-29: beacons alone run the beacon's timer too (the card's countdown).
                let on = a.aprs_on() || a.aprs_holder() == Some(ferrite_app::aprs::Holder::Beacon);
                let n = self.store.aprs_tick(self.t / 1000, on, ok);
                if let Some((from, text)) = n.inbound {
                    a.notify(from.as_bytes(), text.as_bytes(), false);
                }
                if let Some(what) = n.tx {
                    if ferrite_app::screens::transmit::auto_tx(a, 2, what.as_bytes()) {
                        self.radio.burst(2);
                    }
                }
            }
            self.app.tick(self.t, &self.radio.live, &self.store);
            self.app.render(&mut self.fb, &self.radio.live, &self.dev, &self.store);
            self.t += 20;
        }
    }

    fn press(&mut self, key: Key, hold: u32) {
        self.app.key_down(key, self.t, &self.store);
        self.t += hold;
        self.app.tick(self.t, &self.radio.live, &self.store);
        self.app.key_up(key, self.t, &self.store);
        self.run(40);
    }

    fn keys(&mut self, keys: &[Key]) {
        for &k in keys {
            self.press(k, 80);
        }
    }

    fn save(&mut self, name: &str) {
        self.run(40);
        if self.app.toast_up() {
            self.run(3100);
        }
        crate::save(&self.fb, name, self.scale);
    }
}

pub fn run(scale: u32) {
    let store = fake::store();
    let mut dev = fake::device();
    dev.clock_min = Some(14 * 60 + 32);
    dev.utc = false;
    let mut app = fake::app();
    app.settings_mut().backlight = 0;
    let mut radio = fake::FakeRadio::new();
    fake::sample(&mut app, &store, &mut radio);
    let mut s = Shot { app, store, dev, radio, t: 0, scale, fb: FrameBuffer::new() };
    s.run(500);
    use Key::Digit as D;

    // MENU 0 5 3 on, timed beacons every 10 min (0 5 6 1), then a minute of traffic.
    s.keys(&[Key::Menu, D(0), D(5), D(3)]);
    s.app.knob(1, s.t, &s.store);
    s.press(Key::Exit, 800);
    s.keys(&[Key::Menu, D(0), D(5), D(6), D(1)]);
    s.app.knob(5, s.t, &s.store);
    s.press(Key::Exit, 800);
    s.run(62_000);
    s.save("aprsmode-card");
    s.keys(&[Key::Slot, Key::Slot]);
    s.save("aprsmode-card-big");
    // APRS-17.6: \* mutes C; the card says so by the power, the header keeps "VHF: APRS"
    s.press(Key::Star, 80);
    s.save("aprsmode-card-muted-big");
    s.keys(&[Key::Slot]);
    s.save("aprsmode-card-muted");
    s.keys(&[Key::Slot, Key::Slot]);
    s.press(Key::Star, 80);
    s.press(Key::Up, 80);
    s.run(200);
    crate::save(&s.fb, "aprsmode-locked", scale); // APRS-19's toast
    s.press(Key::Menu, 800);
    s.save("aprsmode-card-menu");
    s.press(Key::Exit, 80);

    // PTT A with C (the APRS slot) active: compose; PTT sends. One to a station heard (acked on its second
    // try), one to nobody (retrying).
    for (to, text) in [("VE3KPW-9", "On my way, 10 min"), ("VE3ZZZ", "Are you QRV?")] {
        s.press(Key::Ptt(0), 80);
        let d = Draft { to: Buf::from(to.as_bytes()), text: Buf::from(text.as_bytes()) };
        s.store.request(Op::Draft(d));
        s.run(200);
        if to == "VE3KPW-9" {
            s.save("aprsmode-compose");
        }
        s.press(Key::Ptt(0), 80);
        s.run(1000);
    }
    s.run(65_000);
    s.app.open(Dest::AprsMessages);
    s.save("aprsmode-sent");

    // A on 20 m holds the relay: the beacon waits (APRS-20).
    s.press(Key::Exit, 800);
    s.keys(&[Key::Slot]); // A
    s.app.set_vfo(0, 1_407_400);
    s.store.request(Op::BeaconNow);
    s.run(2000);
    s.save("aprsmode-beacon-waiting");

    // APRS-17 (decision 181): the card for each holder. A back on 2 m; APRS mode off with beacons every 10 min
    // left on: beacons alone hold C ("APRS Beacon", the countdown), small and large.
    s.app.set_vfo(0, 14_652_000);
    s.press(Key::Exit, 800);
    s.keys(&[Key::Menu, D(0), D(5), D(3)]);
    s.app.knob(-1, s.t, &s.store);
    s.press(Key::Exit, 800);
    s.run(4000);
    s.save("aprsbeacon-card");
    s.keys(&[Key::Slot, Key::Slot]); // C
    s.save("aprsbeacon-card-big");
    // KISS on Bluetooth: "APRS KISS", the port and the frames counted.
    s.app.settings_mut().bt = true;
    s.app.settings_mut().kiss_port = 2;
    for sent in [true, false, false, true, false] {
        s.store.kiss_frame(sent);
    }
    s.run(4000);
    s.save("aprskiss-card-big");
    s.keys(&[Key::Slot]); // A
    s.save("aprskiss-card");
}
