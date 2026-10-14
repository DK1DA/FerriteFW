//! Input sampler: the knob every millisecond, the keys every 10 ms (debounced; press and release). It
//! runs on its own interrupt executor, above the Radio task's, so neither a slow redraw nor a busy scan makes it
//! miss a knob step or a key;
//! the UI task owns what the keys mean.
//!
//! With the keys, the side connector's speaker-mic detect (PC15, SND-12): stock's boot check, then its 10 ms poll
//! and four-reading debounce (`ferrite_core::acc`). The state goes to [`ACCESSORY`]; each change to the Radio task,
//! which owns the audio switches (PE1, PB8).

use crate::board::{ACC_DET, ENC_A, ENC_B, KEY_DIRECT0, KEY_DIRECT1, KEY_DRIVE, KEY_PTT, KEY_SENSE, KEY_SIDE1, KEY_SIDE2};
use crate::bus::{BusyDelay, In, Out};
use crate::chan::{Input, ACCESSORY, INPUT, RADIO};
use crate::hal::gpio::Mode;
use core::sync::atomic::Ordering::Relaxed;
use embassy_time::{Duration, Ticker};
use ferrite_core::acc::AccDet;
use ferrite_core::msg::RadioCmd;
use ferrite_core::keys::{CODE_PTT_A, CODE_PTT_B, CODE_PTT_VFO, DIRECT, GROUND_ROW, ROWS};
use keypad::{Encoder, Keys, Matrix};

/// Quadrature steps per click: 2 on the RT-950 Pro's channel knob (20 clicks a turn; with 4, only every other
/// click counted on the radio, Oct 12, 2026).
const PER_DETENT: i8 = 2;

#[embassy_executor::task]
pub async fn task() {
    for p in KEY_DRIVE {
        p.output(true, Mode::OutOd); // open drain: two keys in one column cannot short two rows
    }
    for p in KEY_SENSE.into_iter().chain([KEY_DIRECT0, KEY_DIRECT1, KEY_PTT, KEY_SIDE1, KEY_SIDE2, ENC_A, ENC_B]) {
        p.config(Mode::InPu);
    }
    let mut matrix = Matrix::new(
        KEY_DRIVE.map(Out),
        KEY_SENSE.map(In),
        [In(KEY_DIRECT0), In(KEY_DIRECT1)],
        Some(GROUND_ROW),
        ROWS,
        DIRECT,
        20,
    );
    let ptt = [(KEY_PTT, CODE_PTT_A), (KEY_SIDE1, CODE_PTT_B), (KEY_SIDE2, CODE_PTT_VFO)];
    // 20 ms debounce; press and release only: long press and repeat are the UI's (ferrite-app's KeyTracker,
    // KEY-3, KEY-4), which also handles keys held together.
    let mut keys = Keys::new(2, 0, 0);
    let mut enc = Encoder::new(PER_DETENT);
    enc.update(ENC_A.read(), ENC_B.read());
    // SND-12: PC15 (board::init made it an input with its pull-up) with the keys: stock's boot check, then its
    // debounce. What the Radio task has been told: it starts with PE1 low, nothing connected; a full queue is
    // tried again on the next poll.
    let mut acc = AccDet::new();
    let mut told = false;
    let mut ticker = Ticker::every(Duration::from_millis(1));
    let mut tick = 0u32;
    loop {
        let d = enc.update(ENC_A.read(), ENC_B.read());
        if d != 0 {
            let _ = INPUT.try_send(Input::Knob(d));
        }
        if tick.is_multiple_of(10) {
            let mut raw = matrix.scan(&mut BusyDelay);
            for (p, code) in ptt {
                if !p.read() {
                    raw |= 1 << code;
                }
            }
            keys.tick(raw, |e| {
                let _ = INPUT.try_send(Input::Key(e));
            });
            acc_poll(&mut acc, &mut told);
        }
        tick = tick.wrapping_add(1);
        ticker.next().await;
    }
}

/// SND-12: one reading of PC15 into the debounce, the state to [`ACCESSORY`], a change to the Radio task.
#[inline(never)]
fn acc_poll(acc: &mut AccDet, told: &mut bool) {
    acc.poll(!ACC_DET.read());
    ACCESSORY.store(acc.pack(), Relaxed);
    if acc.on() != *told && RADIO.try_send(RadioCmd::Accessory(acc.on())).is_ok() {
        *told = acc.on();
    }
}
