//! # ferrite-app
//!
//! The UI state machine, the same on the radio and in the simulator. In: key down/up ([`App::key_down`],
//! [`App::key_up`]), knob steps ([`App::knob`]), the time ([`App::tick`]) and the Radio task's snapshot of the
//! three slots ([`LiveAll`]). Out: commands for the Radio task ([`App::pop_cmd`]), channel writes for the store's
//! owner ([`App::pop_write`]) and the screen, drawn through any [`Panel`] by [`App::render`], which redraws only
//! what changed (HOME-7).
//!
//! No heap and fixed-size state (`size_of::<App>()` is checked by a test). Channels come through
//! [`chan::ChannelStore`], read on demand.
//!
//! Screens live in [`screens`], one module each, plugged in through one registry ([`screens::Screen`]); how to
//! add one is in that module's documentation, and how each is reached in [`screens::nav`]. The App itself holds
//! what every screen shares: the three slots, the radio-wide values, the toast, undo, the key tracker and the
//! commands for the Radio task. Rules are cited by their manual IDs.

#![no_std]

#[cfg(test)]
extern crate std;

/// A string literal kept with the fonts in the slow flash section on the radio (`.slow_rodata`, placed after all
/// code and other constants: `firmware/memory.x`), for long tables of text read a row at a time (the menu's
/// items and help lines), so they don't take the zero-wait flash the code needs. On the PC an ordinary static.
macro_rules! slow {
    ($s:literal) => {{
        const B: &[u8] = $s.as_bytes();
        #[cfg_attr(target_os = "none", link_section = ".slow_rodata")]
        static S: [u8; B.len()] = {
            let mut a = [0u8; B.len()];
            let mut i = 0;
            while i < B.len() {
                a[i] = B[i];
                i += 1;
            }
            a
        };
        // SAFETY: the bytes are a string literal's, so UTF-8.
        unsafe { core::str::from_utf8_unchecked(&S) }
    }};
}

pub mod chan;
pub mod data;
pub mod keys;
pub mod persist;
pub mod sample;
pub mod screens;
pub mod settings;
pub mod slot;
pub mod words;
pub mod term;

use chan::{ChanWrite, Channel, ChannelStore};
use screens::alert::AlertInfo;
use settings::Settings;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_core::msg::{LiveAll, Param, RadioCmd, SLOTS};
use ferrite_core::rxsleep::Level;
use ferrite_ui::text::Buf;
use keys::{KeyEvent, KeyTracker, Press};
use screens::nav::Dest;
use screens::shared::REGIONS;
use slot::{band_near, Item, SlotCfg, BANDS, BAND_125M, DEFAULT_CFG, SQ_GLOBAL};

pub use ferrite_ui::panel::Panel;
pub use screens::Screen;

/// Toasts show for 3 s (HOME-9); undo is offered for 5 s (EDIT-4, MENU-22).
pub const TOAST_MS: u32 = 3000;
/// A hold-digit shortcut's popup (HOME-8).
pub const POPUP_MS: u32 = 1200;
pub const UNDO_MS: u32 = 5000;
/// Edit in place ends after 8 s idle (EDIT-6); menus go home after the menu timeout (MENU-28, 0 7 9: 30 s).
pub const EDIT_IDLE_MS: u32 = 8000;
pub const MENU_TIMEOUT_MS: u32 = 30_000;
/// Watch pause (0 1 2 5, default 1 min).
pub const PAUSE_S: u16 = 60;

/// One slot: a VFO and a channel position; which one it is on.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub vfo_mode: bool,
    pub vfo: Freq10,
    /// The VFO's settings.
    pub cfg: SlotCfg,
    /// The VFO's last frequency on each band (VFO-1).
    pub mem: [Freq10; BANDS.len()],
    pub zone: u8,
    /// Position in the zone.
    pub pos: u16,
    /// A copy of the channel it is on, edits included.
    pub ch: Option<Channel>,
}

impl Slot {
    fn new(f: Freq10) -> Self {
        let mut mem = [0; BANDS.len()];
        for (m, b) in mem.iter_mut().zip(&BANDS) {
            *m = b.start;
        }
        let mut cfg = DEFAULT_CFG;
        let b = &BANDS[band_near(f)];
        cfg.mode = b.mode;
        cfg.step = b.step;
        Slot { vfo_mode: true, vfo: f, cfg, mem, zone: 0, pos: 0, ch: None }
    }

    pub fn on_channel(&self) -> bool {
        !self.vfo_mode && self.ch.is_some()
    }

    pub fn freq(&self) -> Freq10 {
        match self.ch {
            Some(c) if !self.vfo_mode => c.rx,
            _ => self.vfo,
        }
    }

    pub fn cfg(&self) -> &SlotCfg {
        match &self.ch {
            Some(c) if !self.vfo_mode => &c.cfg,
            _ => &self.cfg,
        }
    }

    fn cfg_mut(&mut self) -> &mut SlotCfg {
        match &mut self.ch {
            Some(c) if !self.vfo_mode => &mut c.cfg,
            _ => &mut self.cfg,
        }
    }
}

#[derive(Clone, Copy)]
struct Toast {
    head: Buf<20>,
    body: Buf<28>,
    icon: bool,
    /// A shortcut's popup in the middle of the screen (HOME-8), not over the hint bar.
    center: bool,
    until: u32,
}

/// What a change replaced, for "* Undo".
#[derive(Clone, Copy)]
struct Undo {
    slot: u8,
    cfg: SlotCfg,
    /// The channel's own fields as they were, when the slot is on one (its zone, number, frequency and flags
    /// are edited too; a name is kept or dropped in the text entry).
    ch: Option<ChUndo>,
    home: Option<u16>,
    watch: u8,
    bsave: u8,
    until: u32,
}

