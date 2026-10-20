//! The decoders' side of the UI task (manual chapters 23 and 27; roadmap 6b), beside the APRS task:
//!
//! - **SAME:** tells the Radio task whether weather alerts are on (0 4 9 1, US and Canada only: WX-3); its BK4829
//!   receiver then listens on any slot tuned to a weather channel (DEC-19). A header it decodes comes back on
//!   `ALERTS`: the alert screen (WX-4) and an event log record (INBOX-6).
//! - **POCSAG:** asks the DSP task for the pager decoder while POCSAG is on and a slot sits on its frequency
//!   (DEC-18: APRS's AFSK keeps the slot first); each page it decodes goes through the capcode filter into the
//!   event log, the inbox and, with popups on, a toast (DEC-4, DEC-7).

use super::dsp::PAGES;
use crate::chan::{DSP, RADIO};
use crate::store::Store;
use ferrite_app::data::{Event, Src};
use ferrite_app::App;
use ferrite_core::msg::{DspCmd, LiveAll, RadioCmd, SameDecoded};
use ferrite_ui::text::Buf;

pub struct Decode {
    /// What the Radio task and the DSP task were last told (None: not yet).
    same: Option<bool>,
    pager: Option<bool>,
}

impl Decode {
    pub const fn new() -> Self {
        Decode { same: None, pager: None }
    }

    /// One turn of the UI task's loop.
    pub fn poll(&mut self, app: &mut App, store: &Store, live: &LiveAll) {
        let same = app.settings().same_on();
        if self.same != Some(same) && RADIO.try_send(RadioCmd::Same(same)).is_ok() {
            self.same = Some(same);
        }
        let pager = app.pager_wanted(live);
        if self.pager != Some(pager) && DSP.try_send(DspCmd::Pager(pager)).is_ok() {
            self.pager = Some(pager);
        }
        while let Ok(p) = PAGES.try_receive() {
            if let Some(e) = app.pager_hit(&p, live) {
                store.log(&e);
            }
        }
    }

    /// The waiting SAME header (ALERTS), handled in this frame, not the UI task's (SBAR-23).
    #[inline(never)]
    pub fn take_alert(app: &mut App, store: &Store) {
        if let Ok(a) = crate::chan::ALERTS.try_receive() {
            Self::alert(app, store, &a);
        }
    }

    /// A SAME header from the Radio task (ALERTS): the alert, and its record in the event log.
    pub fn alert(app: &mut App, store: &Store, a: &SameDecoded) {
        app.same_alert(&a.header, a.slot, a.freq);
        if app.settings().same_on() {
            let name = ferrite_core::same::event_name(&a.header.event).unwrap_or("Alert");
            store.log(&Event { src: Src::Same, ago_s: 0, freq: a.freq, slot: a.slot, unread: true, id: 0, x: 0, y: 0, pos: None, name: Buf::from(name.as_bytes()), text: Buf::from(&a.header.event[..]) });
        }
    }
}
