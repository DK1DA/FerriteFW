//! The link protocol and the terminal over the simulator's Bluetooth (roadmap 3d): what a phone app on the
//! BT513's serial characteristic sees, beside the cable.

use ferrite_core::link::{self as wire, msg, MAX_WIRE, PROTOCOL};
use ferrite_sim::simcore::SimCore;

fn run(sim: &mut SimCore, t: &mut u32, ms: u32) {
    let end = *t + ms;
    while *t < end {
        *t += 20;
        sim.step(*t);
    }
}

/// A radio switched on, with Bluetooth turned on from the cable's terminal (0 9 2 1) and the terminal closed.
fn radio() -> (SimCore, u32) {
    let mut sim = SimCore::new(1, false);
    let mut t = 0;
    run(&mut sim, &mut t, 6000);
    sim.term_feed(b"\r:set 0 9 2 1 On\r");
    run(&mut sim, &mut t, 200);
    sim.term_feed(&[0x03]); // Ctrl-C: the cable's terminal closes
    run(&mut sim, &mut t, 200);
    sim.term_take();
    (sim, t)
}

fn ask_bt(sim: &mut SimCore, t: &mut u32, ty: u8, seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut w = [0u8; MAX_WIRE];
    let n = wire::build(ty, seq, payload, &mut w).unwrap();
    let mut bytes = vec![0u8];
    bytes.extend_from_slice(&w[..n]);
    sim.bt_feed(&bytes);
    let mut out = Vec::new();
    for _ in 0..50 {
        *t += 20;
        sim.step(*t);
        out.extend(sim.bt_take());
        if out.contains(&0) {
            break;
        }
    }
    let end = out.iter().position(|&b| b == 0).expect("an answer on Bluetooth");
    let f = wire::parse(&mut out[..end]).expect("a good frame");
    assert_eq!((f.ty, f.seq), (ty | wire::ANSWER, seq));
    f.payload.to_vec()
}

/// HELLO over Bluetooth answers on Bluetooth, not the cable, and says the device may not write yet (LINK-7);
/// `BAUD` can't move Bluetooth off 115200 (LINK-25).
#[test]
fn link_over_bluetooth() {
    let (mut sim, mut t) = radio();
    let a = ask_bt(&mut sim, &mut t, msg::HELLO, 1, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[..2], &[0, PROTOCOL]);
    assert_eq!(a[3], 0, "not approved: it reads, and asks before writing");
    assert!(sim.term_take().is_empty(), "nothing on the cable");
    let b = ask_bt(&mut sim, &mut t, msg::BAUD, 2, &921_600u32.to_le_bytes());
    assert_ne!(b[0], 0, "refused");
}

/// The terminal over Bluetooth: Enter opens it there; the cable taking it back closes it on Bluetooth.
#[test]
fn terminal_over_bluetooth() {
    let (mut sim, mut t) = radio();
    sim.bt_feed(b"\r");
    run(&mut sim, &mut t, 200);
    let page = sim.bt_take();
    assert!(page.windows(7).any(|w| w == b"Ferrite"), "the splash on Bluetooth");
    assert!(sim.term_take().is_empty());
    sim.term_feed(b"\r");
    run(&mut sim, &mut t, 200);
    assert!(!sim.term_take().is_empty(), "the cable wins");
    sim.bt_feed(b"x");
    run(&mut sim, &mut t, 200);
    assert!(sim.bt_take().is_empty(), "Bluetooth's bytes dropped while the cable has the terminal");
}
