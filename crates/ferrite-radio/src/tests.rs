//! What the radio is commanded to do, checked on the PC: real plans from `ferrite-core` carried out by [`run`]
//! on the recording mock, and the chip set-ups on a recording BK4829 bus. The physical side (RF, audio, timing on
//! a scope) stays the bench's.

extern crate std;

use crate::bt::BtPower;
use crate::chip::{self, RxSetup};
use crate::gps::{GpsPower, QUIET_MS};
use crate::mock::{block_on, Ev, RecBus, RecHw, State};
use crate::run::{run, Pin, PA_RAMP_STEPS};
use bk4829::{reg, Bk4829};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::path::{Cause, Path, Rx, Sel, Steps, UNKEY_WAIT_MS, XB_AUDIO_DELAY_MS};
use ferrite_core::txplan::Region;
use ferrite_core::txseq::{Cfg, Kind, Req, TxSeq};
use std::vec::Vec;

const VHF: Freq10 = mhz(146) + 52_000;
const UHF: Freq10 = mhz(446);
const TEN_M: Freq10 = mhz(28) + 40_000;

fn req(tx: Freq10, rx: Freq10) -> Req {
    Req {
        tx,
        rx,
        kind: Kind::Voice,
        person: true,
        confirmed: false,
        am: false,
        rx_only: false,
        busy: false,
        hf_si: false,
        si_on: false,
        region: Region::Us,
        extend: false,
        cal_ok: true,
        cal_window: true,
        batt_ok: true,
        power: 2,
    }
}

/// A radio at its safe state, then receiving on `f` as the active slot.
fn receiving(f: Freq10) -> (Path, TxSeq, RecHw, State) {
    let (mut path, seq, mut hw) = (Path::new(), TxSeq::new(), RecHw::new());
    let mut s = Steps::default();
    path.safe(0, &mut s);
    path.rx(Rx::Bk(Sel::rx(f)), Cause::ActiveSlot, 0, &mut s).unwrap();
    block_on(run(&mut hw, &s));
    let st = hw.check(&State::default());
    hw.log.clear();
    (path, seq, hw, st)
}

fn go(hw: &mut RecHw, s: &Steps, st: &State) -> State {
    hw.log.clear();
    block_on(run(hw, s));
    hw.check(st)
}

fn pa_up(e: &Ev) -> bool {
    matches!(e, Ev::Pa(v) if *v > 0)
}

/// TX-23, TX-27, SAFE-11: a key-up carried out on the pins in the planned order: PA drive 0, the receive select
/// cleared, the TX select, the chip keyed (its read-back), then the PA ramped up to the calibrated level in equal
/// steps 1 ms apart; never two selects on, never the PA up without its chip keyed.
#[test]
fn key_up_order_on_the_pins() {
    let (mut path, mut seq, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    seq.key(&req(VHF, VHF), &mut path, 0, &mut s).unwrap();
    go(&mut hw, &s, &st);
    let zero = hw.at(|e| *e == Ev::Pa(0)).unwrap();
    let rx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfRx), false)).unwrap();
    let tx_on = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfTx), true)).unwrap();
    let key = hw.at(|e| *e == Ev::Key(0)).unwrap();
    let ramp = hw.at(pa_up).unwrap();
    assert!(zero < rx_off && rx_off < tx_on && tx_on < key && key < ramp, "{:?}", hw.evs());
    let steps: Vec<u8> = hw.evs().into_iter().filter_map(|e| match e { Ev::Pa(v) if v > 0 => Some(v), _ => None }).collect();
    assert_eq!(steps.len(), PA_RAMP_STEPS as usize);
    assert!(steps.windows(2).all(|w| w[0] < w[1]) && *steps.last().unwrap() == hw.pa_level, "{steps:?}");
    assert!(hw.at(|e| *e == Ev::Pin(Pin::Amp(0), false)).is_some(), "muted while keyed: the speaker off");
}

/// TX-26, TX-47, TONE-8, TX-28: the unkey on the pins in stock's order: end tone, CW ID, the audio stage (PB8) on
/// and tail elimination while still keyed, then PA drive 0, at least 100 ms, the chip out of transmit, then the
/// TX select clear and the receive path back (TX audit F4, F6).
#[test]
fn unkey_order_on_the_pins() {
    let (mut path, mut seq, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    seq.key(&req(VHF, VHF), &mut path, 0, &mut s).unwrap();
    let st = go(&mut hw, &s, &st);
    let cfg = Cfg { end_tone: true, cw_id: true, tail: true, ..Cfg::default() };
    let mut s = Steps::default();
    seq.release(&cfg, &mut path, 5_000, &mut s);
    go(&mut hw, &s, &st);
    let tone = hw.at(|e| *e == Ev::EndTone).unwrap();
    let cw = hw.at(|e| *e == Ev::CwId).unwrap();
    let tail = hw.at(|e| *e == Ev::Tail).unwrap();
    let zero = hw.at(|e| *e == Ev::Pa(0)).unwrap();
    let unkey = hw.at(|e| *e == Ev::Unkey(0)).unwrap();
    let tx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfTx), false)).unwrap();
    let rx_on = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfRx), true)).unwrap();
    let stage = hw.at(|e| *e == Ev::Pin(Pin::Amp(0), true)).unwrap();
    assert!(tone < cw && cw < stage && stage < tail && tail < zero && zero < unkey && unkey < tx_off && tx_off < rx_on, "{:?}", hw.evs());
    assert!(hw.time(unkey) - hw.time(zero) >= UNKEY_WAIT_MS as u32, "the unkey wait before the chip leaves transmit");
}

/// TX-7, TX-23 (the read-back before the PA ramp), SAFE-11: a key-up whose read-back fails stops the plan there: no
/// PA ramp, the fault reported for the caller's safe state.
#[test]
fn failed_read_back_never_ramps() {
    let (mut path, mut seq, mut hw, st) = receiving(UHF);
    hw.key_ok = false;
    let mut s = Steps::default();
    seq.key(&req(UHF, UHF), &mut path, 0, &mut s).unwrap();
    go(&mut hw, &s, &st);
    assert!(hw.at(|e| *e == Ev::Fault(1)).is_some(), "{:?}", hw.evs());
    assert!(hw.at(pa_up).is_none(), "no PA drive after a failed read-back: {:?}", hw.evs());
}

