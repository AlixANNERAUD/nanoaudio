use nanoaudio_core::codec::{Decoder, Encoder, Progress, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Format, Result, Sample};

use crate::{discard, seek_block};

const ADAPTATIONS: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

const MINIMUM_DELTA: i16 = 16;
const CHANNEL_HEADER_SIZE: usize = 7;
const HEADER_SAMPLES: usize = 2;
const MAXIMUM_CHANNELS: usize = 2;
const DELTA_ESTIMATION_SAMPLES: usize = 3;

pub const MS_ADPCM_MAXIMUM_COEFFICIENTS: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MsAdpcmCoefficients {
    pairs: [[i16; 2]; MS_ADPCM_MAXIMUM_COEFFICIENTS],
    count: u8,
}

impl MsAdpcmCoefficients {
    pub const STANDARD: Self = Self {
        pairs: [
            [256, 0],
            [512, -256],
            [0, 0],
            [192, 64],
            [240, 0],
            [460, -208],
            [392, -232],
        ],
        count: 7,
    };

    pub fn new(pairs: &[[i16; 2]]) -> Result<Self> {
        if pairs.is_empty() || pairs.len() > MS_ADPCM_MAXIMUM_COEFFICIENTS {
            return Err(Error::InvalidParameter);
        }

        let mut coefficients = Self {
            pairs: [[0; 2]; MS_ADPCM_MAXIMUM_COEFFICIENTS],
            count: pairs.len() as u8,
        };
        coefficients.pairs[..pairs.len()].copy_from_slice(pairs);

        Ok(coefficients)
    }

    pub fn as_slice(&self) -> &[[i16; 2]] {
        &self.pairs[..self.count as usize]
    }
}

