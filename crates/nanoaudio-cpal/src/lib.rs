#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

#[cfg(target_endian = "big")]
compile_error!("nanoaudio-cpal supports little-endian targets only");

mod callback;
mod convert;
mod error;
mod negotiate;
mod resample;
mod stream;

pub use cpal;
pub use error::*;
pub use negotiate::DeviceConfiguration;
pub use resample::{MAXIMUM_RATIO, Quality};
pub use stream::*;
