//! The simulator for the browser: [`SimCore`] behind a plain C ABI (`extern "C"`, numbers in and out), so a page
//! loads the wasm with `WebAssembly.instantiate` and reads frames and strings straight out of linear memory. No
//! wasm-bindgen. The API, the key codes and the event ids are in `sim-web/README.md`.
//!
//! One simulator per instance; every call before [`init`] does nothing (getters return 0).

use ferrite_core::keys::Key;
use ferrite_sim::simcore::{jump_names, Event, SimCore, KEYMAP};
use std::cell::RefCell;

struct Web {
    sim: SimCore,
    /// The last frame as RGBA8, `width() * height() * 4` bytes, reused.
    rgba: Vec<u8>,
    /// The last string a getter returned (UTF-8), `text_len()` bytes.
    text: Vec<u8>,
    flash: bool,
}

thread_local! {
    static WEB: RefCell<Option<Web>> = const { RefCell::new(None) };
}

fn with<R: Default>(f: impl FnOnce(&mut Web) -> R) -> R {
    WEB.with(|w| w.borrow_mut().as_mut().map(f).unwrap_or_default())
}

/// A string getter: copies `s` into the text buffer and returns its address (length: [`text_len`]).
fn text(w: &mut Web, s: &str) -> usize {
    w.text.clear();
    w.text.extend_from_slice(s.as_bytes());
    w.text.as_ptr() as usize
}

/// The logical key for a page key code (README table).
fn key_of(code: u32) -> Option<Key> {
    Some(match code {
        0..=9 => Key::Digit(code as u8),
        10 => Key::Star,
        11 => Key::Hash,
        12 => Key::Menu,
        13 => Key::Slot,
        14 => Key::VfoMem,
        15 => Key::Exit,
        16 => Key::Up,
        17 => Key::Down,
        18 => Key::Left,
        19 => Key::Right,
        20 => Key::Side(1),
        21 => Key::Side(2),
        22 => Key::Ptt(0),
        23 => Key::Ptt(1),
        24 => Key::Ptt(2),
        25 => Key::PTT,
        _ => return None,
    })
}

fn event_of(id: u32) -> Option<Event> {
    use Event::*;
    [Message, Alert, Gps, Bluetooth, Battery, PowerSwitch, EmptyBattery, SsbPatch, LinkWrite, Crossband, Layout, Beacon]
        .get(id as usize)
        .copied()
}

/// Switches the radio on: the boot screen, the panel at whole `scale` (1-6); `owner_card` != 0 boots with the
/// sample owner card. Calling it again starts over.
#[no_mangle]
pub extern "C" fn init(scale: u32, owner_card: u32) {
    let sim = SimCore::new(scale as usize, owner_card != 0);
    WEB.with(|w| *w.borrow_mut() = Some(Web { sim, rgba: Vec::new(), text: Vec::new(), flash: false }));
}

/// The status bar clock: UTC now, seconds since 1970 (`Date.now() / 1000`).
#[no_mangle]
pub extern "C" fn set_clock(utc_secs: u32) {
    with(|w| w.sim.set_clock(utc_secs as u64))
}

/// A new whole scale (1-6); read `width()` / `height()` again after.
#[no_mangle]
pub extern "C" fn set_scale(scale: u32) {
    with(|w| w.sim.set_scale(scale as usize))
}

#[no_mangle]
pub extern "C" fn scale() -> u32 {
    with(|w| w.sim.scale() as u32)
}

/// The panel's width in pixels at the current scale.
#[no_mangle]
pub extern "C" fn width() -> u32 {
    with(|w| w.sim.size().0 as u32)
}

#[no_mangle]
pub extern "C" fn height() -> u32 {
    with(|w| w.sim.size().1 as u32)
}

/// The screen's top-left inside the panel (for a mock-up overlay drawn by the page), at the current scale.
#[no_mangle]
pub extern "C" fn screen_x() -> u32 {
    with(|w| w.sim.screen_origin().0 as u32)
}

