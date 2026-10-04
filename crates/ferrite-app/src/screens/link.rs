//! Bluetooth and the link (manual chapter 29): link status (D17, MENU 0 9 2 3; LINK-17), paired devices (D18,
//! MENU 0 9 2 2; LINK-7), the write approval prompt (D20; LINK-7, LINK-8, SAFE-6) and a write session's
//! progress (D21; LINK-9). The Link task opens the last two with [`App::open`] (`Dest::WriteApproval`,
//! `Dest::WriteProgress`) and reads the answer from [`App::take_link_answer`].

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
    Paired,
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
    ROWS: [(&str, Val<()>); 6] = [
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
            let _ = uwrite!(b, "{} baud", d.link.baud_100 as u32 * 100);
            TEXT
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
        (slow!("Cable writes"), |_, b| {
            b.set(b"Trusted");
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

impl Link {
    pub fn new(page: LinkPage) -> Self {
        Link { page, row: 0 }
    }
}

impl View for Link {
    fn name(self) -> &'static str {
        match self.page {
            LinkPage::Status => "Link status",
            LinkPage::Paired => "Paired devices",
            LinkPage::Approve => "Write approval",
            LinkPage::Progress => "Writing",
        }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(match self.page {
            LinkPage::Status => b"Link".as_slice(),
            LinkPage::Paired => b"Paired",
            LinkPage::Approve => b"Bluetooth",
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
            (LinkPage::Paired, Key::Up | Key::Down, Short | Repeat) => {
                app.screen = Screen::Link(Link { row: self.row ^ 1, ..self });
            }
            (LinkPage::Paired, Key::Menu, Short) if self.row == 0 => app.show_toast(b"Forget", b"comes with the link task", false),
            (LinkPage::Paired, Key::Menu, Short) => app.open(Dest::PairBt),
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
            LinkPage::Paired => {
                let bt = dev.bt;
                // The connected device (approval tokens are kept by the link task, still to come), then pairing.
                let rows = [
                    Row { num: Some(1), name: b"Connected device", value: if bt { b"approved" } else { b"none" }, dot: false, selected: self.row == 0, grey: !bt },
                    Row { num: Some(2), name: b"Pair a device", value: b">", dot: false, selected: self.row == 1, grey: false },
                ];
                for (i, r) in rows.iter().enumerate() {
                    app.list_row(lcd, i, 23 + 22 * i as i32, 22, r);
                }
                let help: &[u8] = if bt {
                    b"Forget a device to make it ask again."
                } else {
                    b"Devices are approved at their first write."
                };
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
            LinkPage::Paired => Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")]),
            LinkPage::Status => Hints::new(&[("BACK", "Back")]),
        }
    }
}
