mod g711;
mod ima_adpcm;
#[cfg(feature = "mp3")]
mod mp3;
mod qoa;
mod wav;

pub use g711::*;
pub use ima_adpcm::*;
#[cfg(feature = "mp3")]
pub use mp3::*;
pub use qoa::*;
pub use wav::*;

use crate::{Configuration, Result, Sample};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Progress {
    pub consumed: usize,
    pub produced: usize,
}

impl Progress {
    pub const fn new(consumed: usize, produced: usize) -> Self {
        Self { consumed, produced }
    }

    pub const fn is_stalled(&self) -> bool {
        self.consumed == 0 && self.produced == 0
    }
}

pub trait Decoder {
    type Sample: Sample;

    fn configuration(&self) -> Option<Configuration>;

    fn minimum_output(&self) -> usize;

    fn minimum_input(&self) -> usize {
        1
    }

    fn finish(&mut self) {}

    fn decode(&mut self, input: &[u8], output: &mut [Self::Sample]) -> Result<Progress>;
}

pub trait Encoder {
    type Sample: Sample;

    fn configuration(&self) -> Configuration;

    fn minimum_input(&self) -> usize;

    fn minimum_output(&self) -> usize;

    fn finish(&mut self) {}

    fn encode(&mut self, input: &[Self::Sample], output: &mut [u8]) -> Result<Progress>;
}
