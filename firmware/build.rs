use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for f in ["memory.x", "device.x"] {
        fs::copy(f, out.join(f)).unwrap();
        println!("cargo:rerun-if-changed={f}");
    }
    println!("cargo:rustc-link-search={}", out.display());
}
