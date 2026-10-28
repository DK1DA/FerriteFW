//! The screens' less-travelled paths as host tests, on the sample records: every page and option drawn, each
//! key's branch taken, checked by what the readback (LINK-19) says and what the drawing shows.

use crate::sample::Sample;
use crate::screens::nav::Dest;
use crate::screens::Screen;
use crate::App;
use ferrite_core::keys::Key;
use ferrite_core::msg::{Device, GpsState, LiveAll, RadioCmd};
use ferrite_ui::fb::FrameBuffer;
use std::string::String;
use std::vec::Vec;

mod home;
mod msgs;
mod setup;
mod transmit;

/// An app on the sample records, a clock, a quiet radio and a panel.
pub(super) struct T {
    pub app: App,
    pub t: u32,
    pub s: Sample,
    pub fb: FrameBuffer,
    pub live: LiveAll,
    pub dev: Device,
}

#[allow(dead_code)]
impl T {
    pub fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        let dev = Device { gps: GpsState::Fix, sats: 9, clock_min: Some(14 * 60 + 32), battery_pct: 80, ..Default::default() };
        T { app, t: 1000, s: Sample::new(), fb: FrameBuffer::new(), live: LiveAll::default(), dev }
    }

    pub fn press(&mut self, k: Key, hold: u32) {
        self.app.key_down(k, self.t, &self.s);
        let end = self.t + hold;
        while self.t < end {
            self.t += 10;
            self.app.tick(self.t, &self.live, &self.s);
        }
        self.app.key_up(k, self.t, &self.s);
        self.t += 50;
        self.app.tick(self.t, &self.live, &self.s);
        self.apply();
        self.draw();
    }

    pub fn tap(&mut self, k: Key) {
        self.press(k, 100);
    }

    pub fn taps(&mut self, ks: &[Key]) {
        for &k in ks {
            self.tap(k);
        }
    }

    pub fn hold(&mut self, k: Key) {
        self.press(k, 800);
    }

    /// A key down (or up) alone, then a tick.
    pub fn down(&mut self, k: Key, down: bool) {
        if down {
            self.app.key_down(k, self.t, &self.s);
        } else {
            self.app.key_up(k, self.t, &self.s);
        }
        self.t += 50;
        self.app.tick(self.t, &self.live, &self.s);
        self.apply();
        self.draw();
    }

    /// The active PTT down or up.
    pub fn ptt(&mut self, down: bool) {
        self.down(Key::PTT, down);
    }

    /// An app that can transmit (the tx build, the US region).
    pub fn ready() -> Self {
        let mut t = T::new();
        t.app.radio.tx_built = true;
        t.dev.tx_built = true;
        t.app.set_region(Some(ferrite_core::txplan::Region::Us), false);
        t
    }

    /// Tune the active slot's VFO.
    pub fn tune(&mut self, f: ferrite_core::freq::Freq10) {
        let a = self.app.active as usize;
        self.app.slots[a].vfo = f;
        self.app.slots[a].vfo_mode = true;
    }

    /// The toast's head and body, or "".
    pub fn toast(&self) -> String {
        self.app.toast.map(|x| std::format!("{} / {}", String::from_utf8_lossy(x.head.as_bytes()), String::from_utf8_lossy(x.body.as_bytes()))).unwrap_or_default()
    }

    pub fn knob(&mut self, d: i8) {
        self.app.knob(d, self.t, &self.s);
        self.apply();
        self.draw();
    }

    pub fn wait(&mut self, ms: u32) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 100;
            self.app.tick(self.t, &self.live, &self.s);
        }
        self.draw();
    }

    pub fn open(&mut self, d: Dest) {
        self.app.open(d);
        self.t += 50;
        self.app.tick(self.t, &self.live, &self.s);
        self.draw();
    }

    /// The channel writes asked for, applied to the sample records (as the store task does).
    pub fn apply(&mut self) {
        while let Some(w) = self.app.pop_write() {
            self.s.apply(w);
        }
    }

    /// Draws, as the UI task does after every input (renders must never panic).
    pub fn draw(&mut self) {
        self.app.render(&mut self.fb, &self.live, &self.dev, &self.s);
    }

    pub fn name(&self) -> &'static str {
        self.app.screen().name()
    }

    pub fn screen(&self) -> Screen {
        self.app.screen()
    }

    pub fn cmds(&mut self) -> Vec<RadioCmd> {
        let mut v = Vec::new();
        while let Some(c) = self.app.pop_cmd() {
            v.push(c);
        }
        v
    }

    /// The readback's lines (LINK-19), as the link's `UI` gives them.
    pub fn readback(&self) -> Vec<String> {
        let mut l = crate::term::cmd::Line::new();
        (0..)
            .map_while(|i| crate::uistate::line(&self.app, &self.s, &self.dev, &self.live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned()))
            .collect()
    }

    /// The readback in one string, to look for a line or a value in.
    pub fn ui(&self) -> String {
        self.readback().join("\n")
    }

    /// The texts the screen draws, all of it drawn again (as LINK-19.5's readback does), in drawing order, joined
    /// by spaces (a wrapped paragraph reads as one).
    pub fn texts(&mut self) -> String {
        let mut rec = Texts(Vec::new());
        self.app.render_texts(&mut rec, &self.live, &self.dev, &self.s);
        rec.0.join(" ")
    }

    /// Asserts the screen draws `text` somewhere.
    #[track_caller]
    pub fn shows(&mut self, text: &str) {
        let d = self.texts();
        assert!(d.contains(text), "{text:?} not drawn on {}:\n{d}", self.name());
    }

    /// Asserts the screen doesn't draw `text`.
    #[track_caller]
    pub fn hides(&mut self, text: &str) {
        let d = self.texts();
        assert!(!d.contains(text), "{text:?} drawn on {}:\n{d}", self.name());
    }

    /// Asserts the readback has `text` somewhere.
    #[track_caller]
    pub fn reads(&self, text: &str) {
        let d = self.ui();
        assert!(d.contains(text), "{text:?} not in the readback of {}:\n{d}", self.name());
    }

    /// Pixels of colour `c` drawn between rows `y0` and `y1`.
    pub fn px(&self, y0: i32, y1: i32, c: u16) -> usize {
        (y0..y1).flat_map(|y| (0..240).map(move |x| (x, y))).filter(|&(x, y)| self.fb.at(x, y) == c).count()
    }
}

/// A panel that keeps only the texts drawn on it.
struct Texts(Vec<String>);

impl ferrite_ui::panel::Panel for Texts {
    fn window(&mut self, _: i32, _: i32, _: i32, _: i32) {}
    fn push(&mut self, _: u16, _: u32) {}
    fn push_buf(&mut self, _: &[u16]) {}
    fn text_hint(&mut self, _: i32, _: i32, _: i32, _: i32, s: &[u8]) {
        self.0.push(String::from_utf8_lossy(s).into_owned());
    }
    fn offscreen(&self) -> bool {
        true
    }
    fn text_only(&self) -> bool {
        true
    }
}
