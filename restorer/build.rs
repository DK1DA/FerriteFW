use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    // No PROVIDEs: src/main.rs supplies all 96 peripheral vectors itself (as the firmware's device.x).
    fs::write(out.join("device.x"), "/* src/main.rs supplies the peripheral vectors */\n").unwrap();
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rustc-link-search={}", out.display());
}
