//! The link protocol over the simulator's cable, as a PC tool sees it (manual chapter 29, TERM-2).

use ferrite_core::link::{self as wire, msg, what, MAX_WIRE, PROTOCOL};
use ferrite_sim::simcore::SimCore;

fn ask(sim: &mut SimCore, t: &mut u32, ty: u8, seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut w = [0u8; MAX_WIRE];
    let n = wire::build(ty, seq, payload, &mut w).unwrap();
    let mut bytes = vec![0u8];
    bytes.extend_from_slice(&w[..n]);
    sim.term_feed(&bytes);
    let mut out = Vec::new();
    for _ in 0..50 {
        *t += 20;
        sim.step(*t);
        out.extend(sim.term_take());
        if out.contains(&0) {
            break;
        }
    }
    let end = out.iter().position(|&b| b == 0).expect("an answer");
    let f = wire::parse(&mut out[..end]).expect("a good frame");
    assert_eq!((f.ty, f.seq), (ty | wire::ANSWER, seq));
    f.payload.to_vec()
}

/// HELLO, a channel read and written back over the cable; then the terminal still opens on Enter once the link
/// has been quiet (TERM-2).
#[test]
fn link_over_the_cable() {
    let mut sim = SimCore::new(1, false);
    let mut t = 0;
    while t < 6000 {
        t += 20;
        sim.step(t);
    }
    let a = ask(&mut sim, &mut t, msg::HELLO, 1, &[PROTOCOL, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&a[..2], &[0, PROTOCOL]);
    let r = ask(&mut sim, &mut t, msg::READ, 2, &[what::CHANNEL, 0, 0, 0, 0, 200]);
    assert_eq!(r[0], 0, "channel 1 exists in the sample");
    let total = u16::from_le_bytes([r[1], r[2]]) as usize;
    let mut rec = r[3..3 + total].to_vec();
    rec[7] = b'Q';
    let mut p = vec![what::CHANNEL, 0, 0, 0, 0];
    p.extend_from_slice(&(total as u16).to_le_bytes());
    p.extend_from_slice(&rec);
    assert_eq!(ask(&mut sim, &mut t, msg::WRITE, 3, &p)[0], 0);
    let r = ask(&mut sim, &mut t, msg::READ, 4, &[what::CHANNEL, 0, 0, 0, 0, 200]);
    assert_eq!(r[3 + 7], b'Q');
    // quiet past the link's hold: Enter is the terminal's again
    t += 11_000;
    sim.step(t);
    sim.term_feed(b"\r");
    t += 100;
    sim.step(t);
    assert!(!sim.term_take().is_empty(), "the terminal answered");
}