/// TX-33 (the Radio task's `Shutdown` while keyed, TX audit M6): the sequencer's safe state, run before any chip
/// sleeps, takes the PA drive to 0 first, with no end tone, CW ID or tail; then the chip out of transmit, the unkey
/// wait, every select off. The same from a crossband pass, its repeat audio off before the chip unkeys.
#[test]
fn power_off_while_keyed_zeroes_the_pa_first() {
    let (mut path, mut seq, mut hw, st) = receiving(UHF);
    let mut s = Steps::default();
    seq.key(&req(UHF, UHF), &mut path, 0, &mut s).unwrap();
    let st = go(&mut hw, &s, &st);
    let mut s = Steps::default();
    seq.safe(&mut path, 1_000, &mut s);
    let end = go(&mut hw, &s, &st);
    assert_eq!(hw.evs()[0], Ev::Pa(0), "PA drive 0 before anything else: {:?}", hw.evs());
    assert!(hw.at(|e| matches!(e, Ev::EndTone | Ev::CwId | Ev::Tail)).is_none(), "no tones at a power off");
    let unkey = hw.at(|e| *e == Ev::Unkey(1)).unwrap();
    let tx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::UhfTx), false)).unwrap();
    assert!(unkey < tx_off && hw.time(tx_off) >= UNKEY_WAIT_MS as u32, "{:?}", hw.evs());
    assert!(end.sel.iter().all(|&x| !x) && end.pa == 0 && end.keyed.is_none());
    assert!(!seq.keyed() && path.keyed().is_none());

    // A crossband pass: RX on UHF, TX on VHF.
    let (mut path, mut seq, mut hw, st) = receiving(UHF);
    let mut s = Steps::default();
    seq.key_xb(&Req { person: false, ..req(VHF, UHF) }, &mut path, 0, &mut s).unwrap();
    let st = go(&mut hw, &s, &st);
    let mut s = Steps::default();
    seq.safe(&mut path, 1_000, &mut s);
    let end = go(&mut hw, &s, &st);
    assert_eq!(hw.evs()[0], Ev::Pa(0), "{:?}", hw.evs());
    assert!(end.sel.iter().all(|&x| !x) && end.pa == 0 && end.keyed.is_none() && !end.repeat);
}

/// SAFE-20, SAFE-21, HF-11, TX-27: tuning the active slot from 2 m to 10 m moves the relay with no RF: muted,
/// every select clear, the PA at zero, then the relay, its settle time (10 ms), then the new select. Watch and scan
/// (`Cause::Auto`) never move it.
#[test]
fn relay_moves_only_quiet_and_settled() {
    let (mut path, _, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    assert!(path.rx(Rx::Bk(Sel::rx(TEN_M)), Cause::Auto, 0, &mut s).is_err());
    go(&mut hw, &s, &st);
    assert!(hw.at(|e| matches!(e, Ev::Pin(Pin::Relay, _))).is_none(), "Auto never moves the relay");
    let mut s = Steps::default();
    path.rx(Rx::Bk(Sel::rx(TEN_M)), Cause::ActiveSlot, 1_000, &mut s).unwrap();
    go(&mut hw, &s, &st);
    let mute = hw.at(|e| *e == Ev::Muted(true)).unwrap();
    let off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfRx), false)).unwrap();
    let relay = hw.at(|e| *e == Ev::Pin(Pin::Relay, true)).unwrap();
    let hf = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::HfRx), true)).unwrap();
    assert!(mute < relay && off < relay && relay < hf, "{:?}", hw.evs());
    assert!(hw.time(hf) - hw.time(relay) >= ferrite_core::path::RELAY_SETTLE_MS as u32, "the relay's settle time");
}

/// SAFE-22: the eleventh relay switch inside a minute is refused: the pins see ten moves, not eleven.
#[test]
fn relay_guard_on_the_pins() {
    let (mut path, _, mut hw, mut st) = receiving(VHF);
    let mut moves = 0;
    for i in 0..12u32 {
        let f = if i % 2 == 0 { TEN_M } else { VHF };
        let mut s = Steps::default();
        let _ = path.rx(Rx::Bk(Sel::rx(f)), Cause::ActiveSlot, 1_000 + i * 1_000, &mut s);
        st = go(&mut hw, &s, &st);
        moves += hw.evs().iter().filter(|e| matches!(e, Ev::Pin(Pin::Relay, _))).count();
    }
    assert_eq!(moves, ferrite_core::path::GUARD_SWITCHES, "the guard's limit reaches the pins");
    assert!(path.take_alarm(), "the event log's alarm");
}

/// TX-37, TX-28, WATCH-12 (TX audit F11): a key-up on a frequency behind the other relay state (a 10 m slot keyed
/// while 2 m is heard, or HF mode with the SI4732 on the antenna) is refused by the Radio task's own sequencer,
/// with no pin moved: the relay never switches at key-up.
#[test]
fn key_up_across_the_relay_is_refused() {
    let (mut path, mut seq, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    assert_eq!(seq.key(&req(TEN_M, TEN_M), &mut path, 0, &mut s), Err(ferrite_core::txplan::Refusal::RelayMismatch));
    go(&mut hw, &s, &st);
    assert!(hw.evs().is_empty(), "{:?}", hw.evs());
    assert!(!seq.keyed() && path.keyed().is_none() && !path.relay());
}

/// XB-6, XB-7, XB-8 (decision 95: stock's crossband audio switching): the other chip keys with the receiving
/// one's select left on; the PA ramps, then 15 ms later PA15 goes low (the receiving chip's audio into the mic
/// input) with PB8 on; at the receive drop PA15 is high again while still keyed; the unkey clears the receive
/// select while keyed, then PA 0, the unkey wait, the chip back, the TX select clear.
#[test]
fn crossband_audio_switching_as_stock() {
    let (mut path, mut seq, mut hw, st) = receiving(VHF);
    let mut s = Steps::default();
    seq.key_xb(&req(UHF, VHF), &mut path, 0, &mut s).unwrap();
    let st = go(&mut hw, &s, &st);
    assert!(st.sel[Sel::VhfRx as usize] && st.sel[Sel::UhfTx as usize], "receiving side stays selected");
    let last_ramp = hw.log.iter().rposition(|(_, e)| pa_up(e)).unwrap();
    let audio = hw.at(|e| *e == Ev::Pin(Pin::RepeatAudio, false)).unwrap();
    let pb8 = hw.at(|e| *e == Ev::Pin(Pin::Amp(0), true)).unwrap();
    assert!(last_ramp < audio && audio < pb8, "{:?}", hw.evs());
    assert!(hw.time(audio) - hw.time(last_ramp) >= XB_AUDIO_DELAY_MS as u32, "stock's 10-20 ms after the PA");
    // The receive drop: the mic again, still keyed.
    let mut s = Steps::default();
    path.xb_drop(&mut s);
    let st = go(&mut hw, &s, &st);
    assert_eq!(hw.evs().first(), Some(&Ev::Pin(Pin::RepeatAudio, true)));
    assert!(st.pa > 0 && st.keyed == Some(1), "still keyed through the hang");
    // The unkey, as stock's PTT state 4.
    let mut s = Steps::default();
    seq.release(&Cfg::default(), &mut path, 3_000, &mut s);
    let end = go(&mut hw, &s, &st);
    let rx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::VhfRx), false)).unwrap();
    let zero = hw.at(|e| *e == Ev::Pa(0)).unwrap();
    let unkey = hw.at(|e| *e == Ev::Unkey(1)).unwrap();
    let tx_off = hw.at(|e| *e == Ev::Pin(Pin::Sel(Sel::UhfTx), false)).unwrap();
    assert!(rx_off < zero && zero < unkey && unkey < tx_off, "{:?}", hw.evs());
    assert!(hw.time(unkey) - hw.time(zero) >= UNKEY_WAIT_MS as u32);
    assert!(end.keyed.is_none() && end.pa == 0 && !end.repeat);
}