impl Default for MsAdpcmCoefficients {
    fn default() -> Self {
        Self::STANDARD
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MsAdpcmState {
    coefficients: [i16; 2],
    delta: i16,
    previous: i16,
    before_previous: i16,
}

impl MsAdpcmState {
    pub const fn new(
        coefficients: [i16; 2],
        delta: i16,
        previous: i16,
        before_previous: i16,
    ) -> Self {
        Self {
            coefficients,
            delta,
            previous,
            before_previous,
        }
    }

    pub const fn coefficients(&self) -> [i16; 2] {
        self.coefficients
    }

    pub const fn delta(&self) -> i16 {
        self.delta
    }

    pub const fn previous(&self) -> i16 {
        self.previous
    }

    pub const fn before_previous(&self) -> i16 {
        self.before_previous
    }

    fn prediction(&self) -> i32 {
        (self.previous as i32)
            .wrapping_mul(self.coefficients[0] as i32)
            .wrapping_add((self.before_previous as i32).wrapping_mul(self.coefficients[1] as i32))
            >> 8
    }

    pub fn decode(&mut self, nibble: u8) -> i16 {
        let nibble = nibble & 0x0F;
        let signed = ((nibble << 4) as i8 >> 4) as i32;
        let sample = (self.prediction() + signed * self.delta as i32)
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;

        self.delta =
            (((ADAPTATIONS[nibble as usize] * self.delta as i32) >> 8) as i16).max(MINIMUM_DELTA);
        self.before_previous = self.previous;
        self.previous = sample;

        sample
    }

    pub fn encode(&mut self, sample: i16) -> u8 {
        let delta = (self.delta as i32).max(1);
        let difference = sample as i32 - self.prediction();
        let bias = if difference >= 0 {
            delta / 2
        } else {
            -delta / 2
        };
        let nibble = ((difference + bias) / delta).clamp(-8, 7) as u8 & 0x0F;

        self.decode(nibble);

        nibble
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MsAdpcm {
    sample_rate: u32,
    channels: u8,
    block_align: u16,
    samples_per_block: u16,
    coefficients: MsAdpcmCoefficients,
    skip: usize,
}

impl MsAdpcm {
    pub fn new(sample_rate: u32, channels: u8, block_align: u16) -> Result<Self> {
        let data = (block_align as usize).saturating_sub(CHANNEL_HEADER_SIZE * channels as usize);
        let samples_per_block = data * 2 / (channels as usize).max(1) + HEADER_SAMPLES;

        Self::with_coefficients(
            sample_rate,
            channels,
            block_align,
            u16::try_from(samples_per_block).map_err(|_| Error::InvalidParameter)?,
            MsAdpcmCoefficients::STANDARD,
        )
    }

    pub fn with_samples_per_block(
        sample_rate: u32,
        channels: u8,
        samples_per_block: u16,
    ) -> Result<Self> {
        let nibbles = (samples_per_block as usize)
            .checked_sub(HEADER_SAMPLES)
            .ok_or(Error::InvalidParameter)?
            * channels as usize;
        let block_align = CHANNEL_HEADER_SIZE * channels as usize + nibbles.div_ceil(2);

        Self::with_coefficients(
            sample_rate,
            channels,
            u16::try_from(block_align).map_err(|_| Error::InvalidParameter)?,
            samples_per_block,
            MsAdpcmCoefficients::STANDARD,
        )
    }

    pub fn with_coefficients(
        sample_rate: u32,
        channels: u8,
        block_align: u16,
        samples_per_block: u16,
        coefficients: MsAdpcmCoefficients,
    ) -> Result<Self> {
        if channels == 0 || channels as usize > MAXIMUM_CHANNELS {
            return Err(Error::TooManyChannels);
        }

        let header = CHANNEL_HEADER_SIZE * channels as usize;
        let block = block_align as usize;
        let samples = samples_per_block as usize;

        if sample_rate == 0
            || block <= header
            || samples <= HEADER_SAMPLES
            || (samples - HEADER_SAMPLES) * channels as usize > (block - header) * 2
        {
            return Err(Error::InvalidParameter);
        }

        Ok(Self {
            sample_rate,
            channels,
            block_align,
            samples_per_block,
            coefficients,
            skip: 0,
        })
    }

    pub const fn block_align(&self) -> u16 {
        self.block_align
    }

    pub const fn samples_per_block(&self) -> usize {
        self.samples_per_block as usize
    }

    pub const fn coefficients(&self) -> &MsAdpcmCoefficients {
        &self.coefficients
    }

    fn block_samples(&self) -> usize {
        self.samples_per_block() * self.channels as usize
    }

    fn pcm_configuration(&self) -> Configuration {
        Configuration::new(self.sample_rate, self.channels, Format::I16)
    }

    fn read_states(&self, headers: &[u8]) -> Result<[MsAdpcmState; MAXIMUM_CHANNELS]> {
        let channels = self.channels as usize;
        let field = |index: usize, channel: usize| {
            let offset = channels + (index * channels + channel) * 2;

            i16::from_le_bytes([headers[offset], headers[offset + 1]])
        };
        let mut states = [MsAdpcmState::default(); MAXIMUM_CHANNELS];

        for (channel, state) in states[..channels].iter_mut().enumerate() {
            let coefficients = self
                .coefficients
                .as_slice()
                .get(headers[channel] as usize)
                .ok_or(Error::InvalidData)?;

            *state = MsAdpcmState::new(
                *coefficients,
                field(0, channel),
                field(1, channel),
                field(2, channel),
            );
        }

        Ok(states)
    }

    fn decode_block<S: Sample>(&self, block: &[u8], output: &mut [S]) -> Result<()> {
        let channels = self.channels as usize;
        let (headers, data) = block.split_at(CHANNEL_HEADER_SIZE * channels);
        let mut states = self.read_states(headers)?;

        for (channel, state) in states[..channels].iter().enumerate() {
            output[channel] = state.before_previous.convert();
            output[channels + channel] = state.previous.convert();
        }

        let nibbles = data.iter().flat_map(|byte| [byte >> 4, byte & 0x0F]);

        for (index, (output, nibble)) in output[HEADER_SAMPLES * channels..self.block_samples()]
            .iter_mut()
            .zip(nibbles)
            .enumerate()
        {
            *output = states[index % channels].decode(nibble).convert();
        }

        Ok(())
    }

    fn encode_channel(
        &self,
        input: &[i16],
        channel: usize,
        mut state: MsAdpcmState,
        mut data: Option<&mut [u8]>,
    ) -> u64 {
        let channels = self.channels as usize;
        let mut error = 0;

        for frame in HEADER_SAMPLES..self.samples_per_block() {
            let sample = input[frame * channels + channel];
            let nibble = state.encode(sample);
            let difference = sample as i64 - state.previous as i64;

            error += (difference * difference) as u64;

            if let Some(data) = data.as_deref_mut() {
                let index = (frame - HEADER_SAMPLES) * channels + channel;
                let shift = if index.is_multiple_of(2) { 4 } else { 0 };

                data[index / 2] |= nibble << shift;
            }
        }

        error
    }

    fn initial_state(&self, input: &[i16], channel: usize, coefficients: [i16; 2]) -> MsAdpcmState {
        let channels = self.channels as usize;
        let sample = |frame: usize| input[frame * channels + channel];
        let mut state = MsAdpcmState::new(coefficients, 0, sample(1), sample(0));
        let frames = DELTA_ESTIMATION_SAMPLES.min(self.samples_per_block() - HEADER_SAMPLES);
        let mut sum = 0i32;

        for frame in HEADER_SAMPLES..HEADER_SAMPLES + frames {
            sum += (sample(frame) as i32 - state.prediction()).abs();
            state.before_previous = state.previous;
            state.previous = sample(frame);
        }

        let delta = (sum / (4 * frames as i32)).clamp(MINIMUM_DELTA as i32, i16::MAX as i32);

        MsAdpcmState::new(coefficients, delta as i16, sample(1), sample(0))
    }

    fn encode_block(&self, input: &[i16], block: &mut [u8]) {
        let channels = self.channels as usize;
        let (headers, data) = block.split_at_mut(CHANNEL_HEADER_SIZE * channels);

        data.fill(0);

        for channel in 0..channels {
            let mut best = (u64::MAX, 0, MsAdpcmState::default());

            for (predictor, coefficients) in self.coefficients.as_slice().iter().enumerate() {
                let state = self.initial_state(input, channel, *coefficients);
                let error = self.encode_channel(input, channel, state, None);

                if error < best.0 {
                    best = (error, predictor, state);
                }
            }

            let (_, predictor, state) = best;
            let fields = [state.delta, state.previous, state.before_previous];

            headers[channel] = predictor as u8;

            for (index, value) in fields.iter().enumerate() {
                let offset = channels + (index * channels + channel) * 2;

                headers[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
            }

            self.encode_channel(input, channel, state, Some(data));
        }
    }
}

impl Decoder for MsAdpcm {
    type Sample = i16;

    fn configuration(&self) -> Option<Configuration> {
        Some(self.pcm_configuration())
    }

    fn minimum_output(&self) -> usize {
        self.block_samples()
    }

    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
        self.decode_into(input, output)
    }
}

impl MsAdpcm {
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
            if let Err(error) = self.decode_block(block, output) {
                if progress.is_stalled() {
                    return Err(error);
                }

                break;
            }

            progress.consumed += block.len();
            progress.produced += block_samples;
        }

        if self.skip != 0 {
            progress.produced = discard(output, progress.produced, &mut self.skip);
        }

        Ok(progress)
    }
}

impl Seek for MsAdpcm {
    fn seek(&mut self, frame: u64) -> Result<SeekRequest> {
        let (request, skip) = seek_block(
            frame,
            self.channels,
            self.block_align,
            self.samples_per_block(),
        )?;

        self.skip = skip;

        Ok(request)
    }
}

impl Encoder for MsAdpcm {
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