#[no_mangle]
pub extern "C" fn screen_y() -> u32 {
    with(|w| w.sim.screen_origin().1 as u32)
}

/// Outlines each region as it is redrawn, for a quarter second (0 off, 1 on).
#[no_mangle]
pub extern "C" fn set_flash(on: u32) {
    with(|w| w.flash = on != 0)
}

/// Advances to `now_ms` (since start; never going back) and draws: returns the address of the frame, RGBA8,
/// `width() * height() * 4` bytes, valid until the next call.
#[no_mangle]
pub extern "C" fn frame(now_ms: u32) -> usize {
    with(|w| {
        w.sim.step(now_ms);
        let px = w.sim.compose(w.flash, None);
        w.rgba.resize(px.len() * 4, 0);
        for (o, &c) in w.rgba.as_chunks_mut::<4>().0.iter_mut().zip(px) {
            o.copy_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]);
        }
        w.rgba.as_ptr() as usize
    })
}

/// The mouse (or a touch) at panel pixel (x, y), button down (1) or up (0). Send every change; down on a key is
/// its key down until the button comes up, down on the knob steps it, down on VOL is the power switch.
#[no_mangle]
pub extern "C" fn mouse(x: i32, y: i32, down: u32) {
    with(|w| w.sim.mouse(x, y, down != 0))
}

/// A key down (1) or up (0), by code (README). Long presses are the app's: hold for 0.7 s.
#[no_mangle]
pub extern "C" fn key(code: u32, down: u32) {
    if let Some(k) = key_of(code) {
        with(|w| w.sim.key(k, down != 0))
    }
}

/// The channel knob: -1 one step down, +1 one step up.
#[no_mangle]
pub extern "C" fn knob(d: i32) {
    with(|w| w.sim.knob(if d < 0 { -1 } else { 1 }))
}

/// One of the simulator's events, by id (README).
#[no_mangle]
pub extern "C" fn event(id: u32) {
    if let Some(e) = event_of(id) {
        with(|w| w.sim.event(e))
    }
}

/// How many screens `jump` offers (home first).
#[no_mangle]
pub extern "C" fn dest_count() -> u32 {
    jump_names().count() as u32
}

/// Screen `i`'s name with its inventory ID ("B4 Shortcut map"; 0 is "Home"): a string address, `text_len()` bytes.
#[no_mangle]
pub extern "C" fn dest_name(i: u32) -> usize {
    with(|w| {
        let n = jump_names().nth(i as usize).unwrap_or_default();
        text(w, &n)
    })
}

/// Opens screen `i` from home (BACK returns there). Ignored while booting or off.
#[no_mangle]
pub extern "C" fn jump(i: u32) {
    with(|w| w.sim.jump(i as usize))
}

/// The screen on show ("Boot", "Off", or the screen's name): a string address, `text_len()` bytes.
#[no_mangle]
pub extern "C" fn screen_name() -> usize {
    with(|w| {
        let s = w.sim.screen_name();
        text(w, s)
    })
}

/// The look-review mock-up file for the screen on show (`docs/img/screens/<name>`).
#[no_mangle]
pub extern "C" fn mockup_name() -> usize {
    with(|w| {
        let s = w.sim.mockup_name();
        text(w, s)
    })
}

/// One line of state: screen, last key, TX, active slot and frequency, last redraw's pixels, lock.
#[no_mangle]
pub extern "C" fn status_text() -> usize {
    with(|w| {
        let s = w.sim.status();
        text(w, &s)
    })
}

/// The desktop simulator's keyboard map, as text.
#[no_mangle]
pub extern "C" fn keymap() -> usize {
    with(|w| text(w, KEYMAP))
}

/// The length in bytes of the string the last string getter returned.
#[no_mangle]
pub extern "C" fn text_len() -> u32 {
    with(|w| w.text.len() as u32)
}
