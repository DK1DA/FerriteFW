//! The channel editor (edit in place on a channel: hold SLOT on a channel, MENU 8, end of Add repeater; manual
//! chapters 6 and 10, EDIT-1 to EDIT-7, CHAN-5, CHAN-18; HomeEditCh mock-up). Zone, name and frequency at the
//! top, twelve settings in a 3 x 4 grid under them. Arrows pick, the knob changes at once and the channel is
//! saved (EDIT-4); changing zone or number moves the channel (EDIT-5), and Home makes it the one home channel
//! (CHAN-18). OK on the name types it (A7), on the frequency types it (A1), on a setting opens its bigger
//! editor (A5, A6).

use super::edit::other_slots;
use super::entry::{Entry, EntryFor};
use super::forms::field_box;
use super::nav::Dest;
use super::shared::*;
use super::text::TextFor;
use super::{Cx, Hints, View};
use crate::chan::{Channel, ChannelStore, FLAG_APRS, FLAG_PRIORITY, FLAG_RX_ONLY, FLAG_SKIP, MAX_CHANNELS, NO_ZONE};
use crate::keys::{KeyEvent, Press};
use crate::slot::{chan_label, Item, STEPS, band_near};
use crate::{App, Screen, EDIT_IDLE_MS};
use ferrite_core::freq::mhz;
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::{Buf, Zero};
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChEdit {
    /// 0 zone, 1 name, 2 frequency, 3-14 the grid row by row.
    pub cell: u8,
}

/// A grid cell: a slot setting, a channel flag, the number, Home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cell {
    It(Item),
    Flag(u8, &'static str),
    /// In scans, skipped, or priority (decision 63): Skip and Priority exclude each other.
    Scan,
    Number,
    Home,
}

/// The twelve settings (EDIT-2; chapter 10's list), row by row.
const CELLS: [Cell; 12] = [
    Cell::It(Item::Mode),
    Cell::It(Item::RxTone),
    Cell::It(Item::Tones),
    Cell::It(Item::Offset),
    Cell::It(Item::Power),
    Cell::It(Item::Squelch),
    Cell::It(Item::BusyLock),
    Cell::Scan,
    Cell::Flag(FLAG_APRS, "APRS"),
    Cell::Flag(FLAG_RX_ONLY, "RX only"),
    Cell::Number,
    Cell::Home,
];

const ZONE: u8 = 0;
const NAME: u8 = 1;
const FREQ: u8 = 2;
const GRID0: u8 = 3;
/// The Offset cell (shift and offset), where hold 7 and 8 open the editor on a channel.
pub(crate) const OFFSET_CELL: u8 = GRID0 + 3;
/// Grid geometry: 3 columns of 72 at 77 apart, 4 rows of 29 at 34 apart (HomeEditCh).
const GX: i32 = 7;
const GY: i32 = 108;

impl Cell {
    fn label(self) -> &'static str {
        match self {
            Cell::It(i) => i.label(),
            Cell::Flag(_, l) => l,
            Cell::Scan => "Scan",
            Cell::Number => "Number",
            Cell::Home => "Home",
        }
    }
}

/// Changes the active slot's channel (keeping undo), moves the slot with it and saves it (EDIT-4, EDIT-5).
pub(crate) fn edit(app: &mut App, f: impl FnOnce(&mut Channel)) {
    let a = app.active;
    let Some(old) = app.slots[a as usize].ch.filter(|_| app.slots[a as usize].on_channel()) else { return };
    app.keep_undo();
    let mut c = old;
    f(&mut c);
    if c == old {
        return;
    }
    if app.set.home_ch == Some(old.num) {
        app.set.home_ch = Some(c.num); // CHAN-18: renumbering carries it
    }
    let s = &mut app.slots[a as usize];
    (s.ch, s.zone) = (Some(c), c.zone);
    if c.rx != old.rx {
        app.send_slot(a);
    }
    app.save_channel(a, (c.num != old.num).then_some(old.num));
}

/// Where the channel now sits in its zone (by number, CHAN-10), counting the others in the store.
fn place(app: &mut App, store: &impl ChannelStore) {
    let a = app.active as usize;
    if let Some(c) = app.slots[a].ch {
        let pos = (0..store.zone_len(c.zone)).filter_map(|i| store.zone_at(c.zone, i)).filter(|o| o.num < c.num).count();
        app.slots[a].pos = pos as u16;
    }
}

