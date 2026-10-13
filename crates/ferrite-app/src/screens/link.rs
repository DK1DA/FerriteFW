//! Bluetooth and the link (manual chapter 29): link status (D17, MENU 0 9 2 3; LINK-17), approved devices (D18,
//! MENU 0 9 2 2; LINK-7), the write approval prompt (D20; LINK-7, LINK-8, SAFE-6) and a write session's
//! progress (D21; LINK-9). The Link task opens the last two with [`App::open`] (`Dest::WriteApproval`,
//! `Dest::WriteProgress`) and reads the answer from [`App::take_link_answer`].

use crate::link::TOKENS;
use super::nav::Dest;
use super::shared::*;
use super::table::{slow_table, Ctx, Table, Val};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::ON_OFF;
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkPage {
    Status,
    Approved,
    Approve,
    Progress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Link {
    pub page: LinkPage,
    pub row: u8,
}

/// The user's answer to a write approval, for the Link task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAnswer {
    Allowed,
    Refused,
    Cancelled,
}

slow_table! {
    ROWS: [(&str, Val<()>); 5] = [
        (slow!("Bluetooth"), |Ctx { app: a, dev: d, .. }, b| {
            if d.bt {
                b.set(b"Connected");
                BLUE
            } else {
                b.set(if a.set.bt { b"On, waiting".as_slice() } else { b"Off" });
                TEXT
            }
        }),
        (slow!("Cable"), |Ctx { dev: d, .. }, b| {
            if !d.link.cable {
                b.set(b"Not connected");
                return SUB;
            }
            b.set(b"Connected"); // always trusted for writes (LINK-7)
            BLUE
        }),
        (slow!("Writes"), |Ctx { dev: d, .. }, b| {
            match d.link.write_pct {
                Some(p) => {
                    let _ = uwrite!(b, "Writing, {} %", p);
                }
                None => b.set(b"Idle"),
            }
            TEXT
        }),
        (slow!("BT writes"), |_, b| {
            b.set(b"Ask once");
            TEXT
        }),
        (slow!("Debug console"), |_, b| {
            b.set(ON_OFF[0].as_bytes());
            TEXT
        }),
    ];
}

slow_table! {
    STATUS: Table<()> = Table { rows: &ROWS, go: &[], y: 22, h: 20, style: w::KV_STATUS };
}

/// 0 9 2 2's rows: one per approval token, then Add a device.
const APPROVED_ROWS: usize = TOKENS + 1;
/// What the tokens are called: the newest is Device 1 (no names: the module never says who connected, LINK-29).
const DEVICE_NAMES: [&str; TOKENS] = ["Device 1", "Device 2", "Device 3", "Device 4"];

impl Link {
    pub fn new(page: LinkPage) -> Self {
        Link { page, row: 0 }
    }
}

