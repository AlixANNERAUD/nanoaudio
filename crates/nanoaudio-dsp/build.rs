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

fn has_split_gain(target: &str) -> bool {
    [
        "thumbv6m-",
        "thumbv8m.base-",
        "thumbv7m-",
        "thumbv7em-",
        "riscv32",
    ]
    .iter()
    .any(|prefix| target.starts_with(prefix))
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(nanoaudio_float)");
    println!("cargo::rustc-check-cfg=cfg(nanoaudio_split_gain)");
    println!("cargo::rerun-if-changed=build.rs");

    let target = env::var("TARGET").unwrap_or_default();
    let architecture = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let abi = env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();

    println!("cargo::rustc-check-cfg=cfg(nanoaudio_hard_float)");

    let hard_float = has_float(&target, &architecture, &abi, &features);
    let float = if env::var_os("CARGO_FEATURE_FIXED").is_some() {
        false
    } else if env::var_os("CARGO_FEATURE_FLOAT").is_some() {
        true
    } else {
        hard_float
    };

    if hard_float {
        println!("cargo::rustc-cfg=nanoaudio_hard_float");
    }

    if float {
        println!("cargo::rustc-cfg=nanoaudio_float");
    }

    if has_split_gain(&target) {
        println!("cargo::rustc-cfg=nanoaudio_split_gain");
    }
}
