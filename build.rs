use std::env;

fn riscv_has_float(target: &str) -> bool {
    let architecture = target.split('-').next().unwrap_or_default();
    let extensions = architecture
        .trim_start_matches("riscv32")
        .trim_start_matches("riscv64");

    extensions.contains('g') || extensions.contains('f')
}

fn has_float(target: &str, architecture: &str, abi: &str, features: &str) -> bool {
    match architecture {
        "x86" | "x86_64" | "aarch64" | "wasm32" | "wasm64" => true,
        "arm" => abi == "eabihf",
        "riscv32" | "riscv64" => {
            riscv_has_float(target) || features.split(',').any(|feature| feature == "f")
        }
        "xtensa" => target.starts_with("xtensa-esp32-") || target.starts_with("xtensa-esp32s3-"),
        _ => false,
    }
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(nanoaudio_float)");
    println!("cargo::rerun-if-changed=build.rs");

    let target = env::var("TARGET").unwrap_or_default();
    let architecture = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let abi = env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();

    let float = if env::var_os("CARGO_FEATURE_FLOAT").is_some() {
        true
    } else if env::var_os("CARGO_FEATURE_FIXED").is_some() {
        false
    } else {
        has_float(&target, &architecture, &abi, &features)
    };

    if float {
        println!("cargo::rustc-cfg=nanoaudio_float");
    }
}