/// BCR-20: the SI4732 input pins as stock: FM broadcast 0/0, AM and the rest 1/1; leaving the SI4732 sets 1/0,
/// but the boot and safe state before any broadcast leave stock's 0/0 alone.
#[test]
fn si4732_input_pins() {
    assert_eq!(chip::si_input(false), [false, false]);
    assert_eq!(chip::si_input(true), [true, true]);
    let mut hw = RecHw::new();
    let mut s = Steps::default();
    Path::new().safe(0, &mut s);
    block_on(run(&mut hw, &s));
    assert!(hw.at(|e| matches!(e, Ev::Pin(Pin::SiIn(_), _))).is_none(), "untouched before the SI4732 was used");
    hw.si_used = true;
    hw.log.clear();
    block_on(run(&mut hw, &s));
    let a = hw.at(|e| *e == Ev::Pin(Pin::SiIn(0), true)).unwrap();
    let b = hw.at(|e| *e == Ev::Pin(Pin::SiIn(1), false)).unwrap();
    assert!(a < b);
}

/// BCR-19, PWR-6 (decision 113): Battery Save's sleep step during broadcast FM, on the pins: nothing at all (the
/// receive select stays on: switching it clicked in the shared amp); the speaker amp, the receiver mute and the
/// SI4732's input pins are never touched, so the broadcast plays on. Without a broadcast only the BK4829's
/// receive select goes off, as stock.
#[test]
fn sleep_step_keeps_the_broadcast_playing() {
    let (mut path, _, mut hw, _) = receiving(VHF);
    hw.si_used = true; // broadcast FM running
    let mut s = Steps::default();
    path.rx_off(true, &mut s).unwrap();
    block_on(run(&mut hw, &s));
    assert_eq!(hw.evs(), []);
    let (mut path, _, mut hw, _) = receiving(VHF);
    let mut s = Steps::default();
    path.rx_off(false, &mut s).unwrap();
    block_on(run(&mut hw, &s));
    assert_eq!(hw.evs(), [Ev::Pin(Pin::Sel(Sel::VhfRx), false)]);
}

/// TX-27, TX-28, SAFE-21, SAFE-11: random sequences of everything that plans (tuning, watch, key-up and unkey,
/// the sequencer's tick, the safe state) carried out by `run` on the mock: the pin-level rules hold after every
/// plan (the same sequences `ferrite-core`'s rf_invariants checks at the plan level).
#[test]
fn random_plans_keep_the_pin_rules() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u32) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) % n as u64) as u32
        }
    }
    let freqs = [VHF, mhz(147), mhz(223) + 50_000, UHF, TEN_M, mhz(29) + 60_000, mhz(243)];
    let cfg = Cfg { end_tone: true, cw_id: true, tail: true, timeout_s: 30, ..Cfg::default() };
    let mut rng = Rng(950);
    let mut keyed = 0;
    for _ in 0..100 {
        let (mut path, mut seq, mut hw) = (Path::new(), TxSeq::new(), RecHw::new());
        let mut st = State::default();
        let mut now = 0u32;
        for _ in 0..200 {
            now += 1 + rng.next(5_000);
            let f = freqs[rng.next(freqs.len() as u32) as usize];
            let mut s = Steps::default();
            match rng.next(6) {
                0 | 1 => {
                    let cause = [Cause::ActiveSlot, Cause::Auto, Cause::HfReceiver][rng.next(3) as usize];
                    let _ = path.rx(Rx::Bk(Sel::rx(f)), cause, now, &mut s);
                }
                2 => {
                    let mut r = req(f, freqs[rng.next(freqs.len() as u32) as usize]);
                    r.power = rng.next(3) as u8;
                    if seq.key(&r, &mut path, now, &mut s).is_ok() {
                        keyed += 1;
                    }
                }
                3 => seq.release(&cfg, &mut path, now, &mut s),
                4 => {
                    let _ = seq.tick(&cfg, rng.next(20) != 0, &mut path, now, &mut s);
                }
                _ => {
                    if rng.next(8) == 0 {
                        seq.safe(&mut path, now, &mut s);
                    }
                }
            }
            hw.key_ok = rng.next(40) != 0;
            st = go(&mut hw, &s, &st);
            if hw.at(|e| matches!(e, Ev::Fault(_))).is_some() {
                // The Radio task goes to the safe state next.
                let mut s = Steps::default();
                seq.safe(&mut path, now, &mut s);
                st = go(&mut hw, &s, &st);
                st.keyed = None;
            }
        }
    }
    assert!(keyed > 300, "the sequences reach key-ups: {keyed}");
}

fn bk() -> Bk4829<RecBus> {
    Bk4829::new(RecBus::new())
}

/// VFO-20 (AM keeps the wide filter): tuning a slot in AM writes stock's mode 6
/// (REG_31 bit 0, REG_42 0x6F5C, REG_2A 0x7434, REG_2B 0x0500), FM stock's mode 7 (0x6B5A, 0x7400, 0x0000), the
/// wide filter for AM, the audio muted before receive comes on last.
#[test]
fn tune_writes_stocks_am_and_fm() {
    let mut b = bk();
    chip::tune(&mut b, &RxSetup { freq: mhz(118) + 95_000, narrow: false, am: true, rx_tone: 0, rx_tail: false });
    let bus = b.release();
    assert_eq!((bus.last(0x42), bus.last(0x2A), bus.last(reg::AF_FILTERS)), (Some(0x6F5C), Some(0x7434), Some(0x0500)));
    assert_eq!(bus.regs[reg::FUNC2 as usize] & 1, 1, "REG_31 bit 0");
    assert_eq!(bus.last(reg::RF_FILTER), Some(0x3028), "AM keeps the wide filter");
    assert_eq!(bus.writes.last(), Some(&(reg::POWER, 0xBFF1)), "receive on last");
    assert!(bus.writes.iter().rposition(|w| w.0 == reg::AF_OUT).unwrap() < bus.writes.len() - 2, "muted first");
    let mut b = bk();
    chip::tune(&mut b, &RxSetup { freq: VHF, narrow: true, am: false, rx_tone: 0, rx_tail: false });
    let bus = b.release();
    assert_eq!((bus.last(0x42), bus.last(0x2A), bus.last(reg::AF_FILTERS)), (Some(0x6B5A), Some(0x7400), Some(0)));
    assert_eq!(bus.regs[reg::FUNC2 as usize] & 1, 0);
    assert_eq!(bus.last(reg::RF_FILTER), Some(0x4008), "NFM narrow");
    assert_eq!(bus.last(reg::FREQ_LO).map(|lo| lo as u32 | (bus.regs[reg::FREQ_HI as usize] as u32) << 16), Some(VHF));
}

