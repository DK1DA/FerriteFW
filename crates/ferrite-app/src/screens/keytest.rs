//! The key test (D9; MENU 0 9 1 2; manual chapters 7 and 36): the last key's name, code and press type, and a
//! map of every key, lit once seen. Codes come from [`App::raw_key`], which the firmware calls for every
//! matrix code, mapped or not, so the test is how the unmapped keys get their names (KEY-2). Short BACK and PTT
//! are tested like any key here: hold BACK leaves, and PTT does not transmit from this screen.

use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::{self as k, Key};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// What happened last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Got {
    Nothing,
    Down,
    Up,
    Short,
    Long,
    Repeat,
    KnobUp,
    KnobDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyTest {
    /// The last key (None: a code with no meaning yet, or the knob).
    key: Option<Key>,
    got: Got,
    /// Its matrix code, 0xFF unknown.
    code: u8,
    /// Keys seen, a bit per [`MAP`] cell (three bytes: the state stays small).
    seen: [u8; 3],
}

impl KeyTest {
    pub fn new() -> Self {
        KeyTest { key: None, got: Got::Nothing, code: 0xFF, seen: [0; 3] }
    }
}

/// The map: two blocks of 3 x 4 keys (navigation and side keys at the left, the digit pad at the right). A
/// label of one arrow byte draws as a triangle.
const KNOB: usize = 8;
const MAP: [(&str, Option<Key>); 24] = [
    ("MENU", Some(Key::Menu)), ("\x18", Some(Key::Up)), ("BACK", Some(Key::Exit)),
    ("\x1b", Some(Key::Left)), ("\x19", Some(Key::Down)), ("\x1a", Some(Key::Right)),
    ("SLOT", Some(Key::Slot)), ("V/M", Some(Key::VfoMem)), ("Knob", None),
    ("PTT", Some(Key::Ptt(0))), ("S1", Some(Key::Side(1))), ("S2", Some(Key::Side(2))),
    ("1", Some(Key::Digit(1))), ("2", Some(Key::Digit(2))), ("3", Some(Key::Digit(3))),
    ("4", Some(Key::Digit(4))), ("5", Some(Key::Digit(5))), ("6", Some(Key::Digit(6))),
    ("7", Some(Key::Digit(7))), ("8", Some(Key::Digit(8))), ("9", Some(Key::Digit(9))),
    ("*", Some(Key::Star)), ("0", Some(Key::Digit(0))), ("#", Some(Key::Hash)),
];

fn cell(key: Key) -> Option<usize> {
    MAP.iter().position(|(_, m)| *m == Some(key))
}

/// The key's name as the manual writes it.
pub fn key_name(key: Key, b: &mut Buf<16>) {
    match key {
        Key::Menu => b.set(b"MENU/OK"),
        Key::Exit => b.set(b"BACK"),
        Key::Up => b.set(b"Up"),
        Key::Down => b.set(b"Down"),
        Key::Left => b.set(b"Left"),
        Key::Right => b.set(b"Right"),
        Key::Slot => b.set(b"SLOT"),
        Key::VfoMem => b.set(b"V/M"),
        Key::Digit(d) => {
            let _ = uwrite!(b, "{}", d);
        }
        Key::Star => b.set(b"*"),
        Key::Hash => b.set(b"#"),
        Key::Ptt(_) => b.set(b"PTT"),
        Key::Side(n) => {
            let _ = uwrite!(b, "SIDE{}", n);
        }
    }
}

impl KeyTest {
    /// After `key` did `got`: its code (when known from the table), and it is seen.
    fn saw(mut self, key: Option<Key>, got: Got) -> Self {
        if key != self.key {
            // a code comes before its key (App::raw_key); a different key without one has no known code
            self.code = key.and_then(|k| (0..k::CODES as u8).find(|&c| k::key(c) == Some(k))).unwrap_or(0xFF);
        }
        self.key = key;
        self.got = got;
        if let Some(i) = key.and_then(cell) {
            self.mark(i);
        }
        self
    }

    fn mark(&mut self, i: usize) {
        self.seen[i / 8] |= 1 << (i % 8);
    }

    fn is_seen(&self, i: usize) -> bool {
        self.seen[i / 8] & 1 << (i % 8) != 0
    }
}

impl View for KeyTest {
    fn own_side1(self) -> bool {
        true
    }

    /// KEY-19: every key is under test.
    fn own_arrows(self) -> bool {
        true
    }