#[derive(Clone, Copy)]
struct ChUndo {
    num: u16,
    zone: u8,
    flags: u8,
    rx: Freq10,
}

/// Commands waiting for the Radio task: single commands in a ring, and whole slots to send, which are made
/// from the slot when popped. A slot's frequency and nine settings are ten commands; sending a slot again
/// before it has gone out costs nothing, so start-up (three slots) and fast channel stepping never overflow
/// the ring.
struct Outbox {
    q: [Option<RadioCmd>; 16],
    head: u8,
    len: u8,
    /// Slots whose full state is still to send, a bit each.
    sync: u8,
    /// The slot being sent and its next item (0 the frequency, 1-9 [`slot::params`]).
    cur: u8,
    step: u8,
}

impl Outbox {
    fn push(&mut self, c: RadioCmd) {
        if self.len as usize == self.q.len() {
            self.pop(); // full: the oldest goes (the caller drains after every input, so this is a backstop)
        }
        let i = (self.head as usize + self.len as usize) % self.q.len();
        self.q[i] = Some(c);
        self.len += 1;
    }

    /// The whole slot goes out (again, from the start if it is being sent).
    fn sync(&mut self, slot: u8) {
        self.sync |= 1 << slot;
        if self.cur == slot {
            self.step = 0;
        }
    }

    fn pop(&mut self) -> Option<RadioCmd> {
        if self.len == 0 {
            return None;
        }
        let c = self.q[self.head as usize].take();
        self.head = ((self.head as usize + 1) % self.q.len()) as u8;
        self.len -= 1;
        c
    }
}

/// What is on the screen now, so render can skip what hasn't changed.
pub(crate) struct Drawn {
    /// Which screen the content area shows (its variant; a screen whose state changes keeps its page).
    pub screen: Option<core::mem::Discriminant<Screen>>,
    pub sbar: Option<ferrite_ui::widgets::StatusBar>,
    /// One hash per region; 0 = not drawn.
    pub hash: [u32; REGIONS],
    /// Each card's meter: lit segments and whether squelch was open.
    pub meter: [Option<(u8, bool)>; SLOTS],
    /// A popup just went: clear its box and redraw the screen's regions under it.
    pub popup_gone: bool,
}

pub struct App {
    screen: Screen,
    slots: [Slot; SLOTS],
    active: u8,
    /// The slot the PTT being pressed keys (`Key::Ptt`); `PTT_ACTIVE` for the side connector's.
    pub(crate) ptt_slot: u8,
    /// Index into Watch's values: off, A+B, A+B+C.
    watch: u8,
    /// Battery Save: 0 off, 1-3.
    bsave: u8,
    locked: bool,
    inbox: u8,
    alert: bool,
    toast: Option<Toast>,
    undo: Option<Undo>,
    /// The last settings changed, newest first (MENU-8).
    recent: [Option<Item>; 3],
    keys: KeyTracker,
    now: u32,
    last_input: u32,
    paused_until: Option<u32>,
    /// From the last snapshot: slots receiving, scanning, skipped by the relay rule.
    rx_mask: u8,
    /// Slots whose own RX tone is heard on the carrier (busy lockout "wrong tone").
    tone_mask: u8,
    scan_mask: u8,
    relay_mask: u8,
    out: Outbox,
    /// The channel write for the store's owner ([`App::pop_write`]): one, as every input makes at most one and
    /// the owner drains after every input.
    write: Option<ChanWrite>,
    /// The text being typed (A7): a channel name, a callsign; shared by the text entry and the forms that
    /// open it, as only one text is edited at a time.
    text: Buf<16>,
    /// Where BACK returns from screens opened with [`App::open`], newest last (MENU-24).
    back: [Option<Screen>; BACK_DEPTH],
    pub(crate) drawn: Drawn,
    /// Radio activity: transmit, the exclusive modes, scan hits, region ([`screens::activity`]).
    pub(crate) radio: screens::activity::RadioUi,
    /// Radio-wide settings: not the App's, only borrowed ([`settings`]).
    set: &'static mut Settings,
    /// The last alert, for the alert screen (WX-5): kept outside the App too, as it outlives the screen.
    pub(crate) alert_info: &'static mut AlertInfo,
    /// The user's answer to a link write approval, for the Link task.
    link_answer: Option<screens::link::LinkAnswer>,
}

/// Screens remembered for BACK: menu, group, item and one more.
pub(crate) const BACK_DEPTH: usize = 4;

/// [`Outbox::cur`] with no slot being sent.
const NO_SLOT: u8 = 0xFF;