/// SND-2, RX audio level: every receive setup writes stock's REG_48 0xB2BF (Gain2 -4.5 dB, AF DAC 15), FM wide,
/// narrow and AM alike; the start-up 0xB3FF (+3.5 dB) distorted receive audio on the radio (Oct 12, 2026). The wide
/// filter is stock's 0x3028, narrow 0x4008, and the AF route is written whole (0x6042 muted).
#[test]
fn tune_writes_stocks_receive_level() {
    for (narrow, am, filter) in [(false, false, 0x3028), (true, false, 0x4008), (false, true, 0x3028)] {
        let mut b = bk();
        chip::tune(&mut b, &RxSetup { freq: if am { mhz(118) + 95_000 } else { VHF }, narrow, am, rx_tone: 0, rx_tail: false });
        let bus = b.release();
        assert_eq!(bus.last(reg::AF_GAIN), Some(bk4829::AF_GAIN_RX), "REG_48 (narrow {narrow}, am {am})");
        assert_eq!(bk4829::AF_GAIN_RX, 0xB2BF);
        assert_eq!(bus.last(reg::RF_FILTER), Some(filter));
        assert_eq!(bus.last(reg::AF_OUT), Some(0x6042), "muted, whole register");
    }
}

/// CHAN-3, TONE-6: the receive tone word picks the detector: 0 none (REG_51 0), CTCSS in 0.1 Hz, bit 15 DCS (the
/// 134.4 Hz word in REG_07), and no tone with tail elimination on listens for the 55 Hz tail (stock's REG_07 0x21CD).
#[test]
fn tune_sets_the_subtone_detector() {
    let tuned = |rx_tone: u16, rx_tail: bool| {
        let mut b = bk();
        chip::tune(&mut b, &RxSetup { freq: VHF, narrow: false, am: false, rx_tone, rx_tail });
        b.release()
    };
    assert_eq!(tuned(0, false).last(reg::SUBTONE), Some(0));
    let ctcss = tuned(885, false);
    assert_eq!(ctcss.last(reg::CTC), Some(bk4829::ctcss_word(885) & 0x1FFF));
    assert_eq!(ctcss.last(reg::SUBTONE).map(|v| v >> 12 & 1), Some(1), "CTCSS mode");
    let dcs = tuned(0x8000 | 0o23, false);
    assert_eq!(dcs.first(reg::CTC).map(|i| dcs.writes[i].1), Some(0x0AD7));
    assert!(dcs.first(reg::CDCSS).is_some());
    let tail = tuned(0, true);
    assert_eq!(tail.last(reg::CTC), Some(0x21CD), "stock's 55 Hz tail word");
}

/// TSRCH-12: the tone hint's probe switches the detectors to scan (REG_51 51, CTC1 the DCS bit rate) without
/// touching the frequency, filters or audio; putting the slot's tone back writes what tuning wrote (CTCSS, DCS,
/// none with REG_07 then unused, the 55 Hz tail).
#[test]
fn tone_probe_and_restore() {
    let mut b = bk();
    chip::tone_probe(&mut b);
    let bus = b.release();
    assert_eq!(bus.writes, [(reg::SUBTONE, 51), (reg::CTC, 0x0AD7)], "only the detector registers");
    for (rx_tone, rx_tail) in [(1000, false), (0x8000 | 0o23, false), (0x8000 | 0x4000 | 0o754, false), (0, false), (0, true)] {
        let mut t = bk();
        chip::tune(&mut t, &RxSetup { freq: VHF, narrow: false, am: false, rx_tone, rx_tail });
        let tuned = t.release();
        let mut r = bk();
        chip::tone_probe(&mut r);
        chip::subtone(&mut r, rx_tone, rx_tail);
        let restored = r.release();
        for reg_ in [reg::SUBTONE, reg::CTC, reg::CDCSS, reg::CTCSS_THRESH] {
            if reg_ == reg::CTC && rx_tone == 0 && !rx_tail {
                continue; // detector off (REG_51 0): REG_07 is unused
            }
            assert_eq!(restored.regs[reg_ as usize], tuned.regs[reg_ as usize], "REG_{reg_:02X} for {rx_tone:#x}");
        }
    }
}

/// TSRCH-13: what the detectors read becomes a tone word: CTCSS snapped to the standard tone within 1 Hz (REG_68's
/// conversion truncates, 88.5 can read 88.4), else kept to 0.1 Hz; out of 60-260 Hz ignored; DCS in any rotation,
/// with its polarity (the complement is inverted), and a DCS reading wins over a CTCSS one.
#[test]
fn heard_tone_decodes() {
    const CT: [u16; 4] = [670, 885, 1000, 1622];
    const DC: [u16; 3] = [0o023, 0o114, 0o754];
    assert_eq!(chip::heard_tone(Some(884), None, &CT, &DC), Some(885));
    assert_eq!(chip::heard_tone(Some(1003), None, &CT, &DC), Some(1000));
    assert_eq!(chip::heard_tone(Some(1300), None, &CT, &DC), Some(1300), "off the list: as read");
    assert_eq!(chip::heard_tone(Some(550), None, &CT, &DC), None, "the 55 Hz tail is no tone");
    assert_eq!(chip::heard_tone(None, None, &CT, &DC), None);
    let w = bk4829::dcs_codeword(0o114, false);
    assert_eq!(chip::heard_tone(None, Some(w), &CT, &DC), Some(0x8000 | 0o114));
    let rot = (w >> 5) | (w << 18) & 0x7F_FFFF;
    assert_eq!(chip::heard_tone(None, Some(rot), &CT, &DC), Some(0x8000 | 0o114), "rotated");
    let inv = bk4829::dcs_codeword(0o023, true);
    assert_eq!(chip::heard_tone(Some(885), Some(inv), &CT, &DC), Some(0x8000 | 0x4000 | 0o023), "D023I");
}

