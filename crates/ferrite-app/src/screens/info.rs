//! The system's read-only pages, one module because they are one pattern (a title, label + value rows, a note
//! at the foot): About and its self-test detail (D7; chapter 36, SELF-5 to SELF-7), calibration (D8; CAL-5 to
//! CAL-7), advanced squelch (D10; SQL-1, SQL-2: read only for now), SSB patch status (D16; HF-20, SELF-6), the
//! SSB patch confirm at boot (D15; INST-12), add a device (D19), power off (D22; PWR-17), empty battery
//! (D23; PWR-16) and the mailbox (D24; chapter 38, no menu number yet).
//!
//! Each page is data ([`def`]): its rows are a [`super::table`] table naming a value function, so adding a row is
//! one line.

use super::groups::cal_text;
use super::help::{self, Help};
use super::shared::*;
use super::table::{slow_table, Ctx, Table, Val};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::{CalState, Device};
use ferrite_core::VERSION;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::{Buf, Zero};
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    About,
    Checks,
    /// HELP-1: the Help pages between About and the self-test.
    Help(Help),
    Calibration,
    AdvSquelch,
    SsbPatch,
    SsbConfirm,
    PowerOff,
    EmptyBattery,
    Mailbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Info {
    pub page: Page,
}

type InfoRow = (&'static str, Val<()>);

/// A page: status-bar name, rows, the note at the foot, hints.
struct PageDef {
    title: &'static str,
    rows: &'static [InfoRow],
    note: &'static str,
    hints: &'static [(&'static str, &'static str)],
}

fn fixed(b: &mut Buf<40>, s: &str, c: u16) -> u16 {
    b.set(s.as_bytes());
    c
}

/// Why transmit is locked, if it is (BOOT-11, SELF-2).
pub(crate) fn tx_lock(dev: &Device) -> Option<&'static str> {
    let t = &dev.selftest;
    if !dev.tx_built {
        Some("RX-only build")
    } else if !t.mcu_ok {
        Some("Locked: MCU check")
    } else if t.cal != CalState::Ok {
        Some("Locked: calibration")
    } else if dev.battery_cutoff {
        Some("Battery too low")
    } else {
        None
    }
}

