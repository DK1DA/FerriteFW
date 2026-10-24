// Selects the Cortex-M4 DSP-instruction kernels (`cfg(udsp_dsp)`) when building for an ARMv7E-M target. Stable Rust
// doesn't expose `target_feature = "dsp"` on ARM, so the target name decides: every thumbv7em core has the DSP
// extension. The `plain` feature turns the selection off (to compare the two on the target).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(udsp_dsp)");
    println!("cargo::rerun-if-env-changed=TARGET");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("thumbv7em") && std::env::var_os("CARGO_FEATURE_PLAIN").is_none() {
        println!("cargo::rustc-cfg=udsp_dsp");
    }
}