/// TSRCH-13: the DCS match reads the code out of each rotation instead of building every code's word; for every
/// 9-bit code, every rotation and both polarities it gives what the one-codeword-per-code search gave.
#[test]
fn heard_dcs_matches_the_full_search() {
    let all: Vec<u16> = (0..0o1000).collect();
    let reference = |w: u32| {
        for inverted in [false, true] {
            let mut x = if inverted { !w & 0x7F_FFFF } else { w & 0x7F_FFFF };
            for _ in 0..23 {
                if let Some(&c) = all.iter().find(|&&c| bk4829::dcs_codeword(c, false) == x) {
                    return Some(0x8000 | if inverted { 0x4000 } else { 0 } | c);
                }
                x = (x >> 1) | (x & 1) << 22;
            }
        }
        None
    };
    for &c in &all {
        for inv in [false, true] {
            let mut w = bk4829::dcs_codeword(c, inv);
            for _ in 0..23 {
                assert_eq!(chip::heard_tone(None, Some(w), &[], &all), reference(w), "code {c:o} inv {inv} word {w:06x}");
                w = (w >> 1) | (w & 1) << 22;
            }
        }
    }
}

/// SND-11: a local beep on a chip: REG_30 to stock's beep value 0x0302 (AF and the tone DSP only: no PLL, no PA,
/// no mic), the tone generator at stock's beep gain; a sleeping chip is woken first and put back to sleep after;
/// a receiving one goes back to receive with its REG_48.
#[test]
fn beep_never_keys_and_restores() {
    let mut b = Bk4829::new({
        let mut r = RecBus::new();
        r.regs[reg::AF_GAIN as usize] = 0x1234;
        r
    });
    let gain = chip::beep_start(&mut b, 1000, 0x17, true);
    assert_eq!(gain, 0x1234);
    chip::beep_end(&mut b, gain, true);
    let bus = b.release();
    assert_eq!(bus.writes.first().map(|w| w.0), Some(reg::LDO), "woken first");
    let p = bus.writes.iter().find(|w| w.0 == reg::POWER).unwrap().1;
    assert_eq!(p, bk4829::POWER_BEEP);
    assert_eq!(p & 0x80F8 & !0x0300, 0, "no PLL, PA or mic bits in 0x{p:04X}");
    assert_eq!(bus.last(reg::AF_GAIN), Some(0x1234), "REG_48 back");
    assert_eq!(bus.writes.last().map(|w| w.0), Some(reg::LDO), "asleep again last");
    let mut b = bk();
    let g = chip::beep_start(&mut b, 1000, 0x17, false);
    chip::beep_end(&mut b, g, false);
    assert_eq!(b.release().writes.last(), Some(&(reg::POWER, 0xBFF1)), "receive again");
}

/// SQL-15, SQL-16: the squelch thresholds go to REG_78 and REG_4F as computed (stock's 0x0801E64C), and nothing else.
#[test]
fn squelch_writes_stocks_registers() {
    let mut b = bk();
    chip::squelch(&mut b, (0x5955, 0x2827));
    assert_eq!(b.release().writes, [(reg::SQ_RSSI, 0x5955), (reg::SQ_NOISE, 0x2827)]);
}

/// TSRCH-10: the counter's chips sit mid-band, chip 0 below 300 MHz and chip 1 above, each in its own front end,
/// and use stock's frequency scan (REG_32 0x8245 on, 0x8244 off; the reading from REG_0D/0E in 10 Hz units).
#[test]
fn counter_uses_stocks_frequency_scan() {
    assert_eq!(Sel::rx(chip::COUNTER_TUNE[0]).chip(), 0);
    assert_eq!(Sel::rx(chip::COUNTER_TUNE[1]).chip(), 1);
    assert!(!Sel::rx(chip::COUNTER_TUNE[0]).relay() && !Sel::rx(chip::COUNTER_TUNE[1]).relay(), "never the relay");
    let mut b = bk();
    b.freq_scan(true);
    b.freq_scan(false);
    let mut bus = b.release();
    assert_eq!(bus.writes, [(reg::FREQ_SCAN, 0x8245), (reg::FREQ_SCAN, 0x8244)]);
    bus.regs[reg::SCAN_HI as usize] = 0x0001;
    bus.regs[reg::SCAN_LO as usize] = 0x6550;
    let mut b = Bk4829::new(bus);
    assert_eq!(b.freq_scan_result(), Some(((1 << 16) | 0x6550) * 10));
}

/// DEC-19: SAME on the chip's own modem: REG_58 0x03C4 (receive mode 000, bandwidth 010, gain 3), the
/// 520.83 baud rate word, the sync bytes `ZCZC`, set up on a chip tuned to the weather channels (162.400-162.550).
#[test]
fn same_modem_setup() {
    let mut b = bk();
    chip::same_arm(&mut b);
    let bus = b.release();
    assert_eq!(bus.first(reg::FSK_MODE).map(|i| bus.writes[i].1), Some(0x03C4 | 1), "0x03C4 with the modem enable");
    assert_eq!((chip::SAME_FSK.tone_word, bus.last(reg::TONE2_FREQ)), (5377, Some(5377)));
    let sync = [bus.last(reg::FSK_SYNC01), bus.last(reg::FSK_SYNC23)];
    assert_eq!(sync, [Some(0x5AC2), Some(0x5AC2)], "ZCZC as the chip wants it");
    assert!(chip::WX.contains(&(mhz(162) + 55_000)) && !chip::WX.contains(&(mhz(162) + 56_000)));
}

/// TX-7, TX-23 (the chip keyed with its read-back before the PA): key with the microphone (REG_30 0xC1FE), read
/// back REG_30, the frequency (REG_38/39) and REG_3B, then stock's REG_36 (0x7F High, 0x57 otherwise) and its
/// read-back; all match: keyed.
#[test]
fn key_reads_back_before_the_pa() {
    for (high, r36) in [(true, 0x7F), (false, 0x57)] {
        let (mut b, want) = keyed_ready();
        assert_eq!(chip::key_mic(&mut b, high, &want), Ok(()));
        let bus = b.release();
        assert_eq!(bus.last(reg::POWER), Some(chip::REG30_TX_MIC));
        assert_eq!(bus.last(reg::PA), Some(r36));
        assert!(bus.first(reg::PA).unwrap() > bus.writes.iter().rposition(|w| w.0 == reg::POWER).unwrap());
    }
}

fn setup(freq: Freq10, narrow: bool, tone: u16) -> chip::TxSetup {
    chip::TxSetup { freq, narrow, tone, tone_dev: 0x46, mic: 0x24 }
}

