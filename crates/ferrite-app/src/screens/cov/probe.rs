//! Tone search and the frequency counter (chapter 22) as drawn: found tones (CTCSS, DCS both ways), the last
//! hits, the TX question, on a channel; the counter's hold and its hits.

use super::*;
use ferrite_core::freq::mhz;
use ferrite_core::msg::ModeLive;

fn tone_live(t: &mut T, tone: u16) {
    t.live.mode = ModeLive::Probe { freq: 0, rssi: -80, tone };
    t.wait(300);
}

/// TSRCH-2: a found tone drawn big with its kind (CTCSS in Hz, DCS normal or inverted); two tones heard list
/// as the last hits.
#[test]
fn found_tones_drawn() {
    let mut t = T::ready();
    t.open(Dest::ToneSearch);
    t.reads("title=\"Search\"");
    tone_live(&mut t, 885);
    t.shows("Found");
    t.shows("88.5");
    t.shows("CTCSS, Hz");
    tone_live(&mut t, crate::slot::DCS_FLAG | 0o23);
    t.shows("023");
    t.shows("DCS, normal");
    tone_live(&mut t, crate::slot::DCS_FLAG | crate::slot::DCS_INV | 0o754);
    t.shows("754");
    t.shows("DCS, inverted");
    t.shows("T 88.5"); // the last hits, once two differ
}

/// TSRCH-4, TSRCH-9: on a channel OK sets RX then asks about TX (hints with # Save), # saves into the record;
/// the hint bar offers Apply and Save before.
#[test]
fn on_a_channel() {
    let mut t = T::ready();
    let a = t.app.active;
    t.app.set_channel(a, 0, 0, &t.s);
    t.open(Dest::ToneSearch);
    t.shows("On a channel: OK loads, # saves");
    tone_live(&mut t, 1000);
    t.reads("hints \"OK\" \"Apply\" \"#\" \"Save\" \"BACK\" \"Cancel\"");
    t.tap(Key::Menu);
    t.shows("RX set. Set the TX tone too?");
    t.reads("hints \"OK\" \"TX too\" \"#\" \"Save\" \"BACK\" \"RX only\"");
    t.tap(Key::Hash);
    assert_eq!(t.toast(), "Tone saved / in the channel");
    assert_eq!(t.app.slot(a).cfg().rx_tone, 1000);
    // # straight from the search: the RX tone set and saved
    t.open(Dest::ToneSearch);
    t.live.mode = ModeLive::Probe { freq: 0, rssi: -80, tone: 0 };
    tone_live(&mut t, 1318);
    t.tap(Key::Hash);
    assert_eq!(t.app.slot(a).cfg().rx_tone, 1318);
}

/// TSRCH-11: the counter: HOLD freezes the reading (tagged), the live one dimmer when it's only the last hit.
#[test]
fn counter_hold() {
    let mut t = T::ready();
    t.open(Dest::Counter);
    t.reads("title=\"Counter\"");
    t.live.mode = ModeLive::Probe { freq: mhz(146) + 52_000, rssi: -60, tone: 0 };
    t.wait(300);
    t.shows("146.520");
    t.tap(Key::Hash);
    t.shows("HOLD");
    t.live.mode = ModeLive::Probe { freq: mhz(10), rssi: -60, tone: 0 }; // not countable
    t.wait(300);
    t.shows("146.520");
    t.tap(Key::Hash);
    t.wait(100);
    t.shows("146.520"); // the last hit, shown dimmer
}
