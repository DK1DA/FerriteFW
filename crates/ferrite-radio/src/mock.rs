//! The recording mock: every pin, PA drive value, chip step, wait and register write, with the time it happened.

extern crate std;

use crate::run::{Hw, Pin};
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use ferrite_core::path::Sel;
use std::vec::Vec;

/// Run a future that never really waits (the mock's waits only move its clock).
pub fn block_on<F: Future>(f: F) -> F::Output {
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// One recorded step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    Pin(Pin, bool),
    /// The PA drive (DAC2) value.
    Pa(u8),
    Wait(u32),
    Key(u8),
    Fault(u8),
    Unkey(u8),
    Muted(bool),
    EndTone,
    CwId,
    Tail,
}

/// A radio whose hardware only records.
pub struct RecHw {
    pub t: u32,
    pub log: Vec<(u32, Ev)>,
    pub relay: bool,
    pub si_used: bool,
    /// The read-back result `key_chip` gives.
    pub key_ok: bool,
    pub pa_level: u8,
}

impl RecHw {
    pub fn new() -> Self {
        RecHw { t: 0, log: Vec::new(), relay: false, si_used: false, key_ok: true, pa_level: 200 }
    }

    fn rec(&mut self, e: Ev) {
        self.log.push((self.t, e));
    }

    /// The events only, in order.
    pub fn evs(&self) -> Vec<Ev> {
        self.log.iter().map(|&(_, e)| e).collect()
    }

    /// Where the first event matching `f` is.
    pub fn at(&self, f: impl Fn(&Ev) -> bool) -> Option<usize> {
        self.log.iter().position(|(_, e)| f(e))
    }

    /// The time of event `i`.
    pub fn time(&self, i: usize) -> u32 {
        self.log[i].0
    }

    /// Replay the pins and check the states the radio must never be in (TX-27, TX-28, SAFE-21, decision 95),
    /// from `start`: never two selects but crossband's RX + TX on different chips, never RX and TX on one chip,
    /// a TX select only with the PA at zero when it comes on, the PA up only with one TX select on and its chip
    /// keyed, the relay moving only with no select on, the PA at zero and no chip keyed, the repeat audio only
    /// with a chip keyed and the PA up.
    pub fn check(&self, start: &State) -> State {
        let mut s = *start;
        for (i, &(_, e)) in self.log.iter().enumerate() {
            let at = |what: &str| std::format!("event {i} {e:?}: {what}\n{:?}", self.evs());
            match e {
                Ev::Pin(Pin::Sel(sel), on) => {
                    if on {
                        assert!(!sel.is_tx() || s.pa == 0, "{}", at("TX select with PA drive"));
                    }
                    s.sel[sel as usize] = on;
                    let n = s.sel.iter().filter(|&&x| x).count();
                    let on_sels: Vec<Sel> = Sel::ALL.iter().copied().filter(|x| s.sel[*x as usize]).collect();
                    let ok = n <= 1 || (n == 2 && on_sels[0].is_tx() != on_sels[1].is_tx() && on_sels[0].chip() != on_sels[1].chip());
                    assert!(ok, "{}", at("selects on together"));
                }
                Ev::Pin(Pin::Relay, r) => {
                    if r != s.relay {
                        assert!(s.sel.iter().all(|&x| !x), "{}", at("relay with a select on"));
                        assert!(s.pa == 0 && s.keyed.is_none(), "{}", at("relay with RF present"));
                    }
                    s.relay = r;
                }
                Ev::Pin(Pin::RepeatAudio, high) => {
                    if !high {
                        assert!(s.keyed.is_some() && s.pa > 0, "{}", at("repeat audio unkeyed"));
                    }
                    s.repeat = !high;
                }
                Ev::Pa(v) => {
                    if v > 0 {
                        let tx: Vec<Sel> = Sel::ALL.iter().copied().filter(|x| x.is_tx() && s.sel[*x as usize]).collect();
                        assert!(tx.len() == 1, "{}", at("PA up without one TX select"));
                        assert_eq!(s.keyed, Some(tx[0].chip()), "{}", at("PA up before its chip keys"));
                    }
                    s.pa = v;
                }
                Ev::Key(n) => s.keyed = Some(n),
                Ev::Unkey(n) => {
                    assert_eq!(s.pa, 0, "{}", at("chip unkeyed with PA drive"));
                    assert!(!s.repeat, "{}", at("chip unkeyed with the repeat audio on"));
                    // The safe state after a failed read-back unkeys a chip that never counted as keyed.
                    assert!(s.keyed.is_none_or(|k| k == n), "{}", at("unkey of another chip"));
                    s.keyed = None;
                }
                _ => {}
            }
        }
        s
    }
}

/// The pins as [`RecHw::check`] replays them.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub sel: [bool; 6],
    pub relay: bool,
    pub pa: u8,
    pub keyed: Option<u8>,
    pub repeat: bool,
}

impl Hw for RecHw {
    fn pin(&mut self, p: Pin, high: bool) {
        if p == Pin::Relay {
            self.relay = high;
        }
        self.rec(Ev::Pin(p, high));
    }
    fn relay_is(&mut self) -> bool {
        self.relay
    }
    async fn wait_ms(&mut self, ms: u32) {
        self.rec(Ev::Wait(ms));
        self.t += ms;
    }
    fn muted(&mut self, m: bool) {
        self.rec(Ev::Muted(m));
    }
    fn si_used(&mut self) -> bool {
        self.si_used
    }
    fn set_si_used(&mut self) {
        self.si_used = true;
    }
    fn pa_zero(&mut self) {
        self.rec(Ev::Pa(0));
    }
    fn pa_set(&mut self, v: u8) {
        self.rec(Ev::Pa(v));
    }
    fn pa_level(&mut self) -> u8 {
        self.pa_level
    }
    fn key_chip(&mut self, n: u8) -> bool {
        if self.key_ok {
            self.rec(Ev::Key(n));
        }
        self.key_ok
    }
    fn chip_fault(&mut self, n: u8) {
        self.rec(Ev::Fault(n));
    }
    fn unkey_chip(&mut self, n: u8) {
        self.rec(Ev::Unkey(n));
    }
    async fn end_tone(&mut self) {
        self.rec(Ev::EndTone);
    }
    async fn cw_id(&mut self) {
        self.rec(Ev::CwId);
    }
    async fn tail_elim(&mut self) {
        self.rec(Ev::Tail);
    }
}

/// A BK4829 that keeps its registers and records every write.
pub struct RecBus {
    pub regs: [u16; 128],
    pub writes: Vec<(u8, u16)>,
    /// Reads of this register give this value whatever was written (a failing read-back).
    pub stuck: Option<(u8, u16)>,
}

impl RecBus {
    pub fn new() -> Self {
        RecBus { regs: [0; 128], writes: Vec::new(), stuck: None }
    }

    /// The last value written to `reg`.
    pub fn last(&self, reg: u8) -> Option<u16> {
        self.writes.iter().rev().find(|w| w.0 == reg).map(|w| w.1)
    }

    /// Where the first write to `reg` is.
    pub fn first(&self, reg: u8) -> Option<usize> {
        self.writes.iter().position(|w| w.0 == reg)
    }
}

impl bk4829::Bus for RecBus {
    fn write(&mut self, reg: u8, value: u16) {
        self.regs[reg as usize & 0x7F] = value;
        self.writes.push((reg, value));
    }
    fn read(&mut self, reg: u8) -> u16 {
        match self.stuck {
            Some((r, v)) if r == reg => v,
            _ => self.regs[reg as usize & 0x7F],
        }
    }
}
