#![cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), no_std)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod convert;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod web;

pub use convert::*;
pub use nanoaudio_buffer::{Consumer, Producer};
pub use nanoaudio_core::{Configuration, Format};
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use web::*;
