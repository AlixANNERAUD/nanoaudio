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

/// The most coefficient pairs an [`MsAdpcmCoefficients`] holds.
pub const MS_ADPCM_MAXIMUM_COEFFICIENTS: usize = 7;

/// The predictor coefficient pairs of an MS ADPCM stream, in Q8.
///
/// Each block header picks one pair by index. WAV files list them in the `fmt ` chunk, and almost all use [`STANDARD`](Self::STANDARD).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MsAdpcmCoefficients {
    pairs: [[i16; 2]; MS_ADPCM_MAXIMUM_COEFFICIENTS],
    count: u8,
}

impl MsAdpcmCoefficients {
    /// The 7 pairs defined by Microsoft.
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

    /// Creates a set from 1 to 7 pairs.
    ///
    /// Returns [`Error::InvalidParameter`] if `pairs` is empty or longer than [`MS_ADPCM_MAXIMUM_COEFFICIENTS`].
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

    /// Returns the pairs.
    pub fn as_slice(&self) -> &[[i16; 2]] {
        &self.pairs[..self.count as usize]
    }
}

impl Default for MsAdpcmCoefficients {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// The predictor state of one MS ADPCM channel.
///
/// Use it to code samples one at a time, outside the WAV block layout of [`MsAdpcm`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MsAdpcmState {
    coefficients: [i16; 2],
    delta: i16,
    previous: i16,
    before_previous: i16,
}

impl MsAdpcmState {
    /// Creates a state from a coefficient pair, a quantization step and the last two samples.
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

    /// Returns the coefficient pair, in Q8.
    pub const fn coefficients(&self) -> [i16; 2] {
        self.coefficients
    }

    /// Returns the quantization step.
    pub const fn delta(&self) -> i16 {
        self.delta
    }

    /// Returns the last sample.
    pub const fn previous(&self) -> i16 {
        self.previous
    }

    /// Returns the sample before the last one.
    pub const fn before_previous(&self) -> i16 {
        self.before_previous
    }

    fn prediction(&self) -> i32 {
        (self.previous as i32)
            .wrapping_mul(self.coefficients[0] as i32)
            .wrapping_add((self.before_previous as i32).wrapping_mul(self.coefficients[1] as i32))
            >> 8
    }

    /// Decodes the signed 4-bit code in the low nibble of `nibble` and returns the sample.
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

    /// Encodes `sample` to a 4-bit code, updating the state as the decoder will.
    pub fn encode(&mut self, sample: i16) -> u8 {
        let delta = (self.delta as i32).max(1);
        let difference = sample as i32 - self.prediction();
        let nibble = quantize(difference, delta);

        self.decode(nibble);

        nibble
    }
}

const DIVISION: bool = cfg!(any(
    all(target_arch = "arm", not(nanoaudio_no_division)),
    all(
        any(target_arch = "riscv32", target_arch = "riscv64"),
        target_feature = "m"
    ),
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "arm64ec",
    target_arch = "wasm32",
    target_arch = "wasm64",
    target_arch = "loongarch64",
    target_arch = "mips",
    target_arch = "mips64",
    target_arch = "powerpc",
    target_arch = "powerpc64",
    target_arch = "s390x",
));

fn quantize(difference: i32, delta: i32) -> u8 {
    if DIVISION {
        quantize_by_division(difference, delta)
    } else {
        quantize_by_comparison(difference, delta)
    }
}

fn quantize_by_division(difference: i32, delta: i32) -> u8 {
    let bias = if difference >= 0 {
        delta / 2
    } else {
        -delta / 2
    };

    ((difference + bias) / delta).clamp(-8, 7) as u8 & 0x0F
}

fn quantize_by_comparison(difference: i32, delta: i32) -> u8 {
    let mut remainder = difference.unsigned_abs() + (delta >> 1) as u32;
    let delta = delta as u32;
    let mut quotient = 0u8;

    if remainder >= delta << 3 {
        quotient = 8;
    } else {
        if remainder >= delta << 2 {
            remainder -= delta << 2;
            quotient = 4;
        }

        if remainder >= delta << 1 {
            remainder -= delta << 1;
            quotient |= 2;
        }

        if remainder >= delta {
            quotient |= 1;
        }
    }

    if difference < 0 {
        quotient.wrapping_neg() & 0x0F
    } else {
        quotient.min(7)
    }
}

