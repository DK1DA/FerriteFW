//! The Status page (# on home, or MENU 0 0; manual chapter 8, SBAR-13 to SBAR-18; StatusPage mock-up): one
//! row per radio-wide state; OK on a row opens its setting.

use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use super::table::{self, slow_table, Act, Ctx, Go, Table, Val};
use crate::keys::KeyEvent;
use crate::slot::{band_name, Item};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::msg::GpsState;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Status {
    pub row: u8,
    /// Opened from advanced settings (MENU 0 0; BACK returns there by the BACK stack).
    pub from_menu: bool,
}

/// HF-6: everything else pauses in HF mode.
fn paused(c: &Ctx<()>, b: &mut Buf<40>) -> bool {
    let hf = super::hf::in_hf(c.app);
    if hf {
        b.set(b"Paused: HF mode");
    }
    hf
}

fn on_off(b: &mut Buf<40>, on: bool) -> u16 {
    b.set(if on { b"On".as_slice() } else { b"Off" });
    TEXT
}

slow_table! {
    /// The Status page's rows (SBAR-14) and where OK on each goes (SBAR-15).
    ROWS: [(&str, Val<()>); 15] = [
        // SBAR-14: why transmit is locked, if it is (BOOT-11).
        (slow!("Transmit"), |c, b| match super::info::tx_lock(c.dev) {
            None => {
                b.set(b"Ready");
                GREEN
            }
            Some(why) => {
                b.set(why.as_bytes());
                AMBER
            }
        }),
        (slow!("Region"), |c, b| {
            b.set(c.app.set.region_name().as_bytes());
            TEXT
        }),
        (slow!("Extend TX"), |c, b| on_off(b, c.app.set.extend_tx)),
        (slow!("Inbox"), |c, b| {
            if c.app.inbox == 0 {
                b.set(b"Empty");
            } else {
                let _ = uwrite!(b, "{} unread", c.app.inbox);
            }
            TEXT
        }),
        (slow!("Scan"), |c, b| {
            if paused(c, b) {
                return AMBER;
            }
            for (s, l) in c.live.slots.iter().enumerate().filter(|(_, l)| l.scanning) {
                if !b.is_empty() {
                    b.push(b", ");
                }
                let _ = uwrite!(b, "{}: {}", w::LETTERS[s] as char, band_name(l.freq));
            }
            if b.is_empty() {
                b.set(b"Off");
            }
            TEXT
        }),
        (slow!("Watch"), |c, b| {
            if paused(c, b) {
                return AMBER;
            }
            b.set(WATCH_TEXT[c.app.watch as usize % 3].as_bytes());
            if c.app.paused_until.is_some() {
                b.push(" \u{b7} paused".as_bytes());
            }
            TEXT
        }),
        (slow!("Jobs"), |c, b| {
            if paused(c, b) {
                return AMBER;
            }
            b.set(b"None");
            TEXT
        }),
        (slow!("Beacon"), |c, b| if paused(c, b) { AMBER } else { on_off(b, false) }),
        (slow!("Battery Save"), |c, b| {
            let mut t = Buf::<16>::new();
            c.app.item_text(Item::BatterySave, true, &mut t);
            b.set(t.as_bytes());
            TEXT
        }),
        (slow!("GPS"), |c, b| {
            match c.dev.gps {
                GpsState::Off => b.set(b"Off"),
                GpsState::Searching => b.set(b"Searching"),
                GpsState::Fix => {
                    let _ = uwrite!(b, "Fix \u{b7} {} sats", c.dev.sats);
                }
            }
            TEXT
        }),
        (slow!("Bluetooth"), |c, b| {
            b.set(if c.dev.bt { b"Connected".as_slice() } else { b"Off" });
            TEXT
        }),
        (slow!("Battery"), |c, b| {
            let d = c.dev;
            let _ = uwrite!(b, "{}%", d.battery_pct); // volts are on About
            if d.battery_cutoff {
                RED
            } else if d.battery_low {
                AMBER
            } else {
                TEXT
            }
        }),
        (slow!("Key lock"), |c, b| on_off(b, c.app.locked)),
        // SBAR-18: paused in HF mode; WX-3: US and Canada only.
        (slow!("Weather alerts"), |c, b| {
            if !c.app.set.wx_region() {
                b.set(b"Not in region");
                SUB
            } else if !c.app.set.same {
                on_off(b, false)
            } else if paused(c, b) {
                AMBER
            } else {
                on_off(b, true)
            }
        }),
        // Chapter 38: the packet mailbox (no menu number: it is reached from here).
        (slow!("Mailbox"), |_, b| on_off(b, false)),
    ];
}

slow_table! {
    GO: [Go; 15] = [
        Go::Open(Dest::Group(8)),
        Go::Open(Dest::Group(9)),
        Go::Open(Dest::Group(8)),
        Go::Open(Dest::EventLog),
        Go::Later,
        Go::Quick(6),
        Go::Later,
        Go::Later,
        Go::Quick(8),
        Go::Open(Dest::GpsPosition),
        Go::Open(Dest::Sub(9, 2)),
        Go::Later,
        Go::Later,
        Go::Open(Dest::Sub(3, 3)),
        Go::Open(Dest::Mailbox),
    ];
}

slow_table! {
    PAGE: Table<()> = Table { rows: &ROWS, go: &GO, y: 20, h: 18, style: w::KV_STATUS };
}

impl View for Status {
    fn name(self) -> &'static str {
        "Status"
    }

    fn mockup(self) -> Option<&'static str> {
        Some("StatusPage.png")
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        match table::key(app, &PAGE, self.row, e) {
            Act::Move(row) => app.screen = Screen::Status(Status { row, ..self }),
            Act::Back => app.back(),
            Act::Opened | Act::None => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        app.screen = Screen::Status(Status { row: table::knob(&PAGE, self.row, d), ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.table(lcd, &PAGE, cx.live, cx.dev, &(), Some(self.row));
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrite_core::msg::{Device, LiveAll};
    use ferrite_core::txplan::Region;

    fn region_row(app: &App) -> Buf<40> {
        let mut b = Buf::new();
        let (live, dev) = (LiveAll::default(), Device::default());
        (ROWS[1].1)(&Ctx { app, live: &live, dev: &dev, x: &() }, &mut b);
        b
    }

    /// The Region row is the one radio-wide setting (setup, MENU 0 9 4), never a copy of its own.
    #[test]
    fn region_row_reads_the_settings() {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        assert_eq!(ROWS[1].0, "Region");
        assert_eq!(region_row(&app).as_bytes(), b"Not set");
        app.settings_mut().set_region(Some(Region::Uk));
        assert_eq!(region_row(&app).as_bytes(), app.settings().region_name().as_bytes());
        assert_ne!(region_row(&app).as_bytes(), b"Not set");
    }
}
