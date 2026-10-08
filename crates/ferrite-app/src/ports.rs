//! The two remote ports, the programming cable and Bluetooth (manual chapters 26, 29 and 37, roadmap 3d), and
//! what runs on them: the terminal ([`Term`]), the link protocol ([`Session`]) and the KISS TNC. The firmware
//! and the simulators feed both ports' bytes through here, so both run the same rules.
//!
//! - **One terminal and one link session**, held by one port at a time (RAM: the terminal and the session are
//!   about 2 KB together, so there is no second set). The cable always wins: physical access (LINK-7), and the
//!   way back if a phone left a session open. Bluetooth takes them when the cable has neither open; while the
//!   cable has them, Bluetooth's terminal and link bytes are dropped.
//! - **KISS** needs neither, so it runs on its port alongside the other one (KISS-6): the cable by 0 9 2 5 on
//!   KISS or 0 5 9 1 on Cable, Bluetooth by 0 5 9 1 on Bluetooth. Its bytes go back to the caller.
//! - **Trust:** the cable is trusted (TERM-3, LINK-7); on Bluetooth the link and the terminal share one
//!   approval (TERM-25), and `BAUD` can't change the rate (LINK-25: 115200 only).
//! - **A Bluetooth connection** ends when it has been quiet for [`BT_IDLE_MS`]: the module tells the radio
//!   nothing about connects (it is a transparent bridge, LINK-25), so silence is the only sign the phone left.
//!   Its terminal closes and its approval ends with it.

use crate::chan::ChannelStore;
use crate::link::{self, Route, Session};
use crate::term::{Sink, Term};
use crate::App;
use ferrite_core::msg::{Device, LiveAll};

/// Bluetooth's only rate (LINK-25, decision 79).
pub const BT_BAUD: u32 = 115_200;
/// A Bluetooth connection is taken as over after this long with no byte from it.
pub const BT_IDLE_MS: u32 = 120_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    Cable,
    Bt,
}

/// Where a byte from a port goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    Term,
    Link,
    /// The KISS TNC has this port (KISS-6): the caller's KISS decoder.
    Kiss,
    /// The other port holds the terminal and the link: dropped.
    Drop,
}

pub struct Ports {
    pub term: Term,
    pub link: Session,
    owner: Port,
    /// The cable's fastest rate (LINK-11), given back to the session when the cable takes it again.
    cable_max: u32,
    /// When Bluetooth last sent a byte, and whether it has since the connection started.
    bt_last: u32,
    bt_seen: bool,
    /// Bluetooth bytes dropped while the cable held the terminal or the link.
    pub dropped: u32,
}

impl Default for Ports {
    fn default() -> Self {
        Self::new()
    }
}

impl Ports {
    pub const fn new() -> Self {
        Ports { term: Term::new(), link: Session::new(true), owner: Port::Cable, cable_max: BT_BAUD, bt_last: 0, bt_seen: false, dropped: 0 }
    }

    /// The fastest cable rate the caller's UART runs within 2 % (LINK-11).
    pub fn set_cable_max_baud(&mut self, b: u32) {
        self.cable_max = b;
        if self.owner == Port::Cable {
            self.link.set_max_baud(b);
        }
    }

    /// Which port holds the terminal and the link now.
    pub fn owner(&self) -> Port {
        self.owner
    }

    /// Takes another byte now (TERM-4, TERM-13: a whole line or frame waits for the store).
    pub fn ready(&self) -> bool {
        self.term.ready() && self.link.ready()
    }

    /// The terminal or the link is in use on the owner's port.
    fn held(&self, now: u32) -> bool {
        self.term.on() || self.link.active(now) || self.link.progress().is_some()
    }

    fn switch(&mut self, port: Port) {
        self.owner = port;
        self.term = Term::new();
        self.term.set_bluetooth(port == Port::Bt);
        self.term.set_writable(port == Port::Cable);
        self.link.set_port(port == Port::Cable, if port == Port::Cable { self.cable_max } else { BT_BAUD });
    }

    /// Where byte `b` from `port` goes (TERM-2 on the cable; auto on Bluetooth). It may move the terminal and the
    /// link to `port`: always for the cable, for Bluetooth when the cable has neither in use.
    pub fn route(&mut self, app: &App, port: Port, b: u8, now: u32) -> To {
        if port == Port::Bt {
            (self.bt_last, self.bt_seen) = (now, true);
        }
        let kiss = match port {
            Port::Cable => app.set.kiss_on_cable(),
            Port::Bt => app.set.kiss_on_bt(),
        };
        if kiss {
            return To::Kiss;
        }
        if port != self.owner {
            if port == Port::Bt && self.held(now) {
                self.dropped = self.dropped.wrapping_add(1);
                return To::Drop;
            }
            self.switch(port);
        }
        let mode = if port == Port::Cable { app.set.cable_port } else { 0 };
        match link::route(mode, self.link.active(now), b) {
            Route::Link => To::Link,
            Route::Term => To::Term,
            Route::Drop => To::Kiss,
        }
    }

    /// A routed byte to the terminal or the link (call the App's `pop_write` after each: a channel edit writes
    /// one channel per key).
    pub fn feed<S: ChannelStore>(&mut self, app: &mut App, store: &S, to: To, b: u8, now: u32) {
        match to {
            To::Term => self.term.feed(app, b, now, store),
            To::Link => self.link.feed(b, now),
            To::Kiss | To::Drop => {}
        }
    }

    /// The terminal redrawn and a waiting link frame answered, into the owner's output. Ends a Bluetooth
    /// connection gone quiet, and passes the terminal's write requests to the session's approval (TERM-25).
    #[allow(clippy::too_many_arguments)]
    pub fn poll<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, live: &LiveAll, now: u32, cable: &mut impl Sink, bt: &mut impl Sink) {
        if self.bt_seen && (now.wrapping_sub(self.bt_last) >= BT_IDLE_MS || !app.set.bt) {
            self.bt_seen = false;
            if self.owner == Port::Bt {
                self.switch(Port::Cable);
            }
        }
        if self.term.take_write_ask() {
            self.link.ask_for_terminal(app);
        }
        self.term.set_writable(self.link.may_write_now());
        match self.owner {
            Port::Cable => {
                self.term.poll(app, store, dev, live, now, cable);
                self.link.poll(app, store, now, cable);
            }
            Port::Bt => {
                self.term.poll(app, store, dev, live, now, bt);
                self.link.poll(app, store, now, bt);
            }
        }
    }

    /// A rate agreed by `BAUD` for the cable's UART, once (LINK-11); Bluetooth's never changes (LINK-25).
    pub fn take_baud(&mut self) -> Option<u32> {
        let b = self.link.take_baud();
        b.filter(|_| self.owner == Port::Cable)
    }

    /// The link protocol is running on `port` now (TERM-2: for its flow control and the Link status page).
    pub fn link_on(&self, port: Port, now: u32) -> bool {
        self.owner == port && self.link.active(now)
    }

    /// A Bluetooth device is connected, as far as the radio can tell: Bluetooth is on and the device has sent
    /// something within [`BT_IDLE_MS`] (the status bar's icon, SBAR).
    pub fn bt_connected(&self, app: &App, now: u32) -> bool {
        app.set.bt && self.bt_seen && now.wrapping_sub(self.bt_last) < BT_IDLE_MS
    }
}