/// The self-test's checks: name and result (None = not run).
fn checks(dev: &Device) -> [(&'static str, Option<bool>); 6] {
    let t = &dev.selftest;
    let read = |v: u32, ok: bool| (v != 0).then_some(ok);
    [
        ("MCU", read(t.idcode, t.mcu_ok)),
        ("SPI flash", read(t.flash_id, t.flash_mb > 0)),
        ("BK4829 VHF", Some(t.chips & 1 != 0)),
        ("BK4829 UHF", Some(t.chips & 2 != 0)),
        // The SI4732 isn't tested at boot (it stays powered down until FM or HF mode): "Not run" until it answers.
        ("SI4732", t.si4732.then_some(true)),
        ("Calibration", (t.cal != CalState::Unknown).then_some(t.cal == CalState::Ok)),
    ]
}

fn check_row(i: usize, d: &Device, b: &mut Buf<40>) -> u16 {
    match checks(d)[i].1 {
        Some(true) => fixed(b, "OK", GREEN),
        Some(false) => fixed(b, "Failed", RED),
        None => fixed(b, "Not run", SUB),
    }
}

fn volts(b: &mut Buf<40>, mv: u16) {
    let _ = uwrite!(b, "{}.{} V", mv / 1000, Zero((mv % 1000 / 10) as u32, 2));
}

slow_table! {
    ABOUT: [InfoRow; 10] = [
        // SELF-5: version and build (the git commit; "+" built from uncommitted changes).
        (slow!("Version"), |_, b| {
            let _ = uwrite!(b, "{} {}", VERSION, ferrite_core::build());
            TEXT
        }),
        (slow!("MCU"), |Ctx { dev: d, .. }, b| {
            let t = &d.selftest;
            match (t.idcode, t.mcu_ok) {
                (0, _) => fixed(b, slow!("Not read"), SUB),
                (_, true) => fixed(b, slow!("AT32F403A"), TEXT),
                (id, false) => {
                    let _ = uwrite!(b, "Unknown {:08X}", id);
                    RED
                }
            }
        }),
        (slow!("SPI flash"), |Ctx { dev: d, .. }, b| {
            let t = &d.selftest;
            if t.flash_id == 0 {
                return fixed(b, slow!("No answer"), RED);
            }
            let _ = uwrite!(b, "{} MB \u{b7} {:06X}", t.flash_mb, t.flash_id);
            TEXT
        }),
        (slow!("Calibration"), |Ctx { dev: d, .. }, b| fixed(b, cal_text(d.selftest.cal), if d.selftest.cal == CalState::Ok { TEXT } else { AMBER })),
        (slow!("Transmit"), |Ctx { dev: d, .. }, b| match tx_lock(d) {
            None => fixed(b, slow!("Ready"), GREEN),
            Some(why) => fixed(b, why, AMBER),
        }),
        (slow!("Self-test"), |Ctx { dev: d, .. }, b| {
            let c = checks(d);
            let ok = c.iter().filter(|x| x.1 == Some(true)).count();
            let failed = c.iter().any(|x| x.1 == Some(false));
            let _ = uwrite!(b, "{} of {} passed", ok, c.len());
            if failed { RED } else { TEXT }
        }),
        (slow!("Battery"), |Ctx { dev: d, .. }, b| {
            volts(b, d.battery_mv); // the trim is on Calibration and trims
            TEXT
        }),
        // SBAR-20: the MCU's die temperature (the Radio task reads it every 10 s).
        (slow!("MCU temp"), |Ctx { dev: d, .. }, b| match d.mcu_temp {
            Some(c) => {
                let _ = uwrite!(b, "{} \u{b0}C", c);
                TEXT
            }
            None => fixed(b, slow!("Not read yet"), SUB),
        }),
        (slow!("Uptime"), |Ctx { app: a, .. }, b| {
            let m = a.now / 60_000;
            let _ = uwrite!(b, "{} h {} min", m / 60, m % 60);
            TEXT
        }),
        (slow!("SSB patch"), |Ctx { dev: d, .. }, b| if d.selftest.ssb_patch { fixed(b, slow!("Installed"), TEXT) } else { fixed(b, slow!("Not installed"), SUB) }),
    ];
}

slow_table! {
    CHECKS: [InfoRow; 6] = [
        (slow!("MCU"), |Ctx { dev: d, .. }, b| check_row(0, d, b)),
        (slow!("SPI flash"), |Ctx { dev: d, .. }, b| check_row(1, d, b)),
        (slow!("BK4829 VHF"), |Ctx { dev: d, .. }, b| check_row(2, d, b)),
        (slow!("BK4829 UHF"), |Ctx { dev: d, .. }, b| check_row(3, d, b)),
        (slow!("SI4732"), |Ctx { dev: d, .. }, b| check_row(4, d, b)),
        (slow!("Calibration"), |Ctx { dev: d, .. }, b| check_row(5, d, b)),
    ];
}

slow_table! {
    CALIBRATION: [InfoRow; 3] = [
        (slow!("Status"), |Ctx { dev: d, .. }, b| fixed(b, cal_text(d.selftest.cal), if d.selftest.cal == CalState::Ok { GREEN } else { AMBER })),
        (slow!("CRC at install"), |Ctx { dev: d, .. }, b| match d.selftest.cal {
            CalState::CrcMismatch => fixed(b, slow!("Changed"), RED),
            CalState::Unknown => fixed(b, slow!("Not checked"), SUB),
            _ => fixed(b, slow!("Matches"), GREEN),
        }),
        (slow!("User trims"), |_, b| fixed(b, slow!("MENU 0 9 7"), SUB)),
    ];
}

slow_table! {
    ADV_SQUELCH: [InfoRow; 3] = [
        (slow!("Thresholds"), |_, b| fixed(b, slow!("From calibration"), SUB)),
        (slow!("Squelch tail"), |_, b| fixed(b, slow!("150 ms"), TEXT)),
        (slow!("Levels"), |_, b| fixed(b, slow!("0-9 per slot"), TEXT)),
    ];
}

slow_table! {
    SSB_PATCH: [InfoRow; 3] = [
        (slow!("State"), |Ctx { dev: d, .. }, b| if d.selftest.ssb_patch { fixed(b, slow!("Verified"), GREEN) } else { fixed(b, slow!("Not installed"), AMBER) }),
        (slow!("Loads"), |_, b| fixed(b, slow!("Entering HF mode"), TEXT)),
        (slow!("Source"), |_, b| fixed(b, slow!("Your stock .BTF"), TEXT)),
    ];
}

slow_table! {
    MAILBOX: [InfoRow; 4] = [
        (slow!("Mailbox"), |_, b| fixed(b, slow!("Off"), TEXT)),
        (slow!("SSID"), |Ctx { app: a, .. }, b| {
            if a.set.callsign.is_empty() {
                return fixed(b, slow!("Needs a callsign"), AMBER);
            }
            b.set(a.set.callsign.as_bytes());
            b.push(b"-1");
            TEXT
        }),
        (slow!("Bulletins"), |_, b| fixed(b, slow!("On"), TEXT)),
        (slow!("Messages"), |_, b| fixed(b, slow!("None"), SUB)),
    ];
}

const NONE: [InfoRow; 0] = [];

/// HELP-1: About's pages in ◀▶ order; ▶ goes on, ◀ back, round.
const CYCLE: [Page; 7] = [
    Page::About,
    Page::Help(Help::Manual),
    Page::Help(Help::HomeKeys),
    Page::Help(Help::SideKeys),
    Page::Help(Help::Digits),
    Page::Help(Help::MenuNos),
    Page::Checks,
];

const BACK_ONLY: [(&str, &str); 1] = [("BACK", "Back")];

fn def(p: Page) -> PageDef {
    match p {
        Page::About => PageDef {
            title: "About",
            rows: &ABOUT,
            note: "Updating: hold PF1 + PF2 while switching on, then use the web flasher.",
            hints: &[(ARROWS_LR, "Help"), ("BACK", "Back")],
        },
        Page::Checks => PageDef {
            title: "Self-test",
            rows: &CHECKS,
            note: "A failed check never stops receive. Transmit needs the MCU and calibration checks to pass.",
            hints: &[(ARROWS_LR, "About"), ("BACK", "Back")],
        },
        // The hint bar names the page ▶ goes to.
        Page::Help(h) => {
            let (title, hints): (_, &'static [(&str, &str)]) = match h {
                Help::Manual => ("Manual", &[(ARROWS_LR, "Keys"), ("BACK", "Back")]),
                Help::HomeKeys => ("Home keys", &[(ARROWS_LR, "Side keys"), ("BACK", "Back")]),
                Help::SideKeys => ("Side keys", &[(ARROWS_LR, "Shortcuts"), ("BACK", "Back")]),
                Help::Digits => ("Shortcuts", &[(ARROWS_LR, "Menu numbers"), ("BACK", "Back")]),
                Help::MenuNos => ("Menu numbers", &[(ARROWS_LR, "Self-test"), ("BACK", "Back")]),
            };
            PageDef { title, rows: &NONE, note: "", hints }
        }
        Page::Calibration => PageDef {
            title: "Calibrate",
            rows: &CALIBRATION,
            note: "Read only: Ferrite never writes the factory calibration. A failed check locks transmit on every band.",
            hints: &BACK_ONLY,
        },
        Page::AdvSquelch => PageDef {
            title: "Squelch",
            rows: &ADV_SQUELCH,
            note: "Noise open, ceiling, RSSI open, glitch and hysteresis per band come from the calibration. Editing comes later.",
            hints: &BACK_ONLY,
        },
        Page::SsbPatch => PageDef {
            title: "SSB patch",
            rows: &SSB_PATCH,
            note: "Import it from your own stock firmware with the flasher (--ssb-from) or the web app. Without it HF mode has AM only.",
            hints: &BACK_ONLY,
        },
        Page::SsbConfirm => PageDef {
            title: "SSB patch",
            rows: &NONE,
            note: "",
            hints: &[("OK", "Use it"), ("BACK", "Not now")],
        },
        Page::PowerOff | Page::EmptyBattery => PageDef { title: "Power", rows: &NONE, note: "", hints: &[] },
        Page::Mailbox => PageDef {
            title: "Mailbox",
            rows: &MAILBOX,
            note: "Callers connect to your callsign-SSID at 1200 baud to leave and read mail. Settings come with the packet station.",
            hints: &BACK_ONLY,
        },
    }
}

/// Pages that are a message, not rows: heading, text lines, colour.
fn message(p: Page, dev: &Device, b: &mut Buf<40>) -> Option<(&'static str, &'static str, u16)> {
    Some(match p {
        Page::SsbConfirm => (
            "Unknown SSB patch",
            "The patch in flash doesn't match a known stock version. Use it for USB, LSB and CW? You can change this in MENU 0 9 6.",
            AMBER,
        ),
        Page::PowerOff => ("Saving...", "Finishing writes, then off.", TEXT),
        Page::EmptyBattery => {
            volts(b, dev.battery_mv);
            ("Battery empty", "Switching off safely.", RED)
        }
        _ => return None,
    })
}

impl View for Info {
    /// KEY-19: ◀▶ go through About, Help and the self-test (HELP-1).
    fn own_arrows(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        def(self.page).title
    }

    fn modal(self) -> bool {
        matches!(self.page, Page::PowerOff | Page::EmptyBattery)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        if e.press != Press::Short || self.modal() {
            return;
        }
        match (self.page, e.key) {
            (p, Key::Left | Key::Right) if CYCLE.contains(&p) => {
                let i = CYCLE.iter().position(|&c| c == p).unwrap_or(0) as i32;
                let d = if e.key == Key::Right { 1 } else { -1 };
                app.screen = Screen::Info(Info { page: CYCLE[(i + d).rem_euclid(CYCLE.len() as i32) as usize] });
            }
            (Page::SsbConfirm, Key::Menu) => {
                app.back();
                app.show_toast(b"SSB patch", b"in use", false);
            }
            (_, Key::Exit) => app.back(),
            _ => {}
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        if down && !self.modal() {
            super::nav::transmit(app);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let d = def(self.page);
        app.new_page(lcd, hash(&self.page));
        if let Page::Help(h) = self.page {
            return help::render(app, lcd, h);
        }
        let mut b = Buf::<40>::new();
        if let Some((head, text, c)) = message(self.page, cx.dev, &mut b) {
            app.message(lcd, head.as_bytes(), b.as_bytes(), text.as_bytes(), c);
            if self.page == Page::PowerOff {
                // A bar that moves while writes finish (PWR-17).
                let pct = ((app.now / 40) % 100) as u8;
                if app.dirty(R_NOTE, hash(&pct)) {
                    w::progress(lcd, 60, 240, 120, 4, pct, BLUE);
                }
            }
            return;
        }
        app.table(lcd, &Table { rows: d.rows, go: &[], y: 22, h: 20, style: w::KV_STATUS }, cx.live, cx.dev, &(), None);
        if !d.note.is_empty() && app.dirty(R_HELP, hash(&self.page)) {
            let y = 30 + 20 * d.rows.len() as i32;
            let h = (292 - y).min(110);
            lcd.fill(0, y - 6, 240, 300 - y + 6, BG);
            lcd.fill_r(8, y, 224, h, 2, SURFACE);
            w::wrap(lcd, 16, 208, y + 3, y + h - 3, y + 16, 15, d.note.as_bytes(), &UI_12, TEXT, SURFACE);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(def(self.page).hints)
    }
}
