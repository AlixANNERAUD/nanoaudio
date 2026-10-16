use std::env;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(nanoaudio_no_division)");
    println!("cargo::rerun-if-changed=build.rs");

    let target = env::var("TARGET").unwrap_or_default();
    let architecture = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let atomics = env::var("CARGO_CFG_TARGET_HAS_ATOMIC").unwrap_or_default();

    let baseline = target.starts_with("thumbv6m");
    let no_atomics = !atomics.split(',').any(|width| width == "32");

    if architecture == "arm" && (baseline || no_atomics) {
        println!("cargo::rustc-cfg=nanoaudio_no_division");
    }
}