    fn own_hold_menu(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "Key test"
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        let got = match e.press {
            Press::Short => Got::Short,
            Press::Long => Got::Long,
            Press::Repeat => Got::Repeat,
            Press::LongUp => return, // the release after a long press: already shown as long
        };
        app.screen = Screen::KeyTest(self.saw(Some(e.key), got));
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let mut s = self;
        s.mark(KNOB);
        (s.key, s.got, s.code) = (None, if d > 0 { Got::KnobUp } else { Got::KnobDown }, 0xFF);
        app.screen = Screen::KeyTest(s);
    }

    fn raw(self, app: &mut App, key: Option<Key>, code: Option<u8>, down: bool) {
        let mut s = self.saw(key, if down { Got::Down } else { Got::Up });
        if let Some(c) = code {
            s.code = c;
        }
        app.screen = Screen::KeyTest(s);
    }

    fn ptt(self, app: &mut App, down: bool) {
        // Tested, not transmitted: the key test is where PTT's code is found.
        self.raw(app, Some(Key::PTT), None, down);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let mut name = Buf::<16>::new();
        match (self.key, self.got) {
            (_, Got::KnobUp | Got::KnobDown) => name.set(b"Knob"),
            (Some(key), _) => key_name(key, &mut name),
            (None, Got::Nothing) => name.set(b"Press a key"),
            (None, _) => name.set(b"Unmapped"),
        }
        let mut line = Buf::<32>::new();
        match self.got {
            Got::Nothing => line.set(b"Hold BACK to leave"),
            Got::KnobUp => line.set(b"Clockwise"),
            Got::KnobDown => line.set(b"Anticlockwise"),
            g => {
                line.set(match g {
                    Got::Down => b"Pressed",
                    Got::Up => b"Released",
                    Got::Short => b"Short press",
                    Got::Long => b"Long press",
                    _ => b"Repeat",
                });
                if self.code != 0xFF {
                    let _ = uwrite!(line, " \u{b7} code {}", self.code);
                }
            }
        }
        if app.dirty(R_TOP, Mix::new().h(&name).h(&line).get()) {
            w::field(lcd, 10, 220, 28, 54, 48, name.as_bytes(), const { &w::st(&UI_15B, TEXT, BG, Align::Center) });
            w::field(lcd, 10, 220, 58, 76, 72, line.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Center) });
        }
        let now = self.key.and_then(cell).or(matches!(self.got, Got::KnobUp | Got::KnobDown).then_some(KNOB));
        if app.dirty(R_ROW, Mix::new().h(&self.seen).h(&now).get()) {
            for (i, (label, _)) in MAP.iter().enumerate() {
                draw_key(lcd, i, label, self.is_seen(i), now == Some(i));
            }
        }
        let count: u32 = self.seen.iter().map(|b| b.count_ones()).sum();
        if app.dirty(R_HELP, Mix::new().h(&count).get()) {
            let mut t = Buf::<40>::new();
            let _ = uwrite!(t, "{} of {} keys seen", count, MAP.len());
            w::field(lcd, 10, 220, 222, 240, 236, t.as_bytes(), const { &w::st(&UI_12, SUB, BG, Align::Center) });
            w::wrap(lcd, 16, 208, 248, 296, 262, 15, b"PTT is tested here, not transmitted. Codes without a name show as Unmapped.", &UI_12, DIM, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("Any key", "Test"), ("Hold BACK", "Leave")])
    }
}

/// One key's box: lit (the last key) blue, seen green-edged, else grey.
fn draw_key(lcd: &mut impl Panel, i: usize, label: &str, seen: bool, lit: bool) {
    let (col, row) = ((i % 12) % 3, (i % 12) / 3);
    let x = if i < 12 { 8 } else { 126 } + 36 * col as i32;
    let y = 96 + 30 * row as i32;
    let (bg, edge, fg) = if lit { (BLUE, BLUE, BLUE_FG) } else if seen { (BG, GREEN, TEXT) } else { (BG, LINE, SUB) };
    // 25 px tall: the 9 px label and the arrows centre (DISP-19)
    ferrite_ui::canvas::banded(lcd, x, y, 34, 26, BG, |lcd| {
        lcd.fill_r(x, y, 34, 25, 1, bg);
        lcd.frame_r(x, y, 34, 25, 1, edge);
        let b = label.as_bytes();
        if b.len() == 1 && (0x18..=0x1b).contains(&b[0]) {
            lcd.tri(x + 13, y + 9, 8, [0, 1, 3, 2][(b[0] - 0x18) as usize], fg);
        } else {
            lcd.text4_aligned(x + 17, y + 17, Align::Center, b, &UI_12, fg, bg);
        }
    });
}
