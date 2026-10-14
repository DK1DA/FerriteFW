//! The RT-950 Pro's matrix (stock's code table), debouncing, events and the encoder.
#![allow(clippy::needless_range_loop)]

use std::cell::RefCell;
use std::rc::Rc;
use keypad::{Encoder, Event, Keys, Matrix, NONE};
use uradio_bus::mock::MockDelay;
use uradio_bus::{InPin, OutPin};

/// A switch matrix: which (row, column) keys are down; row 4 is the ground row.
#[derive(Default)]
struct Board {
    down: Vec<(usize, usize)>,
    driven: Option<usize>,
    direct: Vec<bool>,
}
type B = Rc<RefCell<Board>>;
struct Row(B, usize);
struct Col(B, usize);
struct Direct(B, usize);
impl OutPin for Row {
    fn set(&mut self, high: bool) {
        let mut b = self.0.borrow_mut();
        if high {
            if b.driven == Some(self.1) {
                b.driven = None;
            }
        } else {
            assert!(b.driven.is_none(), "two rows driven at once");
            b.driven = Some(self.1);
        }
    }
}
impl InPin for Col {
    fn is_high(&mut self) -> bool {
        let b = self.0.borrow();
        !b.down.iter().any(|&(r, c)| c == self.1 && (r == 4 || Some(r) == b.driven))
    }
}
impl InPin for Direct {
    fn is_high(&mut self) -> bool {
        !self.0.borrow().direct[self.1]
    }
}

// Stock's table (analysis/keypad.md): ground row, then PC0..PC3 low; direct keys PE5 -> 0, PA12 -> 1.
const GROUND: [u8; 4] = [10, 14, 18, 13];
const CODES: [[u8; 4]; 4] = [[6, 3, 9, 2], [12, 16, 20, 21], [11, 15, 19, 17], [7, 8, 4, 5]];

fn matrix(b: &B) -> Matrix<Row, Col, Direct, 4, 4, 2> {
    Matrix::new(
        core::array::from_fn(|r| Row(b.clone(), r)),
        core::array::from_fn(|c| Col(b.clone(), c)),
        core::array::from_fn(|d| Direct(b.clone(), d)),
        Some(GROUND),
        CODES,
        [0, 1],
        20,
    )
}

#[test]
fn every_key_scans_to_its_code() {
    let b: B = Rc::new(RefCell::new(Board { direct: vec![false, false], ..Default::default() }));
    let mut m = matrix(&b);
    let mut d = MockDelay::default();
    assert_eq!(m.scan(&mut d), 0);
    for r in 0..5 {
        for c in 0..4 {
            b.borrow_mut().down = vec![(r, c)];
            let want = if r == 4 { GROUND[c] } else { CODES[r][c] };
            assert_eq!(m.scan(&mut d), 1 << want, "row {r} col {c}");
        }
    }
    // A ground-row key masks its column: the other keys in that column do not ghost.
    b.borrow_mut().down = vec![(4, 2), (1, 0)];
    assert_eq!(m.scan(&mut d), 1 << GROUND[2] | 1 << CODES[1][0]);
    b.borrow_mut().down = vec![];
    b.borrow_mut().direct = vec![true, true];
    assert_eq!(m.scan(&mut d), 0b11);
    // 23 scans so far, 4 driven rows each, 20 us settle per row.
    assert_eq!(d.total_us, 23 * 4 * 20);
    assert!(b.borrow().driven.is_none(), "rows released after the scan");
    let _ = NONE;
}

#[test]
fn debounce_long_press_and_repeat() {
    let mut k = Keys::new(2, 50, 10);
    let mut ev = Vec::new();
    // A one-tick glitch: ignored.
    k.tick(1 << 5, |e| ev.push(e));
    k.tick(0, |e| ev.push(e));
    assert!(ev.is_empty());
    // Press: reported on the second agreeing scan.
    k.tick(1 << 5, |e| ev.push(e));
    k.tick(1 << 5, |e| ev.push(e));
    assert_eq!(ev, vec![Event::Press(5)]);
    ev.clear();
    for _ in 0..80 {
        k.tick(1 << 5, |e| ev.push(e));
    }
    // Long at 50 ticks held, then repeats every 10.
    assert_eq!(ev, vec![Event::Long(5), Event::Repeat(5), Event::Repeat(5), Event::Repeat(5)]);
    ev.clear();
    k.tick(0, |e| ev.push(e));
    k.tick(0, |e| ev.push(e));
    assert_eq!(ev, vec![Event::Release(5)]);
    // Two keys together: presses, but no long press for a chord.
    ev.clear();
    for _ in 0..100 {
        k.tick(1 << 1 | 1 << 2, |e| ev.push(e));
    }
    assert_eq!(ev, vec![Event::Press(1), Event::Press(2)]);
}

#[test]
fn encoder_detents_and_glitches() {
    let mut e = Encoder::new(4);
    // One clockwise detent: 00 -> 01 -> 11 -> 10 -> 00.
    let cw = [(false, true), (true, true), (true, false), (false, false)];
    let steps: Vec<i8> = cw.iter().map(|&(a, b)| e.update(a, b)).collect();
    assert_eq!(steps.iter().sum::<i8>().abs(), 1);
    let dir = steps.iter().sum::<i8>();
    // Back the other way: the opposite sign.
    let back: i8 = cw.iter().rev().skip(1).chain([(false, false)].iter()).map(|&(a, b)| e.update(a, b)).sum();
    assert_eq!(back, -dir);
    // Contact bounce (a step forward and back) adds nothing.
    let mut e = Encoder::new(4);
    let s: i8 = [(false, true), (false, false), (false, true), (false, false)].iter().map(|&(a, b)| e.update(a, b)).sum();
    assert_eq!(s, 0);
}