/// TX-45 (TX audit M7): the TX set-up writes stock's REG_43 with every TX frequency (0x3028 wide, 0x4008 narrow), over
/// whatever the chip held (here the reset value 0x4048, as a cross-band split's TX chip may), out of receive
/// first (REG_30 0) and the sub-tone after.
#[test]
fn tx_setup_writes_the_bandwidth() {
    for (narrow, r43) in [(false, 0x3028), (true, 0x4008)] {
        let mut b = bk();
        b.write(reg::RF_FILTER, 0x4048);
        chip::tx_setup(&mut b, &setup(VHF, narrow, 885));
        let bus = b.release();
        assert_eq!(bus.last(reg::RF_FILTER), Some(r43));
        assert_eq!(bus.regs[reg::FREQ_LO as usize] as u32 | (bus.regs[reg::FREQ_HI as usize] as u32) << 16, VHF);
        assert!(bus.first(reg::POWER).unwrap() < bus.writes.iter().rposition(|w| w.0 == reg::RF_FILTER).unwrap(), "out of receive first");
        assert_eq!(bus.regs[reg::SUBTONE as usize] & 0x8000, 0x8000, "CTCSS sent");
    }
    let mut b = bk();
    chip::tx_setup(&mut b, &setup(UHF, false, 0));
    assert_eq!(b.release().last(reg::RF_FILTER), Some(0x3028));
}

/// VOX-6, VOX-15, TONE-1, TX audit F1, F2, F7, F14: the key-up writes stock's registers from the start-up values:
/// REG_7D 0xE912 to stock's default 0xE924 (bit 5 is the 6-bit field's top bit; Mic gain 5), REG_40 0x34D2 to
/// stock's 0x350A, REG_47 0x6142 (stock's TX route), REG_51 with the calibration's CTCSS byte (0x9046 on the black
/// radio) or its DCS byte in REG_51's low 7 bits, and REG_30 0 before any of them.
#[test]
fn tx_setup_writes_stocks_registers() {
    let mut b = bk();
    b.write(reg::MIC, 0xE912);
    b.write(reg::DEVIATION, 0x34D2);
    chip::tx_setup(&mut b, &setup(VHF, false, 885));
    let bus = b.release();
    assert_eq!(bus.last(reg::MIC), Some(0xE924));
    assert_eq!(bus.last(reg::DEVIATION), Some(0x350A));
    assert_eq!(bus.last(reg::AF_OUT), Some(0x6142));
    assert_eq!(bus.last(reg::SUBTONE), Some(0x9046));
    assert_eq!(bus.first(reg::POWER), bus.writes.iter().position(|w| w.0 == reg::POWER));
    let p0 = bus.writes.iter().position(|w| *w == (reg::POWER, 0)).unwrap();
    assert!(p0 < bus.first(reg::AF_OUT).unwrap() && p0 < bus.first(reg::FREQ_LO).unwrap(), "REG_30 0 first, as stock");
    let mut b = bk();
    b.write(reg::MIC, 0xE912);
    chip::tx_setup(&mut b, &chip::TxSetup { tone: 0x8000 | 0o23, tone_dev: 0x3E, mic: ferrite_core::vox::MIC_DATA, ..setup(UHF, false, 0) });
    let bus = b.release();
    assert_eq!(bus.last(reg::SUBTONE).map(|v| v & 0x7F), Some(0x3E), "the DCS byte");
    assert_eq!(bus.last(reg::MIC), Some(0xE934), "a data key-up: V0.29's 0x34");
}

/// A chip tuned to 146.520 MHz with a REG_3B of its own, and what TX-7 expects of it.
fn keyed_ready() -> (Bk4829<RecBus>, chip::TxExpect) {
    let f = mhz(146) + 52_000;
    let mut b = bk();
    b.set_freq(f * 10);
    b.write(0x3B, 0x1234);
    (b, chip::TxExpect { freq: f, reg3b: 0x1234, narrow: false })
}

/// TX-7: any one register wrong refuses the key-up, naming it, before REG_36's level is left standing: REG_30 not
/// keyed, the synthesized frequency not the one written, REG_3B changed since start-up, REG_36 not taking. The
/// caller then goes to the safe state with the PA at zero (`failed_read_back_never_ramps`).
#[test]
fn any_wrong_read_back_refuses() {
    use chip::ReadBack;
    let stuck = |r: u8, v: u16| {
        let (b, want) = keyed_ready();
        let mut bus = b.release();
        bus.stuck = Some((r, v));
        (Bk4829::new(bus), want)
    };
    let (mut b, want) = stuck(reg::POWER, 0x0200);
    assert_eq!(chip::key_mic(&mut b, true, &want), Err(ReadBack::Power));
    assert!(b.release().last(reg::PA).is_none(), "no REG_36 after a failed read-back");
    let (mut b, want) = stuck(reg::FREQ_LO, 0x0001);
    assert_eq!(chip::key_mic(&mut b, true, &want), Err(ReadBack::Freq));
    assert!(b.release().last(reg::PA).is_none());
    let (mut b, mut want) = keyed_ready();
    want.freq += 1; // the word in the chip isn't the one asked for
    assert_eq!(chip::key_mic(&mut b, true, &want), Err(ReadBack::Freq));
    let (mut b, want) = stuck(0x3B, 0x1235);
    assert_eq!(chip::key_mic(&mut b, true, &want), Err(ReadBack::Xtal));
    assert!(b.release().last(reg::PA).is_none());
    let (mut b, want) = stuck(reg::PA, 0x0000);
    assert_eq!(chip::key_mic(&mut b, false, &want), Err(ReadBack::Bias));
}

