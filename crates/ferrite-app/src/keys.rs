//! Key presses from key down/up and time (manual chapter 7):
//!
//! - **KEY-3** a short press fires on release, under 500 ms; a long press fires while held, at 700 ms, and the
//!   release after it does nothing. A release between 500 and 700 ms does nothing either (an open point in the
//!   manual).
//! - **KEY-4** the four arrows repeat at 10 per second after 400 ms held; once one has repeated, its release does
//!   nothing. Arrows have no long press.
//!
//! The firmware's input sampler gives key down/up (uradio `keypad` with its own long press switched off); the
//! simulator gives the PC's key down/up. Both go through this one tracker.

use ferrite_core::keys::Key;

pub const SHORT_MS: u32 = 500;
pub const LONG_MS: u32 = 700;
pub const REPEAT_AFTER_MS: u32 = 400;
pub const REPEAT_MS: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Short,
    Long,
    Repeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub press: Press,
}

#[derive(Clone, Copy)]
struct Held {
    key: Key,
    since: u32,
    /// A long press or repeat has fired: the release does nothing.
    fired: bool,
    /// When the next repeat is due.
    next: u32,
}

/// Up to four keys held at once (SIDE2 + an arrow, # held while another key goes down, ...).
pub struct KeyTracker {
    held: [Option<Held>; 4],
}

fn repeats(k: Key) -> bool {
    matches!(k, Key::Up | Key::Down | Key::Left | Key::Right)
}

impl KeyTracker {
    pub const fn new() -> Self {
        KeyTracker { held: [None; 4] }
    }

    pub fn down(&mut self, key: Key, now: u32) {
        if self.is_held(key) {
            return; // the PC's auto-repeat, or a bounce: one press
        }
        if let Some(slot) = self.held.iter_mut().find(|h| h.is_none()) {
            *slot = Some(Held { key, since: now, fired: false, next: now.wrapping_add(REPEAT_AFTER_MS) });
        }
    }

    /// The short press, if this release makes one.
    pub fn up(&mut self, key: Key, now: u32) -> Option<KeyEvent> {
        let slot = self.held.iter_mut().find(|h| h.is_some_and(|h| h.key == key))?;
        let h = slot.take()?;
        (!h.fired && now.wrapping_sub(h.since) < SHORT_MS).then_some(KeyEvent { key, press: Press::Short })
    }

    /// Long presses and repeats that are due by `now`, oldest first.
    pub fn tick(&mut self, now: u32, mut out: impl FnMut(KeyEvent)) {
        for h in self.held.iter_mut().flatten() {
            if repeats(h.key) {
                while now.wrapping_sub(h.next) as i32 >= 0 {
                    h.fired = true;
                    h.next = h.next.wrapping_add(REPEAT_MS);
                    out(KeyEvent { key: h.key, press: Press::Repeat });
                }
            } else if !h.fired && now.wrapping_sub(h.since) >= LONG_MS {
                h.fired = true;
                out(KeyEvent { key: h.key, press: Press::Long });
            }
        }
    }

    /// The held key was used as a modifier (SIDE2 + knob): its release is not a short press.
    pub fn used(&mut self, key: Key) {
        for h in self.held.iter_mut().flatten().filter(|h| h.key == key) {
            h.fired = true;
        }
    }

    pub fn is_held(&self, key: Key) -> bool {
        self.held.iter().flatten().any(|h| h.key == key)
    }

    /// Everything released (going home, the lock).
    pub fn clear(&mut self) {
        self.held = [None; 4];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    fn run(t: &mut KeyTracker, from: u32, to: u32) -> Vec<KeyEvent> {
        let mut v = Vec::new();
        for ms in (from..=to).step_by(10) {
            t.tick(ms, |e| v.push(e));
        }
        v
    }

    #[test]
    fn short_fires_on_release_under_500() {
        let mut t = KeyTracker::new();
        t.down(Key::Menu, 0);
        assert!(run(&mut t, 0, 490).is_empty(), "nothing while held");
        assert_eq!(t.up(Key::Menu, 499), Some(KeyEvent { key: Key::Menu, press: Press::Short }));
        t.down(Key::Menu, 1000);
        assert_eq!(t.up(Key::Menu, 1500), None, "500 ms is no longer short");
    }

    #[test]
    fn long_fires_while_held_at_700() {
        let mut t = KeyTracker::new();
        t.down(Key::Slot, 0);
        assert!(run(&mut t, 0, 690).is_empty());
        assert_eq!(run(&mut t, 700, 700), [KeyEvent { key: Key::Slot, press: Press::Long }]);
        assert!(run(&mut t, 710, 3000).is_empty(), "once");
        assert_eq!(t.up(Key::Slot, 3000), None, "the release after a long press does nothing");
    }

    #[test]
    fn between_500_and_700_nothing() {
        let mut t = KeyTracker::new();
        t.down(Key::Hash, 0);
        assert!(run(&mut t, 0, 600).is_empty());
        assert_eq!(t.up(Key::Hash, 600), None);
    }

    #[test]
    fn arrows_repeat_at_10_per_second_after_400() {
        let mut t = KeyTracker::new();
        t.down(Key::Up, 0);
        assert!(run(&mut t, 0, 390).is_empty());
        let v = run(&mut t, 400, 1390);
        assert_eq!(v.len(), 10, "400..1390 ms: 10 repeats");
        assert!(v.iter().all(|e| e.press == Press::Repeat && e.key == Key::Up));
        assert_eq!(t.up(Key::Up, 1400), None, "no extra step on release");
        t.down(Key::Down, 2000);
        assert_eq!(t.up(Key::Down, 2100), Some(KeyEvent { key: Key::Down, press: Press::Short }));
    }

    #[test]
    fn two_keys_held() {
        let mut t = KeyTracker::new();
        t.down(Key::Side(2), 0);
        t.down(Key::Up, 50);
        t.down(Key::Up, 60); // auto-repeat from the PC: ignored
        assert!(t.is_held(Key::Side(2)));
        assert_eq!(t.up(Key::Up, 100).map(|e| e.key), Some(Key::Up));
        assert!(!t.is_held(Key::Up));
        assert_eq!(run(&mut t, 700, 700), [KeyEvent { key: Key::Side(2), press: Press::Long }]);
    }
}
