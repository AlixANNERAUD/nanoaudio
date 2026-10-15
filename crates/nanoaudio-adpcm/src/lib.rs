#![no_std]
#![doc = include_str!("../README.md")]

mod ima_adpcm;
mod ms_adpcm;

pub use ima_adpcm::*;
pub use ms_adpcm::*;
