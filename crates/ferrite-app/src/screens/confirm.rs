//! Confirm, shared by every destructive action (manual chapters 11, 10, 34 and 22; MENU-23, CHAN-12, CHAN-13,
//! PIN-6, PIN-7, TSRCH-4): it names what goes and what is kept; OK confirms, BACK cancels. A settings reset
//! (MENU 0 9 8 2) asks twice: settings first, then whether the channels go too (PIN-6). Other screens ask with
//! [`ask`] (the browser's delete, the zones list, tone search's "TX too?").

use super::forms::para;
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::{ChanWrite, ChannelStore, NO_ZONE};
use crate::data::Op;
use crate::keys::{KeyEvent, Press};
use crate::slot::{chan_label, tone_text};
use crate::{App, Screen, Slot, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::{RadioCmd, SLOTS};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

/// What is being confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    /// Settings reset; then, on its second page, the channels too (PIN-6).
    Reset { channels: bool },
    /// Delete a channel, by number (CHAN-13).
    DeleteChannel(u16),
    /// Delete a zone (CHAN-12).
    DeleteZone(u8),
    /// Delete a range preset, by index (MENU 0 1 7).
    DeletePreset(u8),
    /// Tone search found a tone: set it as the TX tone too (TSRCH-4).
    ToneTx(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Confirm {
    pub ask: Ask,
    /// The settings reset asked at power on (hold MENU, PIN-6, BOOT-14): modal, so hold BACK, PTT and the menu
    /// timeout can't skip past a PIN; BACK on its first question goes on to the PIN (if one is set).
    pub boot: bool,
}

/// Opens the confirm for `ask`, remembering the screen it came from for BACK.
pub fn ask(app: &mut App, ask: Ask) {
    app.open(Dest::Confirm);
    app.screen = Screen::Confirm(Confirm { ask, boot: false });
}

/// Title, what goes, what stays (PIN-7: what will be cleared and what is kept).
fn words(ask: Ask) -> (&'static str, &'static str, &'static str, &'static str) {
    match ask {
        Ask::Reset { channels: false } => (
            "Reset settings?",
            "Every setting back to its default: callsign, region, PIN, sounds, display, home channel.",
            "Channels and zones, calibration, the SSB patch, the bootloader.",
            "Reset",
        ),
        Ask::Reset { channels: true } => (
            "Delete the channels too?",
            "Every channel and zone.",
            "Calibration, the SSB patch, the bootloader. BACK keeps the channels.",
            "Delete",
        ),
        Ask::DeleteChannel(_) => ("Delete this channel?", "", "Every other channel, and the zone it was in.", "Delete"),
        Ask::DeleteZone(_) => ("Delete this zone?", "", "Its channels: they move to no zone and stay in All.", "Delete"),
        Ask::DeletePreset(_) => ("Delete this range?", "Its skipped frequencies too.", "Every other range preset, and the channels.", "Delete"),
        Ask::ToneTx(_) => ("Set the TX tone too?", "", "The RX tone is already set. BACK leaves TX as it is.", "Set"),
    }
}

impl View for Confirm {
    fn name(self) -> &'static str {
        "Confirm"
    }

    fn timeout(self) -> Option<u32> {
        (!self.boot).then_some(MENU_TIMEOUT_MS)
    }

    fn modal(self) -> bool {
        self.boot
    }

    fn ptt(self, app: &mut App, down: bool) {
        // At power on PTT waits, as it does for the PIN (PIN-1).
        if down && !self.boot {
            super::nav::transmit(app);
        }
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        match (e.key, e.press) {
            (Key::Menu, Press::Short) => match self.ask {
                Ask::Reset { channels: false } => {
                    reset_settings(app);
                    app.show_toast(b"Settings reset", b"", false);
                    if self.boot {
                        app.back = [None; crate::BACK_DEPTH]; // the PIN it guarded is gone with the settings
                    }
                    app.screen = Screen::Confirm(Confirm { ask: Ask::Reset { channels: true }, ..self });
                }
                Ask::Reset { channels: true } => {
                    app.write(ChanWrite::EraseAll);
                    for s in app.slots.iter_mut() {
                        (s.ch, s.vfo_mode, s.zone, s.pos) = (None, true, 0, 0);
                    }
                    for s in 0..SLOTS as u8 {
                        app.send_slot(s);
                    }
                    app.show_toast(b"Channels deleted", b"", false);
                    app.go_home();
                }
                Ask::DeleteChannel(num) => {
                    if !store.data().request(Op::DeleteChannel(num)) {
                        app.write(ChanWrite::Delete(num)); // a store whose records have no Data owner yet
                    }
                    for s in 0..SLOTS {
                        if app.slots[s].ch.is_some_and(|c| c.num == num) {
                            let sl = &mut app.slots[s];
                            (sl.ch, sl.vfo_mode) = (None, true);
                            app.send_slot(s as u8);
                        }
                    }
                    if app.set.home_ch == Some(num) {
                        app.set.home_ch = None; // CHAN-18
                    }
                    let mut l = Buf::<32>::new();
                    chan_label(num, &mut l);
                    app.show_toast(l.as_bytes(), b"deleted", false);
                    app.back();
                    if let Screen::Browse(b) = app.screen {
                        app.screen = Screen::Browse(super::browse::after_delete(b, store));
                    }
                }
                Ask::DeleteZone(z) => {
                    let name = store.zone_label(z, b"Zone", b"Zone");
                    if !store.data().request(Op::DeleteZone(z)) {
                        app.write(ChanWrite::DeleteZone(z));
                    }
                    for s in app.slots.iter_mut().filter(|s| s.zone == z) {
                        s.zone = NO_ZONE;
                        if let Some(c) = &mut s.ch {
                            c.zone = NO_ZONE;
                        }
                    }
                    app.show_toast(name.as_bytes(), b"deleted", false);
                    app.back();
                    if let Screen::Zones(_) = app.screen {
                        app.screen = Screen::Zones(super::zones::after_delete(store));
                    }
                }
                Ask::DeletePreset(i) => {
                    let d = store.data();
                    let name = d.preset(i).map(|p| p.name);
                    if d.request(Op::DeletePreset(i)) {
                        app.show_toast(b"Deleted", name.as_ref().map_or(b"".as_slice(), |n| n.as_bytes()), false);
                    }
                    app.back();
                    if let Screen::ScanLists(s) = app.screen {
                        app.screen = Screen::ScanLists(super::scanlists::ScanLists { cur: Default::default(), ..s });
                    }
                }
                Ask::ToneTx(t) => {
                    super::tones::set(app, false, t);
                    app.back();
                }
            },
            (Key::Exit, Press::Short) => app.back(),
            _ => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let (title, goes, kept, _) = words(self.ask);
        // What it names: the channel, the zone, the tone.
        let mut what = Buf::<40>::new();
        match self.ask {
            Ask::DeleteChannel(n) => {
                let mut l = Buf::<32>::new();
                chan_label(n, &mut l);
                what.push(l.as_bytes());
                if let Some((c, _)) = cx.store.by_num(n) {
                    what.push(" \u{b7} ".as_bytes());
                    what.push(c.name());
                    what.push(" \u{b7} ".as_bytes());
                    what.push(w::freq3(c.rx).as_bytes());
                }
            }
            Ask::DeleteZone(z) => {
                what.set(cx.store.zone_label(z, b"?", b"?").as_bytes());
                let _ = ufmt::uwrite!(what, " \u{b7} {} channels", cx.store.zone_len(z));
            }
            Ask::DeletePreset(i) => what.set(cx.store.data().preset(i).map_or(Buf::<12>::new(), |p| p.name).as_bytes()),
            Ask::ToneTx(t) => {
                let mut b = Buf::<16>::new();
                tone_text(t, &mut b);
                what.set(b.as_bytes());
            }
            Ask::Reset { .. } => {}
        }
        if app.dirty(R_TOP, hash(&(self.ask, what))) {
            lcd.fill(0, 21, 240, 279, BG);
            let col = if matches!(self.ask, Ask::ToneTx(_)) { TEXT } else { AMBER };
            w::field(lcd, 12, 216, 36, 54, 50, Align::Left, title.as_bytes(), &UI_12B, col, BG);
            let mut y = 64;
            if !what.is_empty() {
                lcd.fill_r(8, y, 224, 30, 4, SURFACE);
                w::field(lcd, 16, 208, y + 6, y + 24, y + 20, Align::Left, what.as_bytes(), &UI_12B, TEXT, SURFACE);
                y += 40;
            }
            if !goes.is_empty() {
                w::field(lcd, 12, 216, y, y + 17, y + 13, Align::Left, b"Goes", &UI_12B, SUB, BG);
                y += 18 + 17 * para(lcd, 12, y + 18, 216, 4, 17, goes.as_bytes(), &UI_12, TEXT, BG);
                y += 8;
            }
            w::field(lcd, 12, 216, y, y + 17, y + 13, Align::Left, b"Kept", &UI_12B, SUB, BG);
            para(lcd, 12, y + 18, 216, 4, 17, kept.as_bytes(), &UI_12, TEXT, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        match words(self.ask).3 {
            "Reset" => Hints::new(&[("OK", "Reset"), ("BACK", "Cancel")]),
            "Set" => Hints::new(&[("OK", "Set"), ("BACK", "Cancel")]),
            _ => Hints::new(&[("OK", "Delete"), ("BACK", "Cancel")]),
        }
    }
}

/// Every setting back to its default, the PIN with them (PIN-4); channels, calibration and the SSB patch are
/// not touched here (PIN-8). Setup has run, so it isn't asked again.
pub(crate) fn reset_settings(app: &mut App) {
    *app.set = crate::settings::Settings { setup_done: app.set.setup_done, ..crate::settings::Settings::DEFAULT };
    app.watch = 1;
    app.bsave = 1;
    app.recent = [None; 3];
    app.undo = None;
    app.locked = false;
    for s in app.slots.iter_mut() {
        let n = Slot::new(s.vfo);
        (s.cfg, s.mem) = (n.cfg, n.mem);
    }
    for s in 0..SLOTS as u8 {
        app.send_slot(s);
    }
    app.out.push(RadioCmd::Watch(app.watch_mask()));
    app.out.push(RadioCmd::BatterySave(crate::level(app.bsave)));
}
