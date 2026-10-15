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

    fn flush(&mut self) {}

    fn encode(&mut self, input: &[Self::Sample], output: &mut [u8]) -> Result<Progress>;
}

/// Where to resume feeding input after a seek.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SeekRequest {
    /// Byte offset to resume from, counted from the first byte the decoder was ever fed: the start of the sound data for WAV and AIFF, the start of the file for QOA.
    pub byte_offset: u64,
    /// Frames the decoder drops by itself after resuming, so the next frame produced is the requested one; reported for information only.
    pub skip_frames: u32,
}

/// Decoders that can jump to an arbitrary frame.
pub trait Seek {
    /// Prepares the decoder to produce `frame`, counted per channel from the start of the audio, and returns where the caller must resume feeding input.
    ///
    /// Decoders that cannot compute the position directly may then return [`Error::SeekTo`](crate::Error::SeekTo) from `decode` until they reach it.
    fn seek(&mut self, frame: u64) -> Result<SeekRequest>;
}