/// TONE-13, TONE-19, DTMF-7, DTMF-8, DTMF-11: while keyed a tone burst or a DTMF symbol (A-D too) switches REG_30 to tone
/// transmit (stock's mode 4, the microphone off) with the sidetone as asked; letting go returns to the microphone
/// with its read-back.
#[test]
fn keyed_tones_switch_the_source() {
    let mut b = bk();
    assert_eq!(chip::keyed_tone(&mut b, Some(1750), None, true, false), (true, true));
    let bus = b.release();
    let tone = bus.last(reg::POWER).unwrap();
    assert_ne!(tone, chip::REG30_TX_MIC, "tone transmit, not the microphone");
    assert_eq!(bus.last(reg::TONE1_FREQ), Some(bk4829::tone_word(1750)));
    // TX audit M8: the burst (and the end tone and CW ID, through the same tone_tx) at V0.29's gain 0x17, REG_70
    // 0x9700, not V0.27's 0xBC00; DTMF pairs keep 0xC5C5 (both versions).
    assert_eq!(bus.last(reg::TONE), Some(0x9700));
    for d in ['5', 'A', 'D', '#'] {
        let mut b = bk();
        assert_eq!(chip::keyed_tone(&mut b, None, Some(d), false, false), (true, true), "{d}");
        let bus = b.release();
        assert_eq!(bus.last(reg::POWER), Some(tone));
        assert_eq!(bus.last(reg::TONE), Some(0xC5C5));
        let pair = bk4829::tx::dtmf_pair(d).unwrap();
        assert_eq!((bus.last(reg::TONE1_FREQ), bus.last(reg::TONE2_FREQ)), (Some(bk4829::tone_word(pair.0)), Some(bk4829::tone_word(pair.1))));
    }
    let mut b = bk();
    assert_eq!(chip::keyed_tone(&mut b, None, None, false, true), (false, true));
    let bus = b.release();
    assert_eq!((bus.last(reg::TONE), bus.last(reg::POWER)), (Some(0), Some(chip::REG30_TX_MIC)));
    assert_eq!(bus.last(reg::AF_OUT), Some(0x6142), "back to the key-up's AF route (F14)");
    let n = bus.writes.len();
    assert_eq!(bus.writes[n - 2..], [(reg::POWER, 0), (reg::POWER, chip::REG30_TX_MIC)], "narrow: through 0, as stock (F3)");
}

/// TONE-6, TONE-8 (tail elimination at the unkey): no tone sends the 55 Hz tail, DCS the 134.4 Hz turn-off code
/// (REG_52 phase 0), CTCSS the chosen phase shift; on the chip, REG_52 bit 15 with the phase in bits 14:13.
#[test]
fn tail_follows_the_tone() {
    use bk4829::tx::Tail;
    // TONE-5, TX audit F7: the 55 Hz tone at the calibration's CTCSS byte, as stock (the black radio's 0x46).
    assert_eq!(chip::tail(0, 2, 0x46), Tail::Tone55 { dev: 0x46 });
    assert_eq!(chip::tail(0x8000 | 0o23, 2, 0x46), Tail::Shift { phase: 0 });
    assert_eq!(chip::tail(885, 2, 0x46), Tail::Shift { phase: 2 });
    assert_eq!(chip::tail(885, 0, 0x46), Tail::Tone55 { dev: 0x46 });
    let mut b = bk();
    b.tail_tx(Some(chip::tail(885, 3, 0x46)));
    let r52 = b.release().last(reg::CTCSS_THRESH).unwrap();
    assert_eq!((r52 >> 15, r52 >> 13 & 3), (1, 3));
}

/// GPS-5, GPS-22: PB10 driven low while any chip transmits and handed back to the UART after unkey, once each.
#[test]
fn gps_pb10_while_keyed() {
    let mut g = GpsPower::default();
    assert_eq!(g.step(0, true, true, false).pb10_low, None);
    assert_eq!(g.step(10, true, true, true).pb10_low, Some(true));
    assert_eq!(g.step(20, true, true, true).pb10_low, None);
    assert_eq!(g.step(30, true, true, false).pb10_low, Some(false));
}

/// GPS-1, GPS-4, GPS-25: powered with 0 6 1; off at once for a signal 0 6 8 covers, and back on only after
/// `QUIET_MS` (2 s) without one, so a burst of short signals doesn't cycle it; 0 6 1 off keeps it off.
#[test]
fn gps_power_around_signals() {
    let mut g = GpsPower::default();
    assert_eq!(g.step(5_000, true, true, false).power, Some(true));
    assert_eq!(g.step(5_100, true, false, false).power, Some(false));
    assert_eq!(g.step(5_200, true, true, false).power, None, "not straight back");
    assert_eq!(g.step(5_100 + QUIET_MS - 1, true, true, false).power, None);
    assert_eq!(g.step(5_100 + QUIET_MS, true, true, false).power, Some(true));
    assert_eq!(g.step(9_000, false, true, false).power, Some(false));
    assert_eq!(g.step(20_000, false, true, false).power, None);
}

/// LINK-14, LINK-25 (decision 76): PB9 follows 0 9 2 1, and while Bluetooth is on it is set again after each
/// transmission, as stock; never while it is off.
#[test]
fn bluetooth_pb9() {
    let mut b = BtPower::default();
    assert_eq!(b.step(false, false).power, None, "off by default");
    assert_eq!(b.step(true, false).power, Some(true));
    assert!(!b.step(true, true).reassert);
    assert!(b.step(true, false).reassert, "after the unkey");
    assert!(!b.step(true, false).reassert, "once");
    assert_eq!(b.step(false, true).power, Some(false));
    assert!(!b.step(false, false).reassert, "never while off");
}

/// SAFE-1, CAL-1, CAL-2, INST-7, INST-8, SAFE-2, UPD-7: the SPI driver's guard refuses any program or erase that
/// touches calibration (0x00F000-0x00FFFF) or the update area (0x300000 up), checks the erase unit (a whole 4 KB
/// sector, aligned) rather than the range asked for, and keeps programs inside one page; everything between the
/// layout header and the waypoints is writable; the bring-up build writes nowhere.
#[test]
fn spi_flash_guard() {
    use crate::flash::{erase_ok, program_ok, PAGE, SECTOR};
    use ferrite_core::layout::{CALIBRATION, UPDATE_AREA, WRITABLE};
    assert_eq!((PAGE as usize, SECTOR as usize), (ferrite_store::PAGE as usize, ferrite_store::SECTOR as usize));
    let ok = &WRITABLE[..];
    for a in [CALIBRATION.start, CALIBRATION.start + 0x800, UPDATE_AREA.start, UPDATE_AREA.end - SECTOR] {
        assert!(!program_ok(ok, a, 16), "program at 0x{a:06X}");
        assert!(!erase_ok(ok, a & !(SECTOR - 1)), "erase at 0x{a:06X}");
    }
    assert!(!erase_ok(ok, CALIBRATION.start - SECTOR), "the sector below calibration isn't Ferrite's either");
    assert!(!erase_ok(ok, 0x02_D800), "unaligned: refused, not rounded");
    assert!(!program_ok(ok, 0x02_D0F8, 16), "across a page boundary");
    assert!(!program_ok(ok, 0x02_D000, 0) && !program_ok(ok, 0x02_D000, PAGE + 1));
    assert!(!program_ok(ok, u32::MAX - 4, 8), "no wrap-around");
    for r in ok {
        assert!(erase_ok(ok, r.start) && erase_ok(ok, r.end - SECTOR) && program_ok(ok, r.start, PAGE));
        assert!(!erase_ok(ok, r.end) || ok.iter().any(|o| o.start == r.end));
    }
    assert!(!program_ok(&[], 0x02_D000, 16) && !erase_ok(&[], 0x02_D000), "bring-up: nowhere");
}

