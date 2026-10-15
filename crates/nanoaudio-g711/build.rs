use std::env;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(nanoaudio_no_clz)");
    println!("cargo::rerun-if-changed=build.rs");

    let target = env::var("TARGET").unwrap_or_default();
    let architecture = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    let features: Vec<&str> = features.split(',').collect();

    let thumb_only = features.contains(&"mclass") && !features.contains(&"thumb2");

    if architecture == "arm" && (target.starts_with("thumbv8m.base") || thumb_only) {
        println!("cargo::rustc-cfg=nanoaudio_no_clz");
    }
}
