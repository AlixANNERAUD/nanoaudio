use nanoaudio_core::codec::{Decoder, Encoder, Progress, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Format, Result, Sample};

use crate::{discard, seek_block};

const INDEX_ADJUSTMENTS: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

const MAXIMUM_INDEX: u8 = STEPS.len() as u8 - 1;
const CHANNEL_HEADER_SIZE: usize = 4;
const CHANNEL_GROUP_SIZE: usize = 4;
const SAMPLES_PER_GROUP: usize = CHANNEL_GROUP_SIZE * 2;

/// The predictor and step index of one IMA ADPCM channel.
///
/// Use it to code samples one at a time, outside the WAV block layout of [`ImaAdpcm`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ImaAdpcmState {
    predictor: i16,
    index: u8,
}

impl ImaAdpcmState {
    /// Creates a state from a predicted sample and a step index, clamping the index to 88.
    pub const fn new(predictor: i16, index: u8) -> Self {
        Self {
            predictor,
            index: if index > MAXIMUM_INDEX {
                MAXIMUM_INDEX
            } else {
                index
            },
        }
    }

    /// Returns the last decoded sample, which predicts the next one.
    pub const fn predictor(&self) -> i16 {
        self.predictor
    }

    /// Returns the step index, from 0 to 88.
    pub const fn index(&self) -> u8 {
        self.index
    }

    /// Decodes the 4-bit code in the low nibble of `nibble` and returns the sample.
    pub fn decode(&mut self, nibble: u8) -> i16 {
        let step = STEPS[self.index as usize];
        let mut difference = step >> 3;

        if nibble & 4 != 0 {
            difference += step;
        }
        if nibble & 2 != 0 {
            difference += step >> 1;
        }
        if nibble & 1 != 0 {
            difference += step >> 2;
        }

        let predictor = if nibble & 8 != 0 {
            self.predictor as i32 - difference
        } else {
            self.predictor as i32 + difference
        };

        self.predictor = predictor.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        self.index = (self.index as i8 + INDEX_ADJUSTMENTS[(nibble & 0x0F) as usize])
            .clamp(0, MAXIMUM_INDEX as i8) as u8;

        self.predictor
    }

    /// Encodes `sample` to a 4-bit code, updating the state as the decoder will.
    pub fn encode(&mut self, sample: i16) -> u8 {
        let mut step = STEPS[self.index as usize];
        let mut difference = sample as i32 - self.predictor as i32;
        let mut nibble = 0;

        if difference < 0 {
            nibble = 8;
            difference = -difference;
        }

        for bit in [4, 2, 1] {
            if difference >= step {
                nibble |= bit;
                difference -= step;
            }
            step >>= 1;
        }

        self.decode(nibble);

        nibble
    }
}

/// An IMA ADPCM encoder and decoder with the WAV block layout.
///
/// Each block starts with a 4-byte header per channel, holding the first sample and the step index, followed by 4 bits per sample in groups of 4 bytes per channel. Up to 8 channels at any sample rate; the blocks store neither, so they come from the container, like the block size.
///
/// `MAXIMUM_CHANNELS` sizes the per-channel state, 4 bytes per channel. [`new`](Self::new) keeps room for 8 channels; [`new_limited`](Self::new_limited) on `ImaAdpcm<1>` or `ImaAdpcm<2>` keeps a mono or stereo codec smaller.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImaAdpcm<const MAXIMUM_CHANNELS: usize = { nanoaudio_core::MAXIMUM_CHANNELS }> {
    sample_rate: u32,
    channels: u8,
    block_align: u16,
    states: [ImaAdpcmState; MAXIMUM_CHANNELS],
    finishing: bool,
    skip: usize,
}

impl ImaAdpcm {
    /// Creates a codec from `block_align`, the size of one block in bytes as found in the WAV header.
    ///
    /// Returns [`Error::TooManyChannels`] for 0 or more than 8 channels, and [`Error::InvalidParameter`] if `sample_rate` is zero or `block_align` is not one header per channel plus whole 4-byte groups per channel.
    pub fn new(sample_rate: u32, channels: u8, block_align: u16) -> Result<Self> {
        Self::new_limited(sample_rate, channels, block_align)
    }

    /// Creates a codec from the number of samples per channel in one block, which must be 1 plus a multiple of 8.
    ///
    /// Returns [`Error::InvalidParameter`] for any other count, and the same errors as [`new`](Self::new).
    pub fn with_samples_per_block(
        sample_rate: u32,
        channels: u8,
        samples_per_block: u16,
    ) -> Result<Self> {
        Self::with_samples_per_block_limited(sample_rate, channels, samples_per_block)
    }
}

