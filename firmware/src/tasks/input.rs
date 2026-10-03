//! Input sampler: the knob every millisecond, the keys every 10 ms (debounced; press and release). It
//! runs on the DSP's interrupt executor so a slow redraw on the thread executor never makes it miss a knob step;
//! the UI task owns what the keys mean.

use crate::board::{ENC_A, ENC_B, KEY_DIRECT0, KEY_DIRECT1, KEY_DRIVE, KEY_PTT, KEY_SENSE, KEY_SIDE1, KEY_SIDE2};
use crate::bus::{BusyDelay, In, Out};
use crate::chan::{Input, INPUT};
use crate::hal::gpio::Mode;
use embassy_time::{Duration, Ticker};
use ferrite_core::keys::{CODE_PTT_A, CODE_PTT_B, CODE_PTT_VFO, DIRECT, GROUND_ROW, ROWS};
use keypad::{Encoder, Keys, Matrix};

/// Quadrature steps per click: to confirm with hwtest's knob figure.
const PER_DETENT: i8 = 4;

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
        }
        tick = tick.wrapping_add(1);
        ticker.next().await;
    }
}
