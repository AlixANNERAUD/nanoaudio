use crate::{Configuration, Error, Format, MAXIMUM_CHANNELS, Result};

use super::{Decoder, Encoder, Progress};

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ImaAdpcmState {
    predictor: i16,
    index: u8,
}

impl ImaAdpcmState {
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

    pub const fn predictor(&self) -> i16 {
        self.predictor
    }

    pub const fn index(&self) -> u8 {
        self.index
    }

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImaAdpcm {
    sample_rate: u32,
    channels: u8,
    block_align: u16,
    states: [ImaAdpcmState; MAXIMUM_CHANNELS],
}

impl ImaAdpcm {
    pub fn new(sample_rate: u32, channels: u8, block_align: u16) -> Result<Self> {
        let header = CHANNEL_HEADER_SIZE * channels as usize;
        let group = CHANNEL_GROUP_SIZE * channels as usize;
        let block = block_align as usize;

        if channels == 0 || channels as usize > MAXIMUM_CHANNELS {
            return Err(Error::TooManyChannels);
        }

        if sample_rate == 0 || block <= header || (block - header) % group != 0 {
            return Err(Error::InvalidParameter);
        }

        Ok(Self {
            sample_rate,
            channels,
            block_align,
            states: [ImaAdpcmState::default(); MAXIMUM_CHANNELS],
        })
    }

    pub fn with_samples_per_block(
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

        Self::new(
            sample_rate,
            channels,
            u16::try_from(block_align).map_err(|_| Error::InvalidParameter)?,
        )
    }

    pub const fn block_align(&self) -> u16 {
        self.block_align
    }

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

    fn pcm_configuration(&self) -> Configuration {
        Configuration::new(self.sample_rate, self.channels, Format::I16)
    }

    fn decode_block(&mut self, block: &[u8], output: &mut [i16]) {
        let channels = self.channels as usize;
        let (headers, data) = block.split_at(CHANNEL_HEADER_SIZE * channels);

        for (channel, header) in headers.chunks_exact(CHANNEL_HEADER_SIZE).enumerate() {
            let predictor = i16::from_le_bytes([header[0], header[1]]);

            self.states[channel] = ImaAdpcmState::new(predictor, header[2]);
            output[channel] = predictor;
        }

        for (group, bytes) in data.chunks_exact(CHANNEL_GROUP_SIZE * channels).enumerate() {
            for (channel, bytes) in bytes.chunks_exact(CHANNEL_GROUP_SIZE).enumerate() {
                let state = &mut self.states[channel];
                let first = 1 + group * SAMPLES_PER_GROUP;

                for (offset, byte) in bytes.iter().enumerate() {
                    let sample = first + offset * 2;

                    output[sample * channels + channel] = state.decode(byte & 0x0F);
                    output[(sample + 1) * channels + channel] = state.decode(byte >> 4);
                }
            }
        }
    }

    fn encode_block(&mut self, input: &[i16], block: &mut [u8]) {
        let channels = self.channels as usize;
        let (headers, data) = block.split_at_mut(CHANNEL_HEADER_SIZE * channels);

        for (channel, header) in headers.chunks_exact_mut(CHANNEL_HEADER_SIZE).enumerate() {
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
            for (channel, bytes) in bytes.chunks_exact_mut(CHANNEL_GROUP_SIZE).enumerate() {
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

impl Decoder for ImaAdpcm {
    type Sample = i16;

    fn configuration(&self) -> Option<Configuration> {
        Some(self.pcm_configuration())
    }

    fn minimum_output(&self) -> usize {
        self.block_samples()
    }

    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
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

        Ok(progress)
    }
}

impl Encoder for ImaAdpcm {
    type Sample = i16;

    fn configuration(&self) -> Configuration {
        self.pcm_configuration()
    }

    fn minimum_input(&self) -> usize {
        self.block_samples()
    }

    fn minimum_output(&self) -> usize {
        self.block_align as usize
    }

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