impl<const MAXIMUM_CHANNELS: usize> ImaAdpcm<MAXIMUM_CHANNELS> {
    /// Like [`new`](ImaAdpcm::new), with room for `MAXIMUM_CHANNELS` channels.
    ///
    /// Returns [`Error::TooManyChannels`] for 0 or more than `MAXIMUM_CHANNELS` channels. Fails to compile unless `MAXIMUM_CHANNELS` is between 1 and 8.
    pub fn new_limited(sample_rate: u32, channels: u8, block_align: u16) -> Result<Self> {
        const {
            assert!(
                MAXIMUM_CHANNELS >= 1 && MAXIMUM_CHANNELS <= nanoaudio_core::MAXIMUM_CHANNELS,
                "the maximum channel count must be between 1 and 8"
            );
        }

        let header = CHANNEL_HEADER_SIZE * channels as usize;
        let group = CHANNEL_GROUP_SIZE * channels as usize;
        let block = block_align as usize;

        if channels == 0 || channels as usize > MAXIMUM_CHANNELS {
            return Err(Error::TooManyChannels);
        }

        if sample_rate == 0 || block <= header || !(block - header).is_multiple_of(group) {
            return Err(Error::InvalidParameter);
        }

        Ok(Self {
            sample_rate,
            channels,
            block_align,
            states: [ImaAdpcmState::default(); MAXIMUM_CHANNELS],
            finishing: false,
            skip: 0,
        })
    }

    /// Like [`with_samples_per_block`](ImaAdpcm::with_samples_per_block), with room for `MAXIMUM_CHANNELS` channels.
    pub fn with_samples_per_block_limited(
        sample_rate: u32,
        channels: u8,
        samples_per_block: u16,
    ) -> Result<Self> {
        let groups = (samples_per_block as usize)
            .checked_sub(1)
            .filter(|samples| samples % SAMPLES_PER_GROUP == 0)
            .ok_or(Error::InvalidParameter)?
            / SAMPLES_PER_GROUP;
        let block_align = (CHANNEL_HEADER_SIZE + groups * CHANNEL_GROUP_SIZE) * channels as usize;

        Self::new_limited(
            sample_rate,
            channels,
            u16::try_from(block_align).map_err(|_| Error::InvalidParameter)?,
        )
    }

    /// Returns the size of one block in bytes.
    pub const fn block_align(&self) -> u16 {
        self.block_align
    }

    /// Returns the number of samples per channel in one block.
    pub const fn samples_per_block(&self) -> usize {
        self.groups() * SAMPLES_PER_GROUP + 1
    }

    const fn groups(&self) -> usize {
        let channels = self.channels as usize;

        (self.block_align as usize - CHANNEL_HEADER_SIZE * channels)
            / (CHANNEL_GROUP_SIZE * channels)
    }

    fn block_samples(&self) -> usize {
        self.samples_per_block() * self.channels as usize
    }

    const fn partial_block_samples(&self, length: usize) -> usize {
        let channels = self.channels as usize;

        match length.checked_sub(CHANNEL_HEADER_SIZE * channels) {
            Some(data) => {
                (data / (CHANNEL_GROUP_SIZE * channels) * SAMPLES_PER_GROUP + 1) * channels
            }
            None => 0,
        }
    }

    fn pcm_configuration(&self) -> Configuration {
        Configuration::new(self.sample_rate, self.channels, Format::I16)
    }

    fn decode_block<S: Sample>(&mut self, block: &[u8], output: &mut [S]) {
        let channels = self.channels as usize;
        let (headers, data) = block.split_at(CHANNEL_HEADER_SIZE * channels);

        for (channel, header) in headers
            .as_chunks::<CHANNEL_HEADER_SIZE>()
            .0
            .iter()
            .enumerate()
        {
            let predictor = i16::from_le_bytes([header[0], header[1]]);

            self.states[channel] = ImaAdpcmState::new(predictor, header[2]);
            output[channel] = predictor.convert();
        }

        for (group, bytes) in data.chunks_exact(CHANNEL_GROUP_SIZE * channels).enumerate() {
            for (channel, bytes) in bytes.as_chunks::<CHANNEL_GROUP_SIZE>().0.iter().enumerate() {
                let state = &mut self.states[channel];
                let first = 1 + group * SAMPLES_PER_GROUP;

                for (offset, byte) in bytes.iter().enumerate() {
                    let sample = first + offset * 2;

                    output[sample * channels + channel] = state.decode(byte & 0x0F).convert();
                    output[(sample + 1) * channels + channel] = state.decode(byte >> 4).convert();
                }
            }
        }
    }