/// SAFE-14, BOOT-3, KEY-8, TX-19, TX-42 (the released-once part): a PTT held at power-on keeps transmit shut until
/// every PTT input has read released once; after that it stays open for the power-on.
#[test]
fn ptt_held_at_power_on() {
    let mut g = crate::ptt::PttGuard::default();
    assert!(!g.seen(false) && !g.seen(false), "held since boot");
    assert!(g.seen(true), "released once");
    assert!(g.seen(false), "pressed again later: that's a real PTT");
}

/// PWR-5: Battery Save's sleep and wake on a chip, as stock: sleep writes REG_30 = 0 then REG_37 = 0x1D00; wake
/// REG_37 = 0x9D1F, then the receive sequence (`chip::tune` wakes first and turns receive on last).
#[test]
fn sleep_and_wake_as_stock() {
    let mut b = bk();
    b.sleep();
    let bus = b.release();
    assert_eq!(bus.writes, [(reg::POWER, 0), (reg::LDO, 0x1D00)]);
    let mut b = bk();
    chip::tune(&mut b, &RxSetup { freq: VHF, narrow: false, am: false, rx_tone: 0, rx_tail: false });
    let bus = b.release();
    assert_eq!(bus.writes.first(), Some(&(reg::LDO, 0x9D1F)));
    assert_eq!(bus.writes.last(), Some(&(reg::POWER, 0xBFF1)));
}

/// A BK4829 bus and a delay writing to one log, so a wait's place among the register writes shows: a wait is
/// logged as (`WAIT`, ms).
#[derive(Clone, Default)]
struct Log(std::rc::Rc<core::cell::RefCell<Vec<(u8, u16)>>>);

const WAIT: u8 = 0xFF;

impl bk4829::Bus for Log {
    fn write(&mut self, reg: u8, value: u16) {
        self.0.borrow_mut().push((reg, value));
    }
    fn read(&mut self, _reg: u8) -> u16 {
        0
    }
    fn delay_us(&mut self, us: u32) {
        self.0.borrow_mut().push((WAIT_US, us as u16));
    }
}

/// The driver's busy waits (us), as (`WAIT_US`, us).
const WAIT_US: u8 = 0xFE;

impl chip::Delay for Log {
    async fn ms(&mut self, ms: u32) {
        self.0.borrow_mut().push((WAIT, ms as u16));
    }
}

impl Log {
    fn take(&self) -> Vec<(u8, u16)> {
        core::mem::take(&mut *self.0.borrow_mut())
    }
}

const RX: RxSetup = RxSetup { freq: VHF, narrow: false, am: false, rx_tone: 0, rx_tail: false };

/// PWR-21 (decision 119): from sleep, REG_37's supplies on, the wake settle, then the receive set-up with the AF
/// route muted before anything else and receive on (REG_30) last; the wake time is kept.
#[test]
fn wake_settles_before_the_receive_setup() {
    use ferrite_core::settle::Settle;
    let log = Log::default();
    let (mut bk, mut d) = (Bk4829::new(log.clone()), log.clone());
    let at = block_on(chip::tune_settled(&mut bk, &mut d, &RX, Settle { wake_ms: 2, sleep_ms: 1 }, None, 100));
    assert_eq!(at, 100);
    let w = log.take();
    assert_eq!(w[..3], [(reg::LDO, 0x9D1F), (WAIT, 2), (reg::AF_OUT, 0x6042)], "{w:x?}");
    assert_eq!(w.iter().position(|e| e.0 == reg::POWER), Some(w.len() - 3), "REG_30 only at the end");
    // Stock's 50 us between the two REG_30 writes (decision 119, the TX audit).
    assert_eq!(w[w.len() - 3..], [(reg::POWER, 0x0200), (WAIT_US, 50), (reg::POWER, 0xBFF1)]);
    assert_eq!(w.iter().filter(|e| e.0 == WAIT).count(), 1);
}

/// PWR-21: a chip whose supplies came on a tick early (the hop's `due`), or that is long awake, is tuned with no
/// wait and no second REG_37 write; one woken 1 ms ago waits the rest; 0 is stock's: no wait at all.
#[test]
fn wake_settle_counts_from_the_early_wake() {
    use ferrite_core::settle::Settle;
    let s = Settle { wake_ms: 2, sleep_ms: 1 };
    let log = Log::default();
    let (mut bk, mut d) = (Bk4829::new(log.clone()), log.clone());
    assert_eq!(block_on(chip::tune_settled(&mut bk, &mut d, &RX, s, Some(90), 100)), 90);
    let w = log.take();
    assert!(!w.iter().any(|e| e.0 == WAIT || e.0 == reg::LDO), "{w:x?}");
    block_on(chip::tune_settled(&mut bk, &mut d, &RX, s, Some(99), 100));
    assert_eq!(log.take().iter().filter(|e| e.0 == WAIT).collect::<Vec<_>>(), [&(WAIT, 1)]);
    block_on(chip::tune_settled(&mut bk, &mut d, &RX, Settle::STOCK, None, 100));
    let w = log.take();
    assert_eq!(w[0], (reg::LDO, 0x9D1F));
    assert!(!w.iter().any(|e| e.0 == WAIT), "stock's: back to back");
}

/// PWR-21: to sleep, the AF route muted, REG_30 off, the sleep settle, then REG_37's supplies off; with 0 the same
/// writes with no wait (stock's REG_30 0 then REG_37 0x1D00, after the mute).
#[test]
fn sleep_mutes_then_settles() {
    use ferrite_core::settle::Settle;
    let log = Log::default();
    let (mut bk, mut d) = (Bk4829::new(log.clone()), log.clone());
    block_on(chip::sleep_settled(&mut bk, &mut d, Settle { wake_ms: 2, sleep_ms: 1 }));
    assert_eq!(log.take(), [(reg::AF_OUT, 0x6042), (reg::POWER, 0), (WAIT, 1), (reg::LDO, 0x1D00)]);
    block_on(chip::sleep_settled(&mut bk, &mut d, Settle::STOCK));
    assert_eq!(log.take(), [(reg::AF_OUT, 0x6042), (reg::POWER, 0), (reg::LDO, 0x1D00)]);
}

/// PWR-21: a receiving chip retuned (scan, a new frequency) is muted before its frequency changes.
#[test]
fn retune_mutes_first() {
    let log = Log::default();
    let mut bk = Bk4829::new(log.clone());
    chip::tune_rest(&mut bk, &RX);
    assert_eq!(log.take()[0], (reg::AF_OUT, 0x6042));
}
