//! The interactive simulator without a window: the firmware's own UI (`ferrite-app` drawing through
//! `ferrite-ui`), a fake three-slot radio and the radio's front panel ([`crate::face`]), driven by key, mouse and
//! event calls and stepped by the caller's clock. The desktop window (`cargo simw`, minifb) and the web page
//! (`sim-web`, wasm) are thin shells over it.
//!
//! Time is the caller's: [`SimCore::step`] takes milliseconds since start. The status bar clock comes from
//! [`SimCore::set_clock`] (UTC seconds), which the desktop sets from the PC and the page from the browser.
//!
//! Channels, zones, range presets, settings and slots live in a simulated SPI flash through the firmware's own
//! store (`ferrite-store`), so they survive the power switch; the caller keeps the flash image between runs
//! ([`SimCore::flash_image`]: the desktop in `target/sim/flash.bin`, the page in the browser).

use crate::face::{Face, Part};
use crate::fake::{self, FakeRadio};
use ferrite_app::chan::ChannelStore;
use ferrite_app::data::{Data, MsgState, Src};
use ferrite_app::link::{self, Route, Session};
use ferrite_app::sample::Sample;
use ferrite_app::settings::Settings;
use ferrite_core::layout::{LAYOUT_HEADER, WAYPOINTS};
use ferrite_store::sim::SimFlash;
use ferrite_store::FlashStore;
use ferrite_app::screens::nav::{self, Dest};
use ferrite_app::{App, Screen as AppScreen};
use ferrite_core::freq;
use ferrite_core::keys::Key;
use ferrite_core::msg::{Device, GpsState};
use ferrite_core::VERSION;
use ferrite_ui::boot::{self, BootInfo, Check, State};
use ferrite_ui::fb::FrameBuffer;
use ferrite_app::term::{cmd, Sink, Term};
use ferrite_ui::gfx::Gfx;
use ferrite_ui::panel::{Panel, H, W};

/// The PC keyboard map, as the desktop window has it (the web page uses the same keys).
pub const KEYMAP: &str = "\
keys: arrows = UP/DOWN/LEFT/RIGHT · Enter = MENU/OK · Esc = BACK · 0-9 digits · , = * · . = #
      S = SLOT · V = V/M · Q = SIDE1 · W = SIDE2 · Space = PTT A · [ ] or mouse wheel = knob
      or click the radio's keys (hold the mouse for long presses; hold SIDE1 + wheel = squelch)
      hold a key 0.7 s for its long press (hold S: edit, hold . : lock, hold Esc: home, hold Enter: MENU 0)
      M message · N alert · F5 GPS · F6 Bluetooth · F7 battery · F8 power off (or click VOL: the power switch) · F9 empty battery
      F10 unknown SSB patch · F11 a write from a new device · X crossband (also MENU 0 3 5)
      L home layout · B a beacon (automatic transmit) · hold Enter while booting: the settings reset
      F2 redraw flash · F3 mock-up overlay · F12 screenshot · Ctrl-Q quit";

/// The largest whole scale offered.
pub const MAX_SCALE: usize = 2;

/// How long the boot screen shows (ms), and how long "Saving..." shows before the radio goes dark.
const BOOT_MS: u32 = 2000;
const SAVE_MS: u32 = 1500;

/// The frame buffer, plus a record of what each redraw touched.
struct Counting {
    fb: FrameBuffer,
    rects: Vec<(i32, i32, i32, i32)>,
    pixels: u64,
}

impl Panel for Counting {
    fn window(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.rects.push((x0, y0, x1, y1));
        self.fb.window(x0, y0, x1, y1);
    }
    fn push(&mut self, color: u16, count: u32) {
        self.pixels += count as u64;
        self.fb.push(color, count);
    }
    fn push_buf(&mut self, px: &[u16]) {
        self.pixels += px.len() as u64;
        self.fb.push_buf(px);
    }
}

/// What the radio (or the world) would report, as the simulator's event keys send it. Each calls the App hook the
/// firmware calls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// M: an APRS message arrives.
    Message,
    /// N: the fake decoder's SAME warning, as if heard on slot C.
    Alert,
    /// F5: GPS off, searching, fix.
    Gps,
    /// F6: Bluetooth on/off.
    Bluetooth,
    /// F7: battery normal, low, cut-off.
    Battery,
    /// F8, or a click on VOL: the power switch (off: "Saving...", then dark; on: a fresh boot).
    PowerSwitch,
    /// F9: an empty battery.
    EmptyBattery,
    /// F10: an unknown SSB patch at boot.
    SsbPatch,
    /// F11: a write from a new Bluetooth device.
    LinkWrite,
    /// X: the crossband repeater.
    Crossband,
    /// L: the next home layout.
    Layout,
    /// B: a beacon (an automatic transmit).
    Beacon,
}