    fn encode_block(&mut self, input: &[i16], block: &mut [u8]) {
        let channels = self.channels as usize;
        let (headers, data) = block.split_at_mut(CHANNEL_HEADER_SIZE * channels);

        for (channel, header) in headers
            .as_chunks_mut::<CHANNEL_HEADER_SIZE>()
            .0
            .iter_mut()
            .enumerate()
        {
            let state = &mut self.states[channel];
            *state = ImaAdpcmState::new(input[channel], state.index);

            header[..2].copy_from_slice(&input[channel].to_le_bytes());
            header[2] = state.index;
            header[3] = 0;
        }

        for (group, bytes) in data
            .chunks_exact_mut(CHANNEL_GROUP_SIZE * channels)
            .enumerate()
        {
            for (channel, bytes) in bytes
                .as_chunks_mut::<CHANNEL_GROUP_SIZE>()
                .0
                .iter_mut()
                .enumerate()
            {
                let state = &mut self.states[channel];
                let first = 1 + group * SAMPLES_PER_GROUP;

                for (offset, byte) in bytes.iter_mut().enumerate() {
                    let sample = first + offset * 2;
                    let low = state.encode(input[sample * channels + channel]);
                    let high = state.encode(input[(sample + 1) * channels + channel]);

                    *byte = low | (high << 4);
                }
            }
        }
    }
}

impl<const MAXIMUM_CHANNELS: usize> Decoder for ImaAdpcm<MAXIMUM_CHANNELS> {
    type Sample = i16;

    fn configuration(&self) -> Option<Configuration> {
        Some(self.pcm_configuration())
    }

    /// One block: [`samples_per_block`](ImaAdpcm::samples_per_block) samples per channel.
    fn minimum_output(&self) -> usize {
        self.block_samples()
    }

    /// Lets the next calls decode a final partial block, up to its last whole group of 4 bytes per channel; without it, a partial block waits for more input.
    fn finish(&mut self) {
        self.finishing = true;
    }

    /// Decodes as many whole blocks as `input` and `output` hold.
    ///
    /// Returns [`Error::BufferTooSmall`] if `output` is shorter than one block.
    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
        self.decode_into(input, output)
    }
}

impl<const MAXIMUM_CHANNELS: usize> ImaAdpcm<MAXIMUM_CHANNELS> {
    /// Like [`Decoder::decode`], but writes any [`Sample`] format.
    pub fn decode_into<S: Sample>(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        let block_samples = self.block_samples();

        if output.len() < block_samples {
            return Err(Error::BufferTooSmall);
        }

        let mut progress = Progress::default();

        for (block, output) in input
            .chunks_exact(self.block_align as usize)
            .zip(output.chunks_exact_mut(block_samples))
        {
            self.decode_block(block, output);

            progress.consumed += block.len();
            progress.produced += block_samples;
        }

        let remainder = &input[progress.consumed..];
        let samples = self.partial_block_samples(remainder.len());

        if self.finishing
            && remainder.len() < self.block_align as usize
            && samples > 0
            && samples <= output.len() - progress.produced
        {
            self.decode_block(remainder, &mut output[progress.produced..]);

            progress.consumed += remainder.len();
            progress.produced += samples;
        }

        if self.skip != 0 {
            progress.produced = discard(output, progress.produced, &mut self.skip);
        }

        Ok(progress)
    }
}

impl<const MAXIMUM_CHANNELS: usize> Seek for ImaAdpcm<MAXIMUM_CHANNELS> {
    /// Seeks to the start of the block holding `frame`, and drops the samples before `frame` from the next output, so the result is sample-exact.
    ///
    /// The byte offset is a whole number of blocks from the start of the encoded data.
    fn seek(&mut self, frame: u64) -> Result<SeekRequest> {
        let (request, skip) = seek_block(
            frame,
            self.channels,
            self.block_align,
            self.samples_per_block(),
        )?;

        self.skip = skip;
        self.finishing = false;

        Ok(request)
    }
}

impl<const MAXIMUM_CHANNELS: usize> Encoder for ImaAdpcm<MAXIMUM_CHANNELS> {
    type Sample = i16;

    fn configuration(&self) -> Configuration {
        self.pcm_configuration()
    }

    /// One block of interleaved samples.
    fn minimum_input(&self) -> usize {
        self.block_samples()
    }

    /// One block of [`block_align`](ImaAdpcm::block_align) bytes.
    fn minimum_output(&self) -> usize {
        self.block_align as usize
    }

    /// Encodes as many whole blocks as `input` and `output` hold.
    ///
    /// A final partial block is not encoded: pad it with silence to a whole block. Returns [`Error::BufferTooSmall`] if `output` is shorter than one block.
    fn encode(&mut self, input: &[i16], output: &mut [u8]) -> Result<Progress> {
        if output.len() < self.block_align as usize {
            return Err(Error::BufferTooSmall);
        }

        let block_samples = self.block_samples();
        let mut progress = Progress::default();

        for (input, block) in input
            .chunks_exact(block_samples)
            .zip(output.chunks_exact_mut(self.block_align as usize))
        {
            self.encode_block(input, block);

            progress.consumed += block_samples;
            progress.produced += block.len();
        }

        Ok(progress)
    }
}
