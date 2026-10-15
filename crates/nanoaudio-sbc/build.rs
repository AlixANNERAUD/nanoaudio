use std::env;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(nanoaudio_no_long_multiply)");
    println!("cargo::rerun-if-changed=build.rs");

    let target = env::var("TARGET").unwrap_or_default();
    let architecture = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    let features: Vec<&str> = features.split(',').collect();
    let atomics = env::var("CARGO_CFG_TARGET_HAS_ATOMIC").unwrap_or_default();

    let thumb_only = features.contains(&"mclass") && !features.contains(&"thumb2");
    let baseline = target.starts_with("thumbv6m") || target.starts_with("thumbv8m.base");
    let no_atomics = !atomics.split(',').any(|width| width == "32");

    if architecture == "arm" && (baseline || thumb_only || no_atomics) {
        println!("cargo::rustc-cfg=nanoaudio_no_long_multiply");
    }
}
