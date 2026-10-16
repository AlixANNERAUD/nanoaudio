#![no_std]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod envelope;
mod filter;
mod oscillator;
mod representation;
mod synth;
mod voice;

pub use envelope::*;
pub use filter::*;
pub use oscillator::{Oscillator, Waveform, frequency_increment, note_increment};
pub use representation::Synthesis;
pub use synth::*;
pub use voice::*;
