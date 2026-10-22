//! Records the simulator for a video (`tools/promo/mkvideo.py`): one sample radio, switched on, then for each
//! segment a setup script (played, not recorded) and a script whose every frame (20 ms, 50 per second) is written
//! as raw 240 x 320 RGB to `OUT_DIR/NN.rgb`, and each script step with the frame it starts on to `OUT_DIR/NN.keys`
//! ("FRAME<TAB>STEP" lines: the video draws the keys pressed). The radio carries on from one segment to the next.
//!
//!     ferrite-sim-record OUT_DIR SETUP1 SCRIPT1 [SETUP2 SCRIPT2 ...]
//!
//! Each SETUP and SCRIPT is a file in the harness's script language (`sim/tests/common/mod.rs`).

#[path = "../../tests/common/mod.rs"]
mod common;

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 3 && args.len() % 2 == 1, "usage: ferrite-sim-record OUT_DIR SETUP SCRIPT [SETUP SCRIPT ...]");
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).unwrap();
    let mut r = common::Radio::new();
    for (k, pair) in args[1..].chunks(2).enumerate() {
        let setup = std::fs::read_to_string(&pair[0]).unwrap();
        let script = std::fs::read_to_string(&pair[1]).unwrap();
        r.rec = None;
        if !setup.trim().is_empty() {
            r.run(&setup);
        }
        let file = std::fs::File::create(out.join(format!("{k:02}.rgb"))).unwrap();
        let w = Rc::new(RefCell::new(std::io::BufWriter::new(file)));
        let frames = Rc::new(RefCell::new(0u32));
        let (w2, f2) = (w.clone(), frames.clone());
        r.rec = Some(Box::new(move |sim| {
            let mut px = Vec::with_capacity(240 * 320 * 3);
            for p in sim.screen_rgb() {
                px.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
            w2.borrow_mut().write_all(&px).unwrap();
            *f2.borrow_mut() += 1;
        }));
        // Step by step, so each step's first frame is known.
        let mut keys = std::io::BufWriter::new(std::fs::File::create(out.join(format!("{k:02}.keys"))).unwrap());
        for step in script.split([';', '\n']).map(str::trim).filter(|s| !s.is_empty()) {
            writeln!(keys, "{}\t{step}", frames.borrow()).unwrap();
            r.run(step);
        }
        keys.flush().unwrap();
        r.rec = None;
        w.borrow_mut().flush().unwrap();
        println!("{k:02}: {} frames", frames.borrow());
    }
}