/// A recent redraw region and when it was drawn (ms), for the redraw flash.
struct Flash {
    rect: (i32, i32, i32, i32),
    at: u32,
}

/// The boot checks the simulator shows: all passed.
const CHECKS: [Check; 6] = [
    Check { label: "MCU", value: "AT32F403A", state: State::Ok },
    Check { label: "SPI flash", value: "4 MB", state: State::Ok },
    Check { label: "BK4829 VHF", value: "0x4829", state: State::Ok },
    Check { label: "BK4829 UHF", value: "0x4829", state: State::Ok },
    Check { label: "SI4732", value: "not tested", state: State::Off },
    Check { label: "Calibration", value: "", state: State::Ok },
];

/// The simulated flash's saved part: from the layout header to the end of the waypoints (the event log, APRS
/// messages and waypoints included).
pub const FLASH_IMAGE: core::ops::Range<u32> = LAYOUT_HEADER.start..WAYPOINTS.end;

/// What the flash holds at power on.
pub enum Stored {
    /// The look review's sample: zones, channels, presets and settings written to a blank flash.
    Sample,
    /// A saved image ([`SimCore::flash_image`]); an empty one gives a blank flash (setup).
    Image(Vec<u8>),
    /// Blank, as after installing: setup runs (BOOT-12).
    Blank,
}

type Store = FlashStore<SimFlash, Sample>;

pub struct SimCore {
    app: App,
    store: Store,
    radio: FakeRadio,
    dev: Device,
    lcd: Counting,
    face: Face,
    scale: usize,
    out: Vec<u32>,
    now: u32,
    /// UTC seconds at `now` = 0, for the status bar clock.
    clock0: u64,
    // The power switch: booting until `boot_end`; `off_at` while the app saves before going dark.
    booting: bool,
    boot_end: u32,
    off_at: Option<u32>,
    off: bool,
    // Parts held by the keyboard and by the mouse; the knob half last clicked and until when it shows; PTT.
    kb_held: Vec<usize>,
    mouse: Option<usize>,
    knob_flash: (i8, u32),
    /// The second the APRS task last ran.
    aprs_s: u32,
    tx: Option<u8>,
    battery: usize,
    layout: u8,
    last_key: String,
    last_px: u64,
    flashes: Vec<Flash>,
    /// The terminal over the cable (manual chapter 37), and what it has drawn that the cable hasn't taken.
    term: Term,
    term_out: Vec<u8>,
    /// Bytes from the cable the terminal hasn't taken yet: a line waits while the one before it runs (TERM-13).
    term_in: std::collections::VecDeque<u8>,
    /// The link protocol on the same cable (manual chapter 29; TERM-2 routes the bytes): trusted, as the cable is.
    link: Session,
}

/// The terminal's bytes waiting for the cable: room up to [`TERM_BUF`].
struct TermBuf<'a>(&'a mut Vec<u8>);

/// How much drawn output the simulator holds for a slow reader (about two full 80 x 24 screens).
pub const TERM_BUF: usize = 8192;

