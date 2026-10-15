#![no_std]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod counter;

pub use counter::*;
pub use nanoaudio_dsp::{
    Analyzer, Tap, analyze, analyze_bytes, analyze_bytes_using, analyze_using,
};
