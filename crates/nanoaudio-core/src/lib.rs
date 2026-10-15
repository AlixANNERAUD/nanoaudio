#![no_std]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

/// A bit reader for the codecs that parse bit streams.
pub mod bits;
/// The [`Decoder`](codec::Decoder) and [`Encoder`](codec::Encoder) traits every codec implements, and seeking.
pub mod codec;
/// CRC-8 and CRC-16 checksums for the codecs that verify their frames.
pub mod crc;
mod error;
mod format;
mod sample;

pub use error::*;
pub use format::*;
pub use sample::*;