impl Sink for TermBuf<'_> {
    fn room(&self) -> usize {
        TERM_BUF.saturating_sub(self.0.len())
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// A window position to panel (buffer) pixels: the panel sits 1:1 in the middle of the window.
pub fn to_panel(m: (f32, f32), win: (usize, usize), buf: (usize, usize)) -> (i32, i32) {
    let (ox, oy) = ((win.0 as f32 - buf.0 as f32) / 2.0, (win.1 as f32 - buf.1 as f32) / 2.0);
    ((m.0 - ox).floor() as i32, (m.1 - oy).floor() as i32)
}

/// The largest whole scale whose panel fits the window; 1 if none does. `sizes[i]` is the panel at scale i + 1.
pub fn fit_scale(win: (usize, usize), sizes: &[(usize, usize)]) -> usize {
    sizes.iter().rposition(|&(w, h)| w <= win.0 && h <= win.1).map_or(1, |i| i + 1)
}

/// RGB565 to 0xRRGGBB.
pub fn rgb565_to_rgb(c: u16) -> u32 {
    let r = ((c >> 11) & 31) as u32 * 255 / 31;
    let g = ((c >> 5) & 63) as u32 * 255 / 63;
    let b = (c & 31) as u32 * 255 / 31;
    r << 16 | g << 8 | b
}

fn blend(a: u32, b: u32) -> u32 {
    let ch = |s: u32| ((a >> s & 255) + (b >> s & 255)) / 2;
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// The key a front-panel part sends.
fn key_of(p: Part) -> Key {
    match p {
        Part::Key(k) => k,
        Part::Ptt(n) => Key::Ptt(n),
        _ => Key::PTT,
    }
}

fn freq_text(f: freq::Freq10) -> String {
    let mut b = [0u8; 16];
    let n = freq::format(f, &mut b);
    String::from_utf8_lossy(&b[..n]).into_owned()
}

/// The screens [`SimCore::jump`] offers: home, then every destination in the manual's inventory.
pub fn jump_names() -> impl Iterator<Item = String> {
    core::iter::once(String::from("Home")).chain(nav::ALL.iter().map(|d| {
        let (name, _, id) = d.info();
        format!("{id} {name}")
    }))
}

impl SimCore {
    /// A radio switched on now (the boot screen), the panel at a whole `scale`, on the sample data; `owner_card`
    /// boots with the sample owner card (MENU 0 7 3 3).
    pub fn new(scale: usize, owner_card: bool) -> Self {
        Self::with_flash(scale, owner_card, Stored::Sample)
    }

    /// A radio switched on now with the flash holding `stored` (see [`Stored`]).
    pub fn with_flash(scale: usize, owner_card: bool, stored: Stored) -> Self {
        let scale = scale.clamp(1, MAX_SCALE);
        let nor = match &stored {
            Stored::Image(img) if !img.is_empty() => SimFlash::with_image(FLASH_IMAGE.start, img),
            _ => SimFlash::new(),
        };
        let store = FlashStore::mount(nor, Sample::new());
        if matches!(stored, Stored::Sample) && store.first_boot() {
            seed(&store);
        }
        let mut radio = FakeRadio::new();
        let app = power_on(&store, &mut radio);
        if matches!(stored, Stored::Sample) {
            // The mock-ups' C: scanning airband, a few hits in.
            radio.cmd(ferrite_core::msg::RadioCmd::Scan { slot: 2, on: true });
            radio.live.slots[2].hits = 3;
        }
        let face = Face::new(scale);
        let out = vec![0u32; face.w * face.h];
        let mut s = SimCore {
            app,
            store,
            radio,
            dev: fake::device_at(0),
            lcd: Counting { fb: FrameBuffer::new(), rects: Vec::new(), pixels: 0 },
            face,
            scale,
            out,
            now: 0,
            clock0: 0,
            booting: true,
            boot_end: BOOT_MS,
            off_at: None,
            off: false,
            kb_held: Vec::new(),
            mouse: None,
            knob_flash: (0, 0),
            aprs_s: u32::MAX,
            tx: None,
            battery: 0,
            layout: 0,
            last_key: String::from("-"),
            last_px: 0,
            flashes: Vec::new(),
            term: Term::new(),
            term_out: Vec::new(),
            term_in: std::collections::VecDeque::new(),
            link: {
                let mut l = Session::new(true);
                l.set_max_baud(921_600); // a pseudo-terminal has no rate
                l
            },
        };
        if owner_card {
            ferrite_app::sample::owner_card(s.app.settings_mut());
        }
        s.draw_boot();
        s.last_px = s.lcd.pixels;
        s
    }

    /// The saved part of the flash ([`FLASH_IMAGE`]), for the caller to keep.
    pub fn flash_image(&self) -> Vec<u8> {
        let r = FLASH_IMAGE.start as usize..FLASH_IMAGE.end as usize;
        self.store.nor().mem[r].to_vec()
    }

    /// The flash changed since the last call (time to save the image).
    pub fn flash_changed(&mut self) -> bool {
        core::mem::take(&mut self.store.nor().dirty)
    }

    /// Settings and slots saved now (the caller is about to quit).
    pub fn flush(&mut self) {
        if !self.booting {
            self.store.flush(&self.app);
        }
    }

    /// The boot screen from the app's settings, as the firmware draws it: the owner card if 0 7 3 3 says so.
    fn draw_boot(&mut self) {
        let set = *self.app.settings();
        ferrite_ui::theme::set(set.theme); // the boot screen in the saved theme (0 7 1)
        boot::draw(&mut self.lcd, &BootInfo { version: VERSION, callsign: set.boot_callsign(), line: set.boot_line(), owner: set.boot_owner(), checks: &CHECKS });
    }

    /// The status bar clock: `utc_secs` is UTC now (seconds since 1970).
    pub fn set_clock(&mut self, utc_secs: u64) {
        self.clock0 = utc_secs.saturating_sub(self.now as u64 / 1000);
        self.store.set_time(utc_secs as u32); // the log's and messages' times in UTC from now
    }

    pub fn scale(&self) -> usize {
        self.scale
    }

    /// The panel's size in pixels at the current scale.
    pub fn size(&self) -> (usize, usize) {
        (self.face.w, self.face.h)
    }

    /// The screen's top-left inside the panel, in panel pixels.
    pub fn screen_origin(&self) -> (usize, usize) {
        (self.face.sx, self.face.sy)
    }

    /// A new whole scale (1 to [`MAX_SCALE`]); held parts are forgotten.
    pub fn set_scale(&mut self, scale: usize) {
        let scale = scale.clamp(1, MAX_SCALE);
        if scale != self.scale {
            self.scale = scale;
            self.face = Face::new(scale);
            self.out = vec![0u32; self.face.w * self.face.h];
            self.kb_held.clear();
            self.mouse = None;
        }
    }

    fn live(&self) -> bool {
        !self.booting
    }

    /// Whether MENU is held, by key or mouse.
    fn menu_held(&self) -> bool {
        self.face.find(Key::Menu).is_some_and(|i| self.kb_held.contains(&i) || self.mouse == Some(i))
    }

    /// Advance to `now_ms` (since start): boot and power-off timing, the fake radio following the app's commands,
    /// the app's tick and redraw.
    pub fn step(&mut self, now_ms: u32) {
        self.now = now_ms;
        let now = now_ms;
        self.lcd.rects.clear();
        self.lcd.pixels = 0;
        if self.off_at.is_some_and(|t| now >= t) {
            // Saved: the radio is off and the screen dark until the switch is turned on again.
            (self.off_at, self.off, self.booting) = (None, true, true);
            self.lcd.fill(0, 0, W, H, 0);
        }
        if self.booting && !self.off && now > self.boot_end {
            self.booting = false;
            // As the firmware does: setup on a first boot (BOOT-12); MENU held through the boot screen: the
            // settings reset (PIN-6, BOOT-14); else the PIN if one is set.
            if self.store.first_boot() {
                self.app.first_boot();
            } else if self.menu_held() {
                self.app.power_on_reset();
            } else {
                self.app.ask_pin();
            }
        }
        if self.live() {
            let (app, radio, store, dev) = (&mut self.app, &mut self.radio, &self.store, &mut self.dev);
            while let Some(c) = app.pop_cmd() {
                radio.cmd(c);
            }
            while let Some(w) = app.pop_write() {
                store.apply(w);
            }
            radio.step(now as f32 / 1000.0, app.active());
            if let Some(a) = radio.take_same() {
                app.same_alert(&a.header, a.slot, a.freq); // WX-4: the decoder's alert, over any screen
                let name = ferrite_core::same::event_name(&a.header.event).unwrap_or("Alert");
                store.log(&log_event(Src::Same, a.slot, a.freq, name.as_bytes(), &a.header.event)); // INBOX-6
            }
            // The fake link: an allowed write runs a session, 2 % a step, then ends.
            match app.take_link_answer() {
                Some(ferrite_app::screens::link::LinkAnswer::Allowed) => {
                    dev.link.write_pct = Some(0);
                    app.link_write_progress();
                }
                Some(_) => dev.link.write_pct = None,
                None => {}
            }
            if let Some(p) = dev.link.write_pct {
                if p >= 100 {
                    dev.link.write_pct = None;
                    app.link_write_done();
                } else {
                    dev.link.write_pct = Some(p + 2);
                }
            }
            // The APRS task, once a second (APRS-15 to APRS-20): arrivals notify, packets key C for a moment.
            if now / 1000 != self.aprs_s {
                self.aprs_s = now / 1000;
                let relay_ok = ferrite_core::txplan::relay_ok(app.tuned_freq(app.active()), app.aprs_freq());
                let n = store.extra.aprs_tick(self.aprs_s, app.aprs_on(), relay_ok);
                if let Some((from, text)) = n.inbound {
                    app.notify(from.as_bytes(), text.as_bytes(), false); // APRS-6
                    store.message_in(true, from.as_bytes(), text.as_bytes(), 0);
                    store.log(&log_event(Src::Aprs, 2, app.aprs_freq(), from.as_bytes(), text.as_bytes())); // INBOX-6
                }
                for (peer, text, st) in n.states.into_iter().flatten() {
                    store.message_state(peer.as_bytes(), text.as_bytes(), st);
                }
                if let Some(what) = n.tx {
                    if ferrite_app::screens::transmit::auto_tx(app, 2, what.as_bytes()) {
                        radio.burst(2);
                    }
                }
            }
            let secs = self.clock0 + now as u64 / 1000;
            dev.clock_min = Some(((secs % 86_400) / 60) as u16);
            app.tick(now, &radio.live, store);
            while let Some(c) = app.pop_cmd() {
                radio.cmd(c);
            }
            while let Some(w) = app.pop_write() {
                store.apply(w);
            }
            store.autosave(now, app);
            app.render(&mut self.lcd, &radio.live, dev, store);
            self.term.poll(app, store, dev, &radio.live, now, &mut TermBuf(&mut self.term_out));
            if let Some(r) = self.term.debug_req() {
                self.term.debug_answer(radio.debug(r)); // TERM-21: the fake radio answers at once
            }
            self.link.poll(app, store, now, &mut TermBuf(&mut self.term_out));
            let _ = self.link.take_baud(); // a pseudo-terminal keeps no rate
            if let Some(p) = self.link.progress() {
                dev.link.write_pct = Some(p);
            }
            while let Some(w) = app.pop_write() {
                store.apply(w);
            }
        }
        if self.live() && (!self.term_in.is_empty() || self.term.busy() || !self.link.ready()) {
            self.term_pump(); // a load or a dump goes on between frames
        }
        if self.lcd.pixels > 0 {
            self.last_px = self.lcd.pixels;
        }
        self.flashes.extend(self.lcd.rects.iter().map(|&rect| Flash { rect, at: now }));
        self.flashes.retain(|f| now.wrapping_sub(f.at) < 250);
    }

    /// The whole panel as 0xRRGGBB pixels, `size()` wide and tall: the front panel, then the screen scaled up,
    /// with an optional mock-up blended over it (240 x 320 RGB) and optional redraw outlines.
    pub fn compose(&mut self, show_flash: bool, mockup: Option<&[u32]>) -> &[u32] {
        let mut pressed = self.kb_held.clone();
        pressed.extend(self.mouse);
        let knob = if self.now < self.knob_flash.1 { self.knob_flash.0 } else { 0 };
        // The LEDs as the firmware drives them (decision 63: RX, TX and the dark-screen heartbeat).
        let leds = if self.live() { self.app.leds(&self.radio.live, &self.dev) } else { Default::default() };
        // A dark screen (backlight off) shows dimmed, so the sim stays readable.
        let dark = self.live() && self.app.dark();
        let (face, lcd, flashes, scale) = (&self.face, &self.lcd, &self.flashes, self.scale);
        face.draw(&mut self.out, &pressed, knob, leds.green, leds.red, scale);
        face.put_screen(&mut self.out, scale, |x, y| {
            let mut c = rgb565_to_rgb(lcd.fb.at(x, y));
            if dark {
                c = (c >> 2) & 0x3F3F3F;
            }
            if let Some(m) = mockup {
                c = blend(c, m[(y * W + x) as usize]);
            }
            if show_flash
                && flashes.iter().any(|f| {
                    let (x0, y0, x1, y1) = f.rect;
                    (x == x0 || x == x1) && (y0..=y1).contains(&y) || (y == y0 || y == y1) && (x0..=x1).contains(&x)
                })
            {
                c = 0xFF00FF;
            }
            c
        });
        &self.out
    }

    /// A logical key down or up, from the keyboard (or the page's key buttons): lights its part on the panel.
    pub fn key(&mut self, key: Key, down: bool) {
        let part = self.face.find(key);
        if down {
            self.last_key = format!("{key:?}");
            if let Some(i) = part {
                if !self.kb_held.contains(&i) {
                    self.kb_held.push(i);
                }
            }
            if let Key::Ptt(n) = key {
                self.tx = Some(if n < 3 { n } else { 0 });
            }
            if self.live() {
                self.app.key_down(key, self.now, &self.store);
            }
        } else {
            if let Some(i) = part {
                self.kb_held.retain(|&h| h != i);
            }
            if matches!(key, Key::Ptt(_)) {
                self.tx = None;
            }
            if self.live() {
                self.app.key_up(key, self.now, &self.store);
            }
        }
    }

    /// The channel knob: one step down (-1) or up (+1).
    pub fn knob(&mut self, d: i8) {
        self.knob_flash = (d, self.now + 150);
        if self.live() {
            self.app.knob(d, self.now, &self.store);
        }
    }

    /// The mouse at panel pixel (x, y), button `down` or not. Down on a key is its key down until the button comes
    /// up; down on the knob steps it by half; down on VOL flips the power switch.
    pub fn mouse(&mut self, x: i32, y: i32, down: bool) {
        if down && self.mouse.is_none() {
            let Some(i) = self.face.hit(x, y) else { return };
            let h = self.face.hits[i];
            match h.part {
                Part::Power => self.power_switch(),
                Part::Knob => self.knob(if x < h.x + h.w / 2 { -1 } else { 1 }),
                Part::Key(_) | Part::Ptt(_) => {
                    if let Part::Ptt(n) = h.part {
                        self.tx = Some(n);
                    }
                    let key = key_of(h.part);
                    self.last_key = format!("{key:?} (mouse)");
                    if self.live() {
                        self.app.key_down(key, self.now, &self.store);
                    }
                    self.mouse = Some(i);
                }
            }
        } else if !down {
            if let Some(i) = self.mouse.take() {
                if let Part::Ptt(_) = self.face.hits[i].part {
                    self.tx = None;
                }
                if self.live() {
                    self.app.key_up(key_of(self.face.hits[i].part), self.now, &self.store);
                }
            }
        }
    }

    /// Bytes from the terminal program on the cable (ignored while booting or off).
    pub fn term_feed(&mut self, bytes: &[u8]) {
        if !self.live() {
            return;
        }
        self.term_in.extend(bytes);
        self.term_pump();
    }

    /// Feeds the waiting bytes while the terminal takes them, running each whole line as it comes (a load runs
    /// line by line); stops when the output is full or after a few hundred lines, to go on next frame.
    fn term_pump(&mut self) {
        for _ in 0..400 {
            while self.term.ready() && self.link.ready() {
                let Some(b) = self.term_in.pop_front() else { break };
                match link::route(self.app.settings().cable_port, self.link.active(self.now), b) {
                    Route::Link => self.link.feed(b, self.now),
                    Route::Term => self.term.feed(&mut self.app, b, self.now, &self.store),
                    Route::Drop => {}
                }
                while let Some(w) = self.app.pop_write() {
                    self.store.apply(w); // one channel write per key
                }
            }
            self.link.poll(&mut self.app, &self.store, self.now, &mut TermBuf(&mut self.term_out));
            self.term.poll(&mut self.app, &self.store, &self.dev, &self.radio.live, self.now, &mut TermBuf(&mut self.term_out));
            if let Some(r) = self.term.debug_req() {
                self.term.debug_answer(self.radio.debug(r));
            }
            while let Some(w) = self.app.pop_write() {
                self.store.apply(w);
            }
            if self.term_in.is_empty() && self.term.ready() && self.link.ready() || self.term_out.len() >= TERM_BUF / 2 {
                break;
            }
        }
    }

    /// Everything as the terminal's `dump` writes it (TERM-13): text, one command per line, ending with `end`.
    pub fn dump_text(&self) -> Vec<u8> {
        let (mut p, mut l, mut v) = (cmd::DumpPos::default(), cmd::Line::new(), Vec::new());
        while cmd::dump_line(&self.app, &self.store, &self.dev, &mut p, &mut l) {
            v.extend_from_slice(l.as_bytes());
            v.push(b'\n');
        }
        v
    }

    /// A dump read back as the terminal's `load` reads it, line by line up to `end` (TERM-13): what each refused
    /// line was refused for, then the count.
    pub fn load_text(&mut self, text: &[u8]) -> String {
        let (mut n, mut ok, mut bad, mut report) = (0u32, 0u32, 0u32, String::new());
        if !self.live() {
            return String::from("The radio is off or starting: switch it on first.\n");
        }
        for line in text.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.iter().filter(|b| !b.is_ascii_whitespace()).eq(b"end".iter()) {
                break;
            }
            n += 1;
            let r = if line.len() > cmd::LINE_MAX {
                Err(cmd::Msg::from(b"too long: 255 characters at most"))
            } else {
                cmd::run(&mut self.app, &self.store, &self.dev, line, true)
            };
            match r {
                Ok(cmd::Done::Nothing) => {}
                Ok(_) => ok += 1,
                Err(m) => {
                    bad += 1;
                    report.push_str(&format!("line {n}: {}\n", String::from_utf8_lossy(m.as_bytes())));
                }
            }
            while let Some(w) = self.app.pop_write() {
                self.store.apply(w);
            }
            while let Some(c) = self.app.pop_cmd() {
                self.radio.cmd(c);
            }
        }
        report.push_str(&format!("{n} lines read: {ok} applied, {bad} refused.\n"));
        report
    }

    /// What the terminal drew since the last call, for the cable.
    pub fn term_take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.term_out)
    }

    /// The pending output, without taking it (the web page reads it in place).
    pub fn term_pending(&self) -> &[u8] {
        &self.term_out
    }

    /// Drops what [`SimCore::term_pending`] showed.
    pub fn term_clear(&mut self) {
        self.term_out.clear();
    }

    /// The terminal's size from outside (the web page's pane); a terminal program answers the size question itself.
    pub fn term_size(&mut self, cols: u16, rows: u16) {
        self.term.set_size(cols, rows);
    }

    /// The power switch on the volume knob (PE0): off saves ("Saving...", PWR-17) then goes dark; on is a fresh
    /// boot.
    fn power_switch(&mut self) {
        if self.off {
            // On: RAM is gone, so the app starts again from what the flash kept.
            let flash = std::mem::replace(&mut self.store, FlashStore::mount(SimFlash::new(), Sample::new())).into_parts().0;
            self.store = FlashStore::mount(flash, Sample::new());
            self.radio = FakeRadio::new();
            self.app = power_on(&self.store, &mut self.radio);
            (self.off, self.boot_end) = (false, self.now + BOOT_MS);
            self.term = Term::new(); // the cable starts over too: Enter opens the terminal again
            self.link = Session::new(true);
            self.link.set_max_baud(921_600);
            self.term_out.clear();
            self.term_in.clear();
            self.draw_boot();
            self.last_key = String::from("power on");
        } else if self.booting {
            (self.off, self.booting) = (true, true);
            self.lcd.fill(0, 0, W, H, 0);
            self.last_key = String::from("power off");
        } else if self.off_at.is_none() {
            // PWR-17: "Saving...", the pending writes, then off.
            self.app.power_off();
            self.store.flush(&self.app);
            self.off_at = Some(self.now + SAVE_MS);
            self.last_key = String::from("power off");
        }
    }

    /// One of the simulator's stand-ins for what the radio would report (most are ignored while booting or off).
    pub fn event(&mut self, e: Event) {
        let live = self.live();
        let app = &mut self.app;
        match e {
            Event::Message if live => app.notify(b"VE3KPW-9", b"On my way", false),
            Event::Alert if live => {
                // The fake decoder's SAME warning now, as if heard on slot C (it also comes by itself 8 s after a
                // slot settles on a weather channel).
                if let Some(h) = ferrite_core::same::parse(fake::SAME_HEADER) {
                    app.same_alert(&h, 2, 16_255_000);
                }
            }
            Event::Crossband if live => app.open(Dest::Crossband),
            Event::Layout if live => {
                self.layout = (self.layout + 1) % 3;
                app.set_layout(self.layout);
            }
            Event::Beacon if live => {
                let a = app.active();
                ferrite_app::screens::transmit::auto_tx(app, a, b"Beacon");
            }
            Event::Gps => {
                self.dev.gps = match self.dev.gps {
                    GpsState::Off => GpsState::Searching,
                    GpsState::Searching => GpsState::Fix,
                    GpsState::Fix => GpsState::Off,
                }
            }
            Event::Bluetooth => self.dev.bt = !self.dev.bt,
            Event::Battery => {
                self.battery = (self.battery + 1) % 3;
                let d = &mut self.dev;
                (d.battery_low, d.battery_cutoff, d.battery_pct, d.battery_mv) =
                    [(false, false, 82, 7900), (true, false, 15, 7000), (true, true, 3, 6600)][self.battery];
            }
            Event::PowerSwitch => self.power_switch(),
            Event::EmptyBattery if live => app.battery_empty(),
            Event::SsbPatch if live => app.ssb_patch_found(),
            Event::LinkWrite if live => app.link_write_request(),
            _ => {}
        }
    }

    /// Opens screen `i` of [`jump_names`] (0 = home), from home, so BACK returns there.
    pub fn jump(&mut self, i: usize) {
        if !self.live() {
            return;
        }
        self.app.go_home();
        if let Some(&d) = i.checked_sub(1).and_then(|i| nav::ALL.get(i)) {
            self.app.open(d);
        }
    }

    fn screen(&self) -> Option<AppScreen> {
        self.live().then(|| self.app.screen())
    }

    /// The screen on show: "Boot" while booting, "Off" while off.
    pub fn screen_name(&self) -> &'static str {
        if self.off {
            return "Off";
        }
        self.screen().map_or("Boot", |s| s.name())
    }

    /// The look-review mock-up for the screen on show (a placeholder has none: the boot one stands in).
    pub fn mockup_name(&self) -> &'static str {
        self.screen().map_or(Some("BootA.png"), |s| s.mockup()).unwrap_or("BootA.png")
    }

    /// One line of state: screen, last key, TX, the active slot and frequency, lock, the last redraw's pixels.
    pub fn status(&self) -> String {
        let a = self.app.active();
        format!(
            "{} · key {} · {}{} {} · last redraw {} px{}",
            self.screen_name(),
            self.last_key,
            self.tx.map(|n| format!("TX {} · ", ["A", "B", "C"][n as usize % 3])).unwrap_or_default(),
            ["A", "B", "C"][a as usize],
            freq_text(self.app.slot(a).freq()),
            self.last_px,
            if self.app.locked() { " · locked" } else { "" },
        )
    }

    /// Pixels pushed by the last redraw that drew anything.
    pub fn last_px(&self) -> u64 {
        self.last_px
    }

    /// Booting or off: the app is not running.
    pub fn booting(&self) -> bool {
        self.booting
    }
}

