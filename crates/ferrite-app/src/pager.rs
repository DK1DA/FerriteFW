//! POCSAG pagers (manual chapter 27: DEC-4, DEC-7, DEC-18; 0 4 2): when the pager decoder is wanted, and what a
//! decoded page becomes. Shared by the firmware's UI task and the simulators.

use crate::data::{Event, Src};
use crate::App;
use ferrite_core::decoders::Page;
use ferrite_core::freq::Freq10;
use ferrite_core::msg::{LiveAll, SLOTS};
use ferrite_ui::text::Buf;
use ufmt::uwrite;

/// A slot counts as on the pager frequency within this much (half a 6.25 kHz step's worth of rounding).
const NEAR: Freq10 = 100;

impl App {
    /// The slot sitting on the pager frequency, if POCSAG is on (0 4 2 1).
    pub fn pager_slot(&self, live: &LiveAll) -> Option<u8> {
        let s = &self.set;
        if !s.pocsag_on || self.low_power() {
            return None; // PWR-24: no decoders in Low Power Mode
        }
        (0..SLOTS).find(|&i| live.slots[i].freq.abs_diff(s.pocsag_freq) <= NEAR).map(|i| i as u8)
    }

    /// Whether the DSP task should run the pager decoder (DEC-18: it gets the slot when APRS doesn't want it).
    pub fn pager_wanted(&self, live: &LiveAll) -> bool {
        self.pager_slot(live).is_some()
    }

    /// A page the decoder heard: through the capcode filter (0 4 2 3), into the inbox with a toast if Popups is on
    /// (DEC-7),
    /// and the event log record to keep (INBOX-6). None when the filter drops it.
    pub fn pager_hit(&mut self, p: &Page, live: &LiveAll) -> Option<Event> {
        let s = &self.set;
        if s.pocsag_cap != 0 && p.capcode != s.pocsag_cap {
            return None;
        }
        let slot = self.pager_slot(live).unwrap_or(0);
        let freq = s.pocsag_freq;
        let mut head = Buf::<20>::new();
        let _ = uwrite!(head, "Pager {}", p.capcode);
        let text: &[u8] = if p.len == 0 { b"(tone only)" } else { p.text() };
        self.notify_decode(head.as_bytes(), text);
        Some(Event { src: Src::Pocsag, ago_s: 0, freq, slot, unread: true, id: p.capcode, x: p.baud, y: p.function as u16, pos: None, name: Buf::new(), text: Buf::from(text) })
    }
}

#[cfg(test)]
mod tests {
    use crate::App;
    use ferrite_core::decoders::{Page, PAGE_TEXT};
    use ferrite_core::msg::LiveAll;

    struct Harness {
        app: App,
    }

    fn page(capcode: u32, text: &[u8]) -> Page {
        let mut p = Page { capcode, baud: 1200, function: 3, numeric: false, len: text.len() as u8, text: [0; PAGE_TEXT] };
        p.text[..text.len()].copy_from_slice(text);
        p
    }

    #[test]
    fn wanted_only_on_its_frequency() {
        let t = Harness { app: App::new(crate::tests::set(), crate::tests::alert()) };
        let mut live = LiveAll::default();
        live.slots[1].freq = 43_998_750;
        assert!(!t.app.pager_wanted(&live), "POCSAG off by default");
        t.app.set.pocsag_on = true;
        assert!(t.app.pager_wanted(&live));
        assert_eq!(t.app.pager_slot(&live), Some(1));
        live.slots[1].freq = 44_000_000;
        assert!(!t.app.pager_wanted(&live), "no slot on 439.9875");
    }

    #[test]
    fn capcode_filter_and_the_record() {
        let mut t = Harness { app: App::new(crate::tests::set(), crate::tests::alert()) };
        t.app.set.pocsag_on = true;
        let mut live = LiveAll::default();
        live.slots[2].freq = 43_998_750;
        let before = t.app.inbox;
        let e = t.app.pager_hit(&page(1_234_567, b"MEETING MOVED TO 1930"), &live).expect("logged");
        assert_eq!((e.id, e.x, e.y, e.slot, e.freq), (1_234_567, 1200, 3, 2, 43_998_750));
        assert_eq!(e.text.as_bytes(), b"MEETING MOVED TO 1930");
        assert_eq!(t.app.inbox, before + 1);
        t.app.set.pocsag_cap = 42;
        assert!(t.app.pager_hit(&page(1_234_567, b"NOT FOR US"), &live).is_none(), "filtered");
        assert!(t.app.pager_hit(&page(42, b""), &live).is_some_and(|e| e.text.as_bytes() == b"(tone only)"));
    }

    /// DEC-7: with Popups (0 4 7) off a page still counts in the inbox and is logged, but shows no toast.
    #[test]
    fn popups_off_logs_without_a_toast() {
        let mut t = Harness { app: App::new(crate::tests::set(), crate::tests::alert()) };
        t.app.set.pocsag_on = true;
        t.app.set.popups = false;
        let mut live = LiveAll::default();
        live.slots[0].freq = 43_998_750;
        let before = t.app.inbox;
        assert!(t.app.pager_hit(&page(7, b"QUIET"), &live).is_some(), "still logged");
        assert_eq!(t.app.inbox, before + 1);
        assert!(t.app.toast.is_none(), "no toast with Popups off");
        t.app.set.popups = true;
        t.app.pager_hit(&page(7, b"LOUD"), &live);
        assert!(t.app.toast.is_some(), "a toast with Popups on");
    }
}