impl View for ChEdit {
    /// KEY-19: ◀▶ move across the edit grid; elsewhere they are the knob.
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Channel editor"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("HomeEditCh.png")
    }

    fn title(self, app: &App, b: &mut Buf<10>) {
        let _ = uwrite!(b, "EDIT {}", w::LETTERS[app.active as usize] as char); // EDIT-1
    }

    fn timeout(self) -> Option<u32> {
        Some(EDIT_IDLE_MS) // EDIT-6
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        if !app.slots[app.active as usize].on_channel() {
            return app.go_home();
        }
        let cell = self.cell;
        let to = |c: u8| Screen::ChEdit(ChEdit { cell: c });
        let (r, col) = if cell >= GRID0 { ((cell - GRID0) / 3, (cell - GRID0) % 3) } else { (0, 0) };
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => {
                app.screen = to(match cell {
                    ZONE => ZONE,
                    NAME | FREQ => cell - 1,
                    _ if r == 0 => FREQ,
                    _ => cell - 3,
                })
            }
            (Key::Down, Short | Repeat) => {
                app.screen = to(match cell {
                    ZONE | NAME => cell + 1,
                    FREQ => GRID0,
                    _ if r == 3 => cell,
                    _ => cell + 3,
                })
            }
            (Key::Left, Short | Repeat) if cell >= GRID0 && col > 0 => app.screen = to(cell - 1),
            (Key::Right, Short | Repeat) if cell >= GRID0 && col < 2 => app.screen = to(cell + 1),
            (Key::Left | Key::Right, Short | Repeat) if cell < GRID0 => self.knob(app, if e.key == Key::Right { 1 } else { -1 }, store),
            (Key::Exit | Key::Slot, Short) => app.back(), // EDIT-6: leaves the edit, one level back
            (Key::Star, Short) => {
                app.do_undo();
                place(app, store);
            }
            (Key::Hash, Short) => match self.at() {
                // MENU-18: back to the default
                Some(Cell::It(i)) => app.reset(i),
                Some(Cell::Flag(bit, _)) => edit(app, |c| c.flags &= !bit),
                Some(Cell::Scan) => edit(app, |c| c.flags &= !(FLAG_SKIP | FLAG_PRIORITY)),
                Some(Cell::Home) => set_home(app, false),
                _ => {}
            },
            (Key::Menu, Short) => match cell {
                NAME => {
                    super::text::open(app, TextFor::ChannelName, store);
                }
                FREQ => {
                    app.open(Dest::Entry(0));
                    app.screen = Screen::Entry(Entry::new(EntryFor::ChannelFreq, None));
                }
                _ => {
                    if let Some(Cell::It(i)) = self.at() {
                        app.open(Dest::value(i)); // MENU-17
                    }
                }
            },
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let Some(ch) = app.slots[app.active as usize].ch else { return };
        match self.cell {
            ZONE => {
                // Every zone, then none (CHAN-10): the channel moves (EDIT-5).
                let n = store.zones() as i32 + 1;
                let i = if ch.zone == NO_ZONE { n - 1 } else { ch.zone as i32 };
                let k = (i + d).rem_euclid(n);
                let z = if k == n - 1 { NO_ZONE } else { k as u8 };
                edit(app, |c| c.zone = z);
                place(app, store);
            }
            NAME => {}
            FREQ => {
                let step = STEPS[crate::slot::band_step(band_near(ch.rx), app.set.region()) as usize].0 as i32;
                let f = (ch.rx as i32 + d * step).clamp(mhz(64) as i32, mhz(1000) as i32 - 1) as u32; // CHAN-22
                edit(app, |c| c.rx = f);
            }
            _ => match self.at() {
                Some(Cell::It(i)) => app.change(i, d),
                Some(Cell::Flag(bit, _)) => edit(app, |c| c.flags ^= bit),
                Some(Cell::Scan) => edit(app, |c| {
                    // In scans -> Skip -> Priority, and back the other way.
                    let i = if c.flags & FLAG_PRIORITY != 0 { 2 } else { (c.flags & FLAG_SKIP != 0) as i32 };
                    let n = (i + d).rem_euclid(3);
                    c.flags = (c.flags & !(FLAG_SKIP | FLAG_PRIORITY)) | [0, FLAG_SKIP, FLAG_PRIORITY][n as usize];
                }),
                Some(Cell::Home) => set_home(app, app.set.home_ch != Some(ch.num)),
                Some(Cell::Number) => {
                    // The next free number that way (EDIT-5: renumbering moves the channel).
                    let free = (1..MAX_CHANNELS as i32)
                        .map(|k| (ch.num as i32 + d.signum() * k).rem_euclid(MAX_CHANNELS as i32) as u16)
                        .find(|&n| store.by_num(n).is_none());
                    if let Some(n) = free {
                        edit(app, |c| c.num = n);
                        place(app, store);
                    }
                }
                None => {}
            },
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let a = app.active;
        let Some(ch) = app.slots[a as usize].ch else { return };
        let cell = self.cell;
        // Top line: letter, number, the zone box, EDIT.
        let zone = cx.store.zone_label(ch.zone, b"None", b"?");
        let zone = zone.as_bytes();
        if app.dirty(R_TOP, hash(&(a, ch.num, zone, cell == ZONE))) {
            w::badge(lcd, 7, 26, a, true, BG);
            let mut l = Buf::<32>::new();
            chan_label(ch.num, &mut l);
            w::field(lcd, 21, 48, 24, 41, 38, Align::Left, b"", &UI_12, TEXT, BG);
            w::field(lcd, 27, 42, 24, 41, 38, Align::Left, l.as_bytes(), &UI_12, TEXT, BG);
            field_box(lcd, 68, 23, 122, 20, b"Zone", zone, cell == ZONE, false);
            lcd.fill(190, 23, 2, 20, BG);
            w::tag(lcd, 232, 25, b"EDIT", BLUE, BG);
        }
        if app.dirty(R_NOTE, hash(&(ch.name, cell == NAME))) {
            field_box(lcd, 7, 46, 226, 22, b"Name", ch.name(), cell == NAME, false);
        }
        // Regions: the grid takes 1-12, so the frequency uses R_SUB.
        if app.dirty(R_SUB, hash(&(ch.rx, cell == FREQ))) {
            w::freq(lcd, 7, 232, 102, ch.rx, &DIG_46, &DIG_28, TEXT, SUB, BG);
            lcd.fill(7, 104, 225, 2, if cell == FREQ { BLUE } else { BG });
        }
        let home = app.set.home_ch == Some(ch.num);
        for (i, c) in CELLS.into_iter().enumerate() {
            let mut v = Buf::<16>::new();
            let mut grey = false;
            match c {
                Cell::It(Item::Power) if app.capped(a) => v.set(b"0.25 W"),
                Cell::It(it) => {
                    it.text(&ch.cfg, false, &mut v);
                    grey = app.greyed(it).is_some();
                }
                Cell::Flag(bit, _) => v.set(if ch.flags & bit != 0 { b"On" } else { b"Off" }),
                Cell::Scan => v.set(if ch.flags & FLAG_PRIORITY != 0 {
                    b"Priority".as_slice()
                } else if ch.flags & FLAG_SKIP != 0 {
                    b"Skip"
                } else {
                    b"In scans"
                }),
                Cell::Number => {
                    let _ = uwrite!(v, "{}", Zero(ch.num as u32 + 1, 3));
                }
                Cell::Home => v.set(if home { b"Yes" } else { b"No" }),
            }
            if c == Cell::It(Item::Power) && ch.flags & FLAG_RX_ONLY != 0 {
                grey = true;
            }
            let (x, y) = (GX + (i as i32 % 3) * 77, GY + (i as i32 / 3) * 34);
            app.grid_cell(lcd, i, x, y, 72, 29, c.label().as_bytes(), v.as_bytes(), cell == GRID0 + i as u8, grey);
        }
        // The note: why the focused cell is greyed (EDIT-7), or what the focused field does.
        let mut note = Buf::<48>::new();
        let mut col = SUB;
        match (cell, self.at()) {
            (ZONE, _) => note.set(b"Knob: move the channel to another zone"),
            (NAME, _) => note.set(b"OK: type the name (16 characters)"),
            (FREQ, _) => note.set("Knob: step \u{b7} OK: type it".as_bytes()),
            (_, Some(Cell::It(Item::Power))) if ch.flags & FLAG_RX_ONLY != 0 => {
                note.set(b"Power: this channel is RX only");
                col = AMBER;
            }
            (_, Some(Cell::It(it))) if app.greyed(it).is_some() => {
                note.set(it.label().as_bytes());
                note.push(b": ");
                note.push(app.greyed(it).unwrap_or("").as_bytes());
                col = AMBER;
            }
            (_, Some(Cell::Number)) => note.set(b"Knob: the next free number moves it"),
            (_, Some(Cell::Home)) => note.set(b"Hold 0 on home jumps to the home channel"),
            _ => {}
        }
        if app.dirty(R_HELP, hash(&(note, col))) {
            w::field(lcd, 7, 226, 242, 259, 255, Align::Left, note.as_bytes(), &UI_12, col, BG);
        }
        other_slots(app, lcd, cx.live);
    }

    fn hints(self, app: &App) -> Hints {
        if app.undo.is_some() {
            Hints::new(&[("Knob", "Change"), ("*", "Undo"), ("BACK", "Done")])
        } else if matches!(self.cell, NAME | FREQ) {
            Hints::new(&[(ARROWS_4, "Pick"), ("OK", "Type"), ("BACK", "Done")])
        } else {
            Hints::new(&[(ARROWS_4, "Pick"), ("Knob", "Change"), ("BACK", "Done")])
        }
    }
}

impl ChEdit {
    fn at(self) -> Option<Cell> {
        self.cell.checked_sub(GRID0).and_then(|i| CELLS.get(i as usize)).copied()
    }
}

/// Home on or off for the active slot's channel: one home channel per radio (CHAN-17, CHAN-18).
fn set_home(app: &mut App, on: bool) {
    let Some(ch) = app.slots[app.active as usize].ch else { return };
    let is = app.set.home_ch == Some(ch.num);
    if on != is {
        app.keep_undo();
        app.set.home_ch = if on { Some(ch.num) } else { None };
    }
}