/// The settings from the flash (the defaults on a first boot), kept for the program's life as the firmware keeps
/// its own; the App on them with its slots restored, and its start-up commands sent to the fake radio.
fn power_on(store: &Store, radio: &mut FakeRadio) -> App {
    let set: &'static mut Settings = Box::leak(Box::new(Settings::DEFAULT));
    store.load_settings(set);
    let alert = Box::leak(Box::new(ferrite_app::screens::alert::AlertInfo::NONE));
    let mut app = App::new(set, alert);
    store.restore(&mut app);
    while let Some(c) = app.pop_cmd() {
        radio.cmd(c);
    }
    app
}

/// An event for the log, heard now.
fn log_event(src: Src, slot: u8, freq: freq::Freq10, name: &[u8], text: &[u8]) -> ferrite_app::data::Event {
    use ferrite_ui::text::Buf;
    ferrite_app::data::Event { src, ago_s: 0, freq, slot, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(name), text: Buf::from(text) }
}

/// The look review's sample written to a blank flash: its zones, channels (with positions) and range presets,
/// then the sample settings and slots (B on "Club repeater", C scanning airband, watch A+B+C).
fn seed(store: &Store) {
    let sample = Sample::new();
    for z in 0..sample.zones() {
        let name = sample.zone_name(z).unwrap_or_default();
        store.add_zone(name.as_bytes(), &sample.zone_cfg(z));
    }
    for i in 0..sample.count() {
        if let Some(c) = sample.by_number(i) {
            store.put_with_position(c, sample.position(c.num));
        }
    }
    for i in 0..sample.presets() {
        if let Some(p) = sample.preset(i) {
            store.add_preset(&p);
        }
    }
    // The event log, messages and waypoints, oldest first, each as old as the sample says.
    for i in (0..sample.events()).rev() {
        if let Some(e) = sample.event(i) {
            store.log(&e);
        }
    }
    for i in (0..sample.messages()).rev() {
        if let Some(m) = sample.message(i) {
            store.message_in(m.inbound, m.peer.as_bytes(), m.text.as_bytes(), m.ago_s);
            match m.state {
                MsgState::Read => {
                    store.request(ferrite_app::data::Op::MessageRead(0));
                }
                MsgState::Unread | MsgState::Pending => {}
                s => {
                    if let MsgState::Retrying(n) = s {
                        for k in 1..n {
                            store.message_state(m.peer.as_bytes(), m.text.as_bytes(), if k == 1 { MsgState::Pending } else { MsgState::Retrying(k) });
                        }
                    }
                    store.message_state(m.peer.as_bytes(), m.text.as_bytes(), s);
                }
            }
        }
    }
    for i in (0..sample.waypoints()).rev() {
        if let Some(w) = sample.waypoint(i) {
            store.add_waypoint(&w);
        }
    }
    let mut radio = FakeRadio::new();
    let mut app = fake::app();
    fake::sample(&mut app, store, &mut radio);
    store.save(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut SimCore, from: u32, to: u32) {
        for t in (from..=to).step_by(20) {
            s.step(t);
        }
    }

    #[test]
    fn the_flash_keeps_settings_through_the_power_switch_and_the_image() {
        let mut s = SimCore::new(1, false);
        run(&mut s, 0, 2500);
        assert_eq!(s.screen_name(), "Home");
        assert!(s.store.count() > 10, "the sample's channels are in the flash");
        s.event(Event::Layout);
        run(&mut s, 2520, 5000); // saved 2 s after the change
        let layout = s.app.settings().layout;
        assert_eq!(layout, 1);
        s.event(Event::PowerSwitch);
        run(&mut s, 5020, 7000);
        assert_eq!(s.screen_name(), "Off");
        s.event(Event::PowerSwitch);
        run(&mut s, 7020, 9500);
        assert_eq!(s.screen_name(), "Home");
        assert_eq!(s.app.settings().layout, layout, "kept through the power switch");
        assert_eq!(s.app.slot(1).ch.map(|c| c.num), Some(11), "B still on Club repeater");
        // The image, as the desktop and the page keep it.
        let img = s.flash_image();
        let mut t = SimCore::with_flash(1, false, Stored::Image(img));
        run(&mut t, 0, 2500);
        assert_eq!(t.app.settings().layout, layout);
    }

    #[test]
    fn a_blank_flash_runs_setup() {
        let mut s = SimCore::with_flash(1, false, Stored::Blank);
        run(&mut s, 0, 2500);
        assert!(matches!(s.app.screen(), AppScreen::Setup(_)), "{}", s.screen_name());
        assert_eq!(s.store.count(), 0);
    }
}
