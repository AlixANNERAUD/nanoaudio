use std::env;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(nanoaudio_synth_split)");
    println!("cargo::rerun-if-changed=build.rs");

    let target = env::var("TARGET").unwrap_or_default();

    if ["thumbv6m-", "thumbv8m.base-"]
        .iter()
        .any(|prefix| target.starts_with(prefix))
    {
        println!("cargo::rustc-cfg=nanoaudio_synth_split");
    }
}
