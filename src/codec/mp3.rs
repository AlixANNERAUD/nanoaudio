use core::marker::PhantomData;

use nanomp3_core::MAX_SAMPLES_PER_FRAME;
pub use nanomp3_core::Sample as Mp3Sample;

use crate::{Configuration, Error, Result, Sample};

use super::{Decoder, Progress};

pub const MP3_DEFAULT_MINIMUM_INPUT: usize = 16 * 1024;

#[derive(Clone)]
pub struct Mp3Decoder<S: Sample + nanomp3_core::Sample = i16> {
    decoder: nanomp3_core::Decoder,
    configuration: Option<Configuration>,
    minimum_input: usize,
    finishing: bool,
    sample: PhantomData<S>,
}

impl<S: Sample + nanomp3_core::Sample> Mp3Decoder<S> {
    pub const fn new() -> Self {
        Self::with_minimum_input(MP3_DEFAULT_MINIMUM_INPUT)
    }

    pub const fn with_minimum_input(minimum_input: usize) -> Self {
        Self {
            decoder: nanomp3_core::Decoder::new(),
            configuration: None,
            minimum_input,
            finishing: false,
            sample: PhantomData,
        }
    }

    pub fn reset(&mut self) {
        self.decoder = nanomp3_core::Decoder::new();
        self.configuration = None;
        self.finishing = false;
    }
}

impl<S: Sample + nanomp3_core::Sample> Default for Mp3Decoder<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Sample + nanomp3_core::Sample> Decoder for Mp3Decoder<S> {
    type Sample = S;

    fn configuration(&self) -> Option<Configuration> {
        self.configuration
    }

    fn minimum_input(&self) -> usize {
        self.minimum_input
    }

    fn minimum_output(&self) -> usize {
        MAX_SAMPLES_PER_FRAME
    }

    fn finish(&mut self) {
        self.finishing = true;
    }

    fn decode(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        if output.len() < MAX_SAMPLES_PER_FRAME {
            return Err(Error::BufferTooSmall);
        }

        if input.is_empty() || (!self.finishing && input.len() < self.minimum_input) {
            return Ok(Progress::default());
        }

        let (consumed, result) = self.decoder.decode(input, output);

        let produced = match result {
            Ok(information) => {
                self.configuration = Some(Configuration::new(
                    information.sample_rate,
                    information.channels.num(),
                    S::FORMAT,
                ));

                information.samples_produced * information.channels.num() as usize
            }
            Err(_) => 0,
        };

        Ok(Progress::new(consumed, produced))
    }
}