/// A Microsoft ADPCM encoder and decoder with the WAV block layout.
///
/// Each block starts with a 7-byte header per channel, holding the coefficient index, the step and the first two samples, followed by 4 bits per sample, interleaved. Mono or stereo at any sample rate; the blocks store neither, so they come from the container, like the block size. The encoder tries every coefficient pair for each block, so it takes 10 to 30 times longer than the decoder.
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
    /// Creates a codec from `block_align`, the size of one block in bytes, with as many samples per block as fit and the [standard coefficients](MsAdpcmCoefficients::STANDARD).
    ///
    /// Returns [`Error::TooManyChannels`] for 0 or more than 2 channels, and [`Error::InvalidParameter`] if `sample_rate` is zero or `block_align` leaves no room for samples after the headers.
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

    /// Creates a codec from the number of samples per channel in one block, with the smallest block that holds them and the [standard coefficients](MsAdpcmCoefficients::STANDARD).
    ///
    /// Returns [`Error::InvalidParameter`] for fewer than 3 samples, and the same errors as [`new`](Self::new).
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

    /// Creates a codec from the values of a WAV `fmt ` chunk: block size, samples per channel in one block and coefficients.
    ///
    /// Returns [`Error::TooManyChannels`] for 0 or more than 2 channels, and [`Error::InvalidParameter`] if `sample_rate` is zero, `samples_per_block` is under 3, or the samples do not fit in `block_align`.
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

    /// Returns the size of one block in bytes.
    pub const fn block_align(&self) -> u16 {
        self.block_align
    }

    /// Returns the number of samples per channel in one block.
    pub const fn samples_per_block(&self) -> usize {
        self.samples_per_block as usize
    }

    /// Returns the coefficient pairs blocks can pick from.
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

        let samples = &mut output[HEADER_SAMPLES * channels..self.block_samples()];
        let (pairs, last) = samples.as_chunks_mut::<2>();
        let [left, right] = &mut states;

        if channels == 1 {
            for ([high, low], byte) in pairs.iter_mut().zip(data) {
                *high = left.decode(byte >> 4).convert();
                *low = left.decode(byte & 0x0F).convert();
            }
        } else {
            for ([high, low], byte) in pairs.iter_mut().zip(data) {
                *high = left.decode(byte >> 4).convert();
                *low = right.decode(byte & 0x0F).convert();
            }
        }

        if let ([last], Some(byte)) = (last, data.get(pairs.len())) {
            *last = left.decode(byte >> 4).convert();
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
            let difference = (sample as i32 - state.previous as i32).unsigned_abs();

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

    /// One block: [`samples_per_block`](MsAdpcm::samples_per_block) samples per channel.
    fn minimum_output(&self) -> usize {
        self.block_samples()
    }

    /// Decodes as many whole blocks as `input` and `output` hold. A final partial block is not decoded.
    ///
    /// Returns [`Error::BufferTooSmall`] if `output` is shorter than one block, and [`Error::InvalidData`] when the next block picks a coefficient pair that does not exist.
    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
        self.decode_into(input, output)
    }
}

impl MsAdpcm {
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

        Ok(request)
    }
}

impl Encoder for MsAdpcm {
    type Sample = i16;

    fn configuration(&self) -> Configuration {
        self.pcm_configuration()
    }

    /// One block of interleaved samples.
    fn minimum_input(&self) -> usize {
        self.block_samples()
    }

    /// One block of [`block_align`](MsAdpcm::block_align) bytes.
    fn minimum_output(&self) -> usize {
        self.block_align as usize
    }

    /// Encodes as many whole blocks as `input` and `output` hold, keeping for each channel the coefficient pair with the smallest error.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_matches_division() {
        let step = if cfg!(miri) { 997 } else { 1 };

        for delta in (1..=i16::MAX as i32).step_by(step) {
            let half = delta / 2;
            let steps = (-17..=17).flat_map(|step| [step * delta, step * delta + half]);

            for base in steps.chain([-140_000, 140_000]) {
                for difference in base - 2..=base + 2 {
                    assert_eq!(
                        quantize_by_comparison(difference, delta),
                        quantize_by_division(difference, delta),
                        "{difference} / {delta}"
                    );
                }
            }
        }
    }
}