impl App {
    /// Three slots: A on 146.520, B on 443.475, C on 118.950 (the look review's sample), all VFO. The settings
    /// (stored, or [`Settings::DEFAULT`] on a first boot) and the last alert record ([`AlertInfo::NONE`]) are
    /// the caller's, borrowed for the App's lifetime: the App holds only UI state.
    pub fn new(set: &'static mut Settings, alert: &'static mut AlertInfo) -> Self {
        let mut app = App {
            screen: Screen::Home,
            slots: [Slot::new(mhz(146) + 52_000), Slot::new(mhz(443) + 47_500), Slot::new(mhz(118) + 95_000)],
            active: 0,
            ptt_slot: ferrite_core::keys::PTT_ACTIVE,
            watch: 1,
            bsave: 1,
            locked: false,
            inbox: 0,
            alert: false,
            toast: None,
            undo: None,
            recent: [None; 3],
            keys: KeyTracker::new(),
            now: 0,
            last_input: 0,
            paused_until: None,
            rx_mask: 0,
            tone_mask: 0,
            scan_mask: 0,
            relay_mask: 0,
            out: Outbox { q: [None; 16], head: 0, len: 0, sync: 0, cur: NO_SLOT, step: 0 },
            write: None,
            text: Buf::new(),
            back: [None; BACK_DEPTH],
            drawn: Drawn { screen: None, sbar: None, hash: [0; REGIONS], meter: [None; SLOTS], popup_gone: false },
            radio: screens::activity::RadioUi::new(),
            set,
            alert_info: alert,
            link_answer: None,
        };
        app.region_steps(); // VFO-18: the region's step on 2 m and 1.25 m (saved slots bring their own)
        for s in 0..SLOTS as u8 {
            app.send_slot(s);
        }
        app.out.push(RadioCmd::Active(0));
        app.out.push(RadioCmd::Watch(app.watch_mask()));
        app.out.push(RadioCmd::BatterySave(level(app.bsave)));
        app
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn active(&self) -> u8 {
        self.active
    }

    pub fn slot(&self, i: u8) -> &Slot {
        &self.slots[i as usize % SLOTS]
    }

    /// A toast is showing.
    pub fn toast_up(&self) -> bool {
        self.toast.is_some()
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Off, A+B, A+B+C.
    pub fn watch(&self) -> u8 {
        self.watch
    }

    /// The next command for the Radio task: single commands first, then whole slots, made from the slot now
    /// (so they always carry its latest state).
    pub fn pop_cmd(&mut self) -> Option<RadioCmd> {
        if let Some(c) = self.out.pop() {
            return Some(c);
        }
        if self.out.sync == 0 {
            return None;
        }
        if self.out.cur == NO_SLOT {
            (self.out.cur, self.out.step) = (self.out.sync.trailing_zeros() as u8, 0);
        }
        let slot = self.out.cur;
        let s = &self.slots[slot as usize];
        let c = match self.out.step {
            0 => RadioCmd::TuneSlot { slot, freq: s.freq() },
            k => {
                let (param, value) = slot::params(s.cfg())[k as usize - 1];
                let value = if param == Param::Power && self.capped(slot) { 0 } else { value };
                let value = if param == Param::Squelch { self.sq_level(slot) as i32 } else { value };
                RadioCmd::SlotParam { slot, param, value }
            }
        };
        self.out.step += 1;
        if self.out.step as usize > slot::params(&DEFAULT_CFG).len() {
            self.out.sync &= !(1 << slot);
            self.out.cur = NO_SLOT;
        }
        Some(c)
    }

    /// The next channel write for the store's owner (the Storage task; the simulator's store). Drained, like
    /// [`App::pop_cmd`], after every input.
    pub fn pop_write(&mut self) -> Option<ChanWrite> {
        self.write.take()
    }

    /// Queues a write; a later write of the same channel replaces it (keeping where it came from).
    pub(crate) fn write(&mut self, w: ChanWrite) {
        if let (Some(ChanWrite::Put { ch: c, from: f }), ChanWrite::Put { ch, from }) = (self.write, w) {
            if c.num == ch.num || Some(c.num) == from {
                self.write = Some(ChanWrite::Put { ch, from: f.or(from).filter(|&n| n != ch.num) });
                return;
            }
        }
        debug_assert!(self.write.is_none(), "one channel write per input: {:?} then {:?}", self.write, w);
        self.write = Some(w);
    }

    /// A channel changed outside the radio's own editor (the terminal: TERM-9): saved as an edit there is, and
    /// every slot on it follows at once.
    pub(crate) fn put_channel(&mut self, c: chan::Channel) {
        self.write(ChanWrite::Put { ch: c, from: None });
        for i in 0..SLOTS {
            let s = &mut self.slots[i];
            if s.ch.is_some_and(|o| o.num == c.num) {
                let moved = s.ch.is_some_and(|o| o.rx != c.rx || o.cfg != c.cfg);
                (s.ch, s.zone) = (Some(c), c.zone);
                if moved && s.on_channel() {
                    self.send_slot(i as u8);
                }
            }
        }
    }

    /// A channel given another number by the terminal (TERM-14): the record moves, and slots on it and the home
    /// channel go with it (CHAN-18), as the radio's editor does (EDIT-5).
    pub(crate) fn renumber_channel(&mut self, c: chan::Channel, to: u16) {
        let n = chan::Channel { num: to, ..c };
        self.write(ChanWrite::Put { ch: n, from: Some(c.num) });
        for s in self.slots.iter_mut() {
            if s.ch.is_some_and(|o| o.num == c.num) {
                s.ch = Some(n);
            }
        }
        if self.set.home_ch == Some(c.num) {
            self.set.home_ch = Some(to);
        }
    }

    /// A channel write is waiting for [`App::pop_write`] (the terminal runs its next line only after it).
    pub fn write_pending(&self) -> bool {
        self.write.is_some()
    }

    /// Deletes a channel (CHAN-13): slots on it go to VFO, and it stops being the home channel (CHAN-18).
    pub(crate) fn delete_channel(&mut self, num: u16, store: &impl chan::ChannelStore) {
        if !store.data().request(data::Op::DeleteChannel(num)) {
            self.write(ChanWrite::Delete(num)); // a store whose records have no Data owner yet
        }
        for s in 0..SLOTS {
            if self.slots[s].ch.is_some_and(|c| c.num == num) {
                let sl = &mut self.slots[s];
                (sl.ch, sl.vfo_mode) = (None, true);
                self.send_slot(s as u8);
            }
        }
        if self.set.home_ch == Some(num) {
            self.set.home_ch = None;
        }
    }

    /// Deletes every channel (the reset's second step, PIN-6; the terminal's `clear channels`): every slot to VFO.
    pub(crate) fn erase_channels(&mut self) {
        self.write(ChanWrite::EraseAll);
        for s in self.slots.iter_mut() {
            (s.ch, s.vfo_mode, s.zone, s.pos) = (None, true, 0, 0);
        }
        for s in 0..SLOTS as u8 {
            self.send_slot(s);
        }
        self.set.home_ch = None;
    }

    /// The channel the active slot is on was changed (a setting, its zone, number, name): save it (EDIT-4).
    pub(crate) fn save_channel(&mut self, slot: u8, from: Option<u16>) {
        if let Some(ch) = self.slots[slot as usize].ch.filter(|_| self.slots[slot as usize].on_channel()) {
            self.write(ChanWrite::Put { ch, from });
        }
    }

    /// Put a slot on a channel (setup, or the simulator's sample): channel mode at that zone position.
    pub fn set_channel(&mut self, slot: u8, zone: u8, pos: u16, store: &impl ChannelStore) {
        let s = &mut self.slots[slot as usize % SLOTS];
        if let Some(c) = store.zone_at(zone, pos) {
            (s.zone, s.pos, s.ch, s.vfo_mode) = (zone, pos, Some(c), false);
            self.send_slot(slot);
        }
    }

    /// Watch: 0 off, 1 A+B, 2 A+B+C (setup, or the simulator's sample).
    pub fn set_watch(&mut self, v: u8) {
        self.watch = v % 3;
        self.out.push(RadioCmd::Watch(self.watch_mask()));
    }

    /// A slot's VFO settings (setup, or the simulator's sample).
    pub fn set_vfo_cfg(&mut self, slot: u8, cfg: SlotCfg) {
        self.slots[slot as usize % SLOTS].cfg = cfg;
        self.send_slot(slot % SLOTS as u8);
    }

    /// Everything is drawn again on the next render.
    pub fn redraw_all(&mut self) {
        self.drawn.screen = None;
        self.drawn.sbar = None;
    }

    /// A message, decode or alert arrived: a toast and the inbox count (SBAR-6); an alert also opens the alert
    /// screen, with `body` as its event and `head` as its originator.
    pub fn notify(&mut self, head: &[u8], body: &[u8], alert: bool) {
        self.inbox = self.inbox.saturating_add(1);
        self.alert |= alert;
        self.show_toast(head, body, true);
        if alert {
            let mut a = screens::alert::AlertInfo::NONE;
            a.event.set(body);
            a.from.set(head);
            *self.alert_info = a;
            self.open(Dest::Alert); // WX-4: over any screen
        }
    }

    /// An alert with everything the decoder knows (WX-5): over any screen, flashing, until OK (WX-4, WX-6).
    pub fn raise_alert(&mut self, a: &screens::alert::AlertInfo) {
        self.inbox = self.inbox.saturating_add(1);
        self.alert = true;
        *self.alert_info = *a;
        self.open(Dest::Alert);
    }

    /// A SAME header decoded on a slot (WX-4, WX-5): the alert screen over any screen, with its event, areas,
    /// issue and expiry (local time), originator, slot and frequency. Tests (RWT, RMT) only toast and count in
    /// the inbox. Nothing happens with SAME alerts off or outside the US and Canada (WX-3). Returns whether the
    /// full-screen alert went up. The decoder (DSP task) hands headers to the UI task, which calls this.
    pub fn same_alert(&mut self, h: &ferrite_core::same::SameHeader, slot: u8, freq: Freq10) -> bool {
        use ferrite_core::same;
        if !self.set.same_on() {
            return false;
        }
        let mut name = Buf::<30>::new();
        match same::event_name(&h.event) {
            Some(n) => name.set(n.as_bytes()),
            None => {
                name.set(b"Alert ");
                name.push(&h.event);
            }
        }
        if same::is_test(&h.event) {
            self.inbox = self.inbox.saturating_add(1);
            self.show_toast(b"SAME test", name.as_bytes(), true);
            return false;
        }
        let mut a = AlertInfo::NONE;
        a.event.set(name.as_bytes());
        let mut areas = Buf::<20>::new();
        let _ = ufmt::uwrite!(areas, "{}", ferrite_ui::text::Zero(h.areas[0], 6));
        if h.n_areas > 1 {
            let _ = ufmt::uwrite!(areas, " +{}", h.n_areas - 1);
        }
        a.areas.set(areas.as_bytes());
        let sender = h.sender.iter().rposition(|&c| c != b' ').map_or(&h.sender[..0], |n| &h.sender[..=n]);
        a.from.set(if sender.is_empty() { same::org_name(&h.org).as_bytes() } else { sender });
        let tz = settings::TIME_ZONES[self.set.tz as usize % settings::TIME_ZONES.len()] as i32 * 15;
        a.issued = (h.issued_min as i32 + tz).rem_euclid(1440) as u16;
        a.until = (a.issued as u32 + h.purge_min as u32).rem_euclid(1440) as u16;
        (a.slot, a.freq) = (slot, freq);
        self.raise_alert(&a);
        true
    }

    /// First boot (no layout header: BOOT-12): setup runs, and BACK on its first page has nowhere to go.
    pub fn first_boot(&mut self) {
        self.go_home();
        self.screen = Screen::Setup(screens::setup::Setup::new(true));
    }

    /// At power on, after the self-test: the PIN, if one is set (PIN-1, BOOT-13).
    pub fn ask_pin(&mut self) {
        if self.set.pin_len > 0 && self.set.pin_mode > 0 {
            self.go_home();
            self.screen = Screen::Pin(screens::pin::Pin::power_on());
        }
    }

    // ---- events the firmware (and the simulator) report: each opens its screen ----

    /// The volume knob turned to off (the power switch, PWR-17): "Saving...", modal, while the writes finish;
    /// then the firmware cuts the power. Nothing else happens on screen after this.
    pub fn power_off(&mut self) {
        self.go_home();
        self.screen = Screen::Info(screens::info::Info { page: screens::info::Page::PowerOff });
    }

    /// The battery reached empty (PWR-16): the empty-battery screen, modal, before the firmware switches off.
    pub fn battery_empty(&mut self) {
        self.go_home();
        self.screen = Screen::Info(screens::info::Info { page: screens::info::Page::EmptyBattery });
    }

    /// At boot, an SSB patch in flash that matches no known stock version (D15): ask whether to use it,
    /// over whatever is up (after the PIN, if any).
    pub fn ssb_patch_found(&mut self) {
        self.open(Dest::SsbConfirm);
    }

    /// The Link task: the first write from a device not approved yet (LINK-7, SAFE-6). The answer comes back
    /// through [`App::take_link_answer`].
    pub fn link_write_request(&mut self) {
        if !matches!(self.screen, Screen::Link(l) if l.page == screens::link::LinkPage::Approve) {
            self.open(Dest::WriteApproval);
        }
    }

    /// The Link task: a write session is running (LINK-9): its progress screen (the percentage is the
    /// Device's `link.write_pct`), keys ignored but BACK, which cancels.
    pub fn link_write_progress(&mut self) {
        if !matches!(self.screen, Screen::Link(l) if l.page == screens::link::LinkPage::Progress) {
            self.open(Dest::WriteProgress);
        }
    }

    /// The Link task: the write session ended: back to where it was.
    pub fn link_write_done(&mut self) {
        if matches!(self.screen, Screen::Link(l) if l.page == screens::link::LinkPage::Progress) {
            self.back();
            self.show_toast(b"Write done", b"", false);
        }
    }

    /// MENU held at power on (PIN-6, BOOT-14), after the self-test: the settings reset, asked twice (settings,
    /// then channels too). It clears the PIN, so it comes before it; BACK on its first question goes on to the
    /// PIN if one is set (else home). Calibration, stock data and the SSB patch are never touched.
    pub fn power_on_reset(&mut self) {
        self.go_home();
        if self.set.pin_len > 0 && self.set.pin_mode > 0 {
            self.screen = Screen::Pin(screens::pin::Pin::power_on());
            self.push_back();
        }
        let ask = screens::confirm::Ask::Reset { channels: false };
        self.screen = Screen::Confirm(screens::confirm::Confirm { ask, boot: true });
    }

    /// A matrix code went down or up, mapped to a key or not: the key test shows it (KEY-2). The firmware calls
    /// this for every code, before [`App::key_down`] / [`App::key_up`] for the mapped ones.
    pub fn raw_key(&mut self, code: u8, down: bool) {
        self.screen.raw(self, ferrite_core::keys::key(code), Some(code), down);
    }

    /// The answer to the last write approval or write session (allowed, refused, cancelled), once.
    pub fn take_link_answer(&mut self) -> Option<screens::link::LinkAnswer> {
        self.link_answer.take()
    }

    /// The callsign, for the boot screen (empty until set).
    pub fn callsign(&self) -> &[u8] {
        self.set.callsign.as_bytes()
    }

    /// The radio-wide settings.
    pub fn settings(&self) -> &Settings {
        self.set
    }

    /// The radio-wide settings, to change from outside (the simulator's sample, tests; the link once it
    /// writes them). The radio state that follows from them is sent again.
    pub fn settings_mut(&mut self) -> &mut Settings {
        self.set
    }

    // ---- inputs ----

    pub fn key_down(&mut self, key: Key, now: u32, store: &impl ChannelStore) {
        self.tick_keys(now, store);
        self.keys.down(key, now);
        self.last_input = now;
        self.out.push(RadioCmd::Activity);
        self.screen.raw(self, Some(key), None, true);
        if let Key::Ptt(slot) = key {
            // KEY-7: from any screen; works locked too (KEY-14). Each screen says what PTT does there. PTT A, B and
            // C key their own slot; the side connector's keys the active slot.
            self.ptt_slot = slot;
            self.screen.ptt(self, true);
        }
    }

    pub fn key_up(&mut self, key: Key, now: u32, store: &impl ChannelStore) {
        self.tick_keys(now, store);
        self.screen.raw(self, Some(key), None, false);
        if matches!(key, Key::Ptt(_)) {
            self.screen.ptt(self, false);
            self.ptt_released(); // TX-25: whatever the screen did, PTT up unkeys
        }
        match self.keys.release(key, now) {
            Some(e) if e.press == Press::Short => self.on_key(e, store),
            // The release after a long press: only the screen hears it (the shortcut map runs its shortcut).
            Some(e) if !self.locked => self.screen.key(self, e, store),
            _ => {}
        }
    }

    /// Knob: +1 per click clockwise.
    pub fn knob(&mut self, d: i8, now: u32, store: &impl ChannelStore) {
        self.tick_keys(now, store);
        self.last_input = now;
        self.out.push(RadioCmd::Activity);
        if self.locked {
            return self.locked_toast();
        }
        self.screen.knob(self, d as i32, store);
    }

    /// Time passes: long presses and repeats, toasts, undo, timeouts; and the latest snapshot.
    pub fn tick(&mut self, now: u32, live: &LiveAll, store: &impl ChannelStore) {
        self.tick_keys(now, store);
        let mut rx = 0;
        let mut tone = 0;
        let mut scan = 0;
        let mut relay = 0;
        for (i, s) in live.slots.iter().enumerate() {
            rx |= (s.squelch_open as u8) << i;
            tone |= (s.tone_heard as u8) << i;
            scan |= (s.scanning as u8) << i;
            relay |= (s.relay as u8) << i;
        }
        (self.rx_mask, self.tone_mask, self.scan_mask, self.relay_mask) = (rx, tone, scan, relay);
        if let Some(t) = self.toast.filter(|t| after(now, t.until)) {
            self.toast = None;
            self.drawn.popup_gone |= t.center;
        }
        if self.undo.is_some_and(|u| after(now, u.until)) {
            self.undo = None;
        }
        if self.paused_until.is_some_and(|t| after(now, t)) {
            self.paused_until = None;
        }
        if self.screen.timeout().is_some_and(|t| now.wrapping_sub(self.last_input) >= t) {
            self.go_home();
        }
        self.radio_tick(live);
        self.screen.tick(self, live);
        if self.screen == Screen::Home {
            self.home_stops_modes(); // however the screen went home
        }
    }

    fn tick_keys(&mut self, now: u32, store: &impl ChannelStore) {
        self.now = now;
        let mut evs = [None; 8];
        let mut n = 0;
        self.keys.tick(now, |e| {
            if n < evs.len() {
                evs[n] = Some(e);
                n += 1;
            }
        });
        for e in evs.into_iter().flatten() {
            self.last_input = now;
            self.on_key(e, store);
        }
    }

    // ---- what keys do ----

    fn on_key(&mut self, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        if self.screen.modal() {
            return self.screen.key(self, e, store); // the PIN at power on, power off, a link write
        }
        // One BACK rule (KEY-5): short BACK is the screen's, and goes back one level ([`App::back`]); hold BACK
        // goes home from anywhere but the modal screens. Hold # toggles the lock (KEY-13).
        match (e.key, e.press) {
            (Key::Exit, Long) => return self.go_home(),
            (Key::Hash, Long) if self.locked && self.set.pin_mode == 2 => {
                // KEY-16: unlocking asks for the PIN
                return self.open(Dest::Pin);
            }
            (Key::Hash, Long) => {
                self.locked = !self.locked;
                return self.show_toast(if self.locked { b"Keys locked" } else { b"Keys unlocked" }, b"", false);
            }
            _ => {}
        }
        if self.locked {
            // KEY-14: PTT and SIDE1 still work (PTT through key_down/key_up; SIDE1 does nothing on screen yet).
            if !matches!(e.key, Key::Ptt(_) | Key::Side(1)) {
                self.locked_toast();
            }
            return;
        }
        // MENU-25: hold MENU is this screen's options, on every screen the same way.
        if (e.key, e.press) == (Key::Menu, Long) && !self.screen.own_hold_menu() {
            if !self.screen.options(self) {
                self.show_toast(b"No options here", b"", false);
            }
            return;
        }
        // KEY-19: ◀▶ are the knob, unless the screen gives them a job of its own.
        if matches!((e.key, e.press), (Key::Left | Key::Right, Short | Repeat)) && !self.screen.own_arrows() {
            return self.screen.knob(self, if e.key == Key::Right { 1 } else { -1 }, store);
        }
        self.screen.key(self, e, store);
    }

    // ---- actions ----

    /// Home, forgetting where BACK would return (HOME-2).
    pub fn go_home(&mut self) {
        self.stop_mode(); // a scope, search or repeater stops with its screen
        self.screen = Screen::Home;
        self.back = [None; BACK_DEPTH];
    }

    /// Opens a screen by where it is in the manual ([`Dest`]), remembering this one for BACK. Screens not built
    /// yet open as a "coming" placeholder ([`screens::nav::enter`]).
    pub fn open(&mut self, dest: Dest) {
        self.push_back();
        self.screen = screens::nav::enter(dest, self);
    }

    /// Remembers the screen up now for BACK (MENU-24); full, the oldest goes.
    pub(crate) fn push_back(&mut self) {
        if self.back[BACK_DEPTH - 1].is_some() {
            self.back.copy_within(1.., 0);
            self.back[BACK_DEPTH - 1] = None;
        }
        if let Some(b) = self.back.iter_mut().find(|b| b.is_none()) {
            *b = Some(self.screen);
        }
    }

    /// Short BACK, on every screen (KEY-5, MENU-24): back one level, to the screen this one was opened from
    /// ([`App::open`] or [`App::push_back`] remembered it), or home when it was opened from home. Screens with
    /// steps of their own (an edit, a sub-page, typed digits) take one step back first.
    fn back(&mut self) {
        match self.back.iter_mut().rev().find(|b| b.is_some()).and_then(|b| b.take()) {
            Some(s) if s != Screen::Home => self.screen = s,
            _ => self.go_home(), // home by BACK too stops what the screens started
        }
    }

    fn locked_toast(&mut self) {
        self.show_toast(b"Locked:", b"hold # to unlock", false); // KEY-14
    }

    /// A toast from outside the app (the firmware's bring-up notice at boot).
    pub fn notice(&mut self, head: &[u8], body: &[u8]) {
        self.show_toast(head, body, false);
    }

    fn show_toast(&mut self, head: &[u8], body: &[u8], icon: bool) {
        self.toast = Some(Toast { head: Buf::from(head), body: Buf::from(body), icon, center: false, until: self.now.wrapping_add(TOAST_MS) });
    }

    /// A hold-digit shortcut's popup in the middle of the screen: what ran, and its new value if it has one (HOME-8).
    pub(crate) fn show_popup(&mut self, head: &[u8], body: &[u8]) {
        self.drawn.popup_gone |= self.toast.is_some_and(|t| t.center);
        self.toast = Some(Toast { head: Buf::from(head), body: Buf::from(body), icon: false, center: true, until: self.now.wrapping_add(POPUP_MS) });
    }

    /// A watched slot (not the active one) is receiving.
    fn watched_rx(&self) -> bool {
        self.rx_mask & self.watch_mask() & !(1 << self.active) & !self.relay_mask != 0
    }

    fn watch_mask(&self) -> u8 {
        [0, 0b011, 0b111][self.watch as usize % 3]
    }

    /// The slot's frequency and every setting, to the Radio task (made when popped: [`App::pop_cmd`]).
    fn send_slot(&mut self, slot: u8) {
        self.out.sync(slot);
    }

    /// 1.25 m: transmit power fixed at 0.25 W (the Radio task enforces it too). By the band it transmits on:
    /// a split's TX band, not RX's.
    fn capped(&self, slot: u8) -> bool {
        let s = &self.slots[slot as usize];
        slot::band_of(s.cfg().tx_of(s.freq())) == Some(BAND_125M)
    }

    /// Split on the active slot: transmit on `tx` whatever RX is tuned to (MENU 5's 4th choice). Refused (false,
    /// with a toast) when TX needs the band relay in the other state from RX (TX-37).
    pub(crate) fn set_split(&mut self, tx: Freq10) -> bool {
        let a = self.active;
        let s = &self.slots[a as usize];
        if !ferrite_core::txplan::relay_ok(s.cfg().rx_of(s.freq()), tx) {
            self.relay_toast();
            return false;
        }
        self.keep_undo();
        let c = self.slots[a as usize].cfg_mut();
        (c.split, c.offset) = (true, tx as i32);
        self.out.push(RadioCmd::SlotParam { slot: a, param: Param::Split, value: tx as i32 });
        self.save_channel(a, None);
        true
    }

    /// TX-37's refusal when a shift or split is set (the full sentence is on the TX refused screen).
    fn relay_toast(&mut self) {
        self.show_toast(b"Split TX needs relay", b"not with this RX frequency", false);
    }

    /// Why an item can't be changed on the active slot right now (MENU-14, EDIT-7), if it can't.
    pub fn greyed(&self, item: Item) -> Option<&'static str> {
        let s = &self.slots[self.active as usize];
        match item {
            Item::Step if s.on_channel() => Some("VFO only"), // VFO-8
            Item::Power if self.capped(self.active) => Some("Fixed at 0.25 W on 1.25 m"),
            _ => None,
        }
    }

    /// The knob or ◀▶ on a setting: change it live (MENU-16, MENU-19), remember it for undo and Recent.
    fn change(&mut self, item: Item, d: i32) {
        if d == 0 || self.greyed(item).is_some() {
            if let Some(why) = self.greyed(item) {
                self.show_toast(item.label().as_bytes(), why.as_bytes(), false);
            }
            return;
        }
        if matches!(item, Item::Channel | Item::Advanced) {
            return;
        }
        let a = self.active;
        self.keep_undo();
        match item {
            Item::Watch => {
                self.watch = (self.watch as i32 + d).rem_euclid(3) as u8;
                self.out.push(RadioCmd::Watch(self.watch_mask()));
            }
            Item::BatterySave => {
                self.bsave = (self.bsave as i32 + d).rem_euclid(4) as u8;
                self.out.push(RadioCmd::BatterySave(level(self.bsave)));
            }
            _ => {
                let rx = self.slots[a as usize].freq();
                let shift = BANDS[band_near(rx)].shift;
                let on_ch = self.slots[a as usize].on_channel();
                let c = self.slots[a as usize].cfg_mut();
                let mut v = item.get(c).0 + d;
                if item == Item::Squelch && !on_ch && v.rem_euclid(11) == 0 {
                    v += d.signum(); // a VFO has no "Global": it is the slot's level (SQL-6)
                }
                if item == Item::Offset && v.rem_euclid(4) == 3 {
                    // Turning onto Split keeps today's TX as the split's; OK on it sets another (VFO-16), so the
                    // knob can go on past it.
                    let tx = if c.split { c.offset.max(0) as Freq10 } else { (rx as i64 + c.offset as i64).max(0) as Freq10 };
                    if self.set_split(tx) {
                        self.show_toast(b"Split:", b"OK sets the TX frequency", false);
                    }
                    return;
                }
                let (was_split, old) = (c.split, *c);
                if let Some((param, value)) = item.set(c, v, shift) {
                    if item == Item::Offset && !ferrite_core::txplan::relay_ok(c.rx_of(rx), c.tx_of(rx)) {
                        *c = old; // TX-37: a shift that crosses the relay boundary isn't kept
                        self.relay_toast();
                        return;
                    }
                    let value = if param == Param::Squelch { self.sq_level(a) as i32 } else { value };
                    self.out.push(RadioCmd::SlotParam { slot: a, param, value });
                    if was_split && param == Param::Offset {
                        self.out.push(RadioCmd::SlotParam { slot: a, param: Param::Split, value: 0 });
                    }
                }
                self.save_channel(a, None);
            }
        }
        if self.recent[0] != Some(item) {
            let keep = self.recent.iter().position(|&r| r == Some(item)).unwrap_or(self.recent.len() - 1);
            self.recent.copy_within(0..keep, 1);
            self.recent[0] = Some(item);
        }
    }

    /// # on a setting: back to its default (MENU-18).
    fn reset(&mut self, item: Item) {
        let cur = match item {
            Item::Watch => self.watch as i32,
            Item::BatterySave => self.bsave as i32,
            _ => item.get(self.slots[self.active as usize].cfg()).0,
        };
        let def = match item {
            Item::Watch | Item::BatterySave => 1,
            Item::Squelch if self.slots[self.active as usize].on_channel() => 0, // a channel's default: Global
            _ => item.default_of(),
        };
        self.change(item, def - cur);
    }

    /// The squelch level slot `s` listens with: its own, or the slot's (VFO) level when its channel says Global
    /// (SQL-6).
    pub fn sq_level(&self, s: u8) -> u8 {
        let slot = &self.slots[s as usize];
        match slot.cfg().squelch {
            SQ_GLOBAL.. => slot.cfg.squelch.min(9),
            l => l,
        }
    }

    /// SIDE1 + knob / ▲▼: the live level one step up or down, stopping at 0 and 9 (SQL-3). A channel on Global
    /// moves the slot's level and stays Global.
    pub(crate) fn step_squelch(&mut self, d: i32) {
        let a = self.active;
        let cur = self.sq_level(a) as i32;
        let to = (cur + d).clamp(0, 9);
        if to == cur {
            return;
        }
        if self.slots[a as usize].cfg().squelch >= SQ_GLOBAL {
            self.keep_undo();
            self.slots[a as usize].cfg.squelch = to as u8;
            self.out.push(RadioCmd::SlotParam { slot: a, param: Param::Squelch, value: to });
        } else {
            self.change(Item::Squelch, to - cur);
        }
    }

    /// Remembers the active slot's state before a change, for "* Undo" (EDIT-4, MENU-22): the first change
    /// of a run keeps it, every change restarts the 5 s.
    pub(crate) fn keep_undo(&mut self) {
        let a = self.active;
        if self.undo.is_none() {
            let s = &self.slots[a as usize];
            let ch = s.ch.filter(|_| s.on_channel()).map(|c| ChUndo { num: c.num, zone: c.zone, flags: c.flags, rx: c.rx });
            self.undo = Some(Undo { slot: a, cfg: *s.cfg(), ch, home: self.set.home_ch, watch: self.watch, bsave: self.bsave, until: 0 });
        }
        if let Some(u) = &mut self.undo {
            u.until = self.now.wrapping_add(UNDO_MS);
        }
    }

    fn do_undo(&mut self) {
        if let Some(u) = self.undo.take() {
            let s = &mut self.slots[u.slot as usize];
            match u.ch {
                Some(old) if s.on_channel() => {
                    let from = s.ch.map(|c| c.num).filter(|&n| n != old.num);
                    if let Some(c) = &mut s.ch {
                        (c.num, c.zone, c.flags, c.rx, c.cfg) = (old.num, old.zone, old.flags, old.rx, u.cfg);
                        s.zone = old.zone;
                    }
                    self.set.home_ch = u.home;
                    self.save_channel(u.slot, from);
                }
                _ => *s.cfg_mut() = u.cfg,
            }
            self.send_slot(u.slot);
            if u.watch != self.watch {
                self.watch = u.watch;
                self.out.push(RadioCmd::Watch(self.watch_mask()));
            }
            if u.bsave != self.bsave {
                self.bsave = u.bsave;
                self.out.push(RadioCmd::BatterySave(level(self.bsave)));
            }
            self.show_toast(b"Undone", b"", false);
        }
    }

    /// Value text for quick rows that aren't slot settings.
    fn item_text(&self, item: Item, long: bool, b: &mut Buf<16>) {
        let s = &self.slots[self.active as usize];
        match item {
            Item::Watch => b.set(["Off", "A+B", "A+B+C"][self.watch as usize % 3].as_bytes()),
            Item::BatterySave => {
                b.clear();
                if self.bsave == 0 {
                    b.set(b"Off");
                } else {
                    let _ = ufmt::uwrite!(b, "Level {}", self.bsave);
                }
            }
            Item::Channel => b.set(if s.on_channel() { b"Edit" } else { b"Save as channel" }),
            Item::Advanced => b.set(b">"),
            Item::Power if self.capped(self.active) => b.set(b"0.25 W"),
            // A channel on Global: the level in use; "Global (4)" where there is room.
            Item::Squelch if s.cfg().squelch >= SQ_GLOBAL => {
                b.clear();
                let l = self.sq_level(self.active);
                let _ = if long { ufmt::uwrite!(b, "Global ({})", l) } else { ufmt::uwrite!(b, "{}", l) };
            }
            _ => item.text(s.cfg(), long, b),
        }
    }

    fn is_default(&self, item: Item) -> bool {
        match item {
            Item::Watch => self.watch == 1,
            Item::BatterySave => self.bsave == 1,
            Item::Channel | Item::Advanced => true,
            Item::Squelch if self.slots[self.active as usize].on_channel() => {
                self.slots[self.active as usize].cfg().squelch >= SQ_GLOBAL
            }
            _ => item.get(self.slots[self.active as usize].cfg()).0 == item.default_of(),
        }
    }
}

fn level(b: u8) -> Level {
    [Level::Off, Level::L1, Level::L2, Level::L3][b as usize % 4]
}

/// `now` has reached `t` (wrapping ms).
fn after(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) as i32 >= 0
}

#[cfg(test)]
mod tests;