impl View for Link {
    fn name(self) -> &'static str {
        match self.page {
            LinkPage::Status => "Link status",
            LinkPage::Approved => "Approved devices",
            LinkPage::Approve => "Write approval",
            LinkPage::Progress => "Writing",
        }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(match self.page {
            LinkPage::Status => b"Link".as_slice(),
            LinkPage::Approved => b"Approved",
            LinkPage::Approve => b"Approve",
            LinkPage::Progress => b"Writing",
        });
    }

    fn modal(self) -> bool {
        self.page == LinkPage::Progress // LINK-9: keys ignored but BACK
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        match (self.page, e.key, e.press) {
            (LinkPage::Approve, Key::Menu, Short) => {
                app.link_answer = Some(LinkAnswer::Allowed);
                app.back();
                app.show_toast(b"Writes allowed", b"for this device", false);
            }
            (LinkPage::Approve, Key::Exit, Short) => {
                app.link_answer = Some(LinkAnswer::Refused);
                app.back();
                app.show_toast(b"Writes refused", b"", false);
            }
            (LinkPage::Progress, Key::Exit, Short | Long) => {
                app.link_answer = Some(LinkAnswer::Cancelled);
                app.back();
                app.show_toast(b"Write cancelled", b"", false);
            }
            (LinkPage::Progress, ..) => {}
            // LINK-7: the approved devices (their tokens), then Add a device.
            (LinkPage::Approved, Key::Up | Key::Down, Short | Repeat) => {
                let n = APPROVED_ROWS as u8;
                let row = if e.key == Key::Down { (self.row + 1) % n } else { (self.row + n - 1) % n };
                app.screen = Screen::Link(Link { row, ..self });
            }
            (LinkPage::Approved, Key::Menu, Short) if (self.row as usize) < TOKENS => {
                let i = self.row as usize;
                if app.set.link_tokens[i].iter().all(|&b| b == 0) {
                    app.show_toast(b"No device here", b"", false);
                } else {
                    // Forgetting deletes the token: that app asks again at its next write.
                    app.set.link_tokens[i] = [0; 8];
                    app.show_toast(DEVICE_NAMES[i].as_bytes(), b"forgotten", false);
                }
            }
            (LinkPage::Approved, Key::Menu, Short) => app.open(Dest::AddDevice),
            (_, Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        if down && self.page != LinkPage::Progress {
            super::nav::transmit(app);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let dev = cx.dev;
        match self.page {
            LinkPage::Status => {
                app.table(lcd, &STATUS, cx.live, dev, &(), None);
                app.help_box(lcd, 160, 48, b"The web app connects by cable or Bluetooth.", b"");
            }
            LinkPage::Approved => {
                // The four approval tokens (LINK-7: the oldest gives way), then Add a device. The module tells the
                // radio nothing about the phone (LINK-29), so a device is its token: "Device 1" is the newest.
                for i in 0..APPROVED_ROWS {
                    let set = i < TOKENS && app.set.link_tokens[i].iter().any(|&b| b != 0);
                    let r = if i < TOKENS {
                        Row { num: Some(i as u8 + 1), name: DEVICE_NAMES[i].as_bytes(), value: if set { b"approved" } else { b"-" }, dot: false, selected: self.row as usize == i, grey: !set }
                    } else {
                        Row { num: Some(i as u8 + 1), name: b"Add a device", value: b">", dot: false, selected: self.row as usize == i, grey: false }
                    };
                    app.list_row(lcd, i, 23 + 22 * i as i32, 22, &r);
                }
                let help: &[u8] = b"OK on a device forgets it: it asks again at its next write.";
                app.help_box(lcd, 226, 46, help, b"MENU 0 9 2 2");
            }
            LinkPage::Approve => app.message(
                lcd,
                b"Allow writes?",
                b"Bluetooth device",
                b"It wants to change this radio's channels and settings. Allowed once, it's trusted until you forget it (MENU 0 9 2 2). Reading needs no approval.",
                AMBER,
            ),
            LinkPage::Progress => {
                let pct = dev.link.write_pct.unwrap_or(100);
                app.message(lcd, b"Writing", b"from the web app", b"Keys are ignored while the web app writes. BACK cancels.", TEXT);
                if app.dirty(R_NOTE, hash(&pct)) {
                    let mut t = Buf::<8>::new();
                    let _ = uwrite!(t, "{} %", pct);
                    w::progress(lcd, 30, 210, 180, 6, pct, BLUE);
                    w::field(lcd, 10, 220, 222, 240, 236, Align::Center, t.as_bytes(), &ferrite_ui::assets::UI_12B, TEXT, BG);
                }
            }
        }
    }

    fn hints(self, _: &App) -> Hints {
        match self.page {
            LinkPage::Approve => Hints::new(&[("OK", "Allow"), ("BACK", "Refuse")]),
            LinkPage::Progress => Hints::new(&[("BACK", "Cancel")]),
            LinkPage::Approved => Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")]),
            LinkPage::Status => Hints::new(&[("BACK", "Back")]),
        }
    }
}
