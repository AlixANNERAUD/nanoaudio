#![no_std]
#![doc = include_str!("../README.md")]

use core::marker::PhantomData;

use nanoaudio_core::codec::{Decoder, Encoder, Progress};
use nanoaudio_core::{Configuration, Error, MAXIMUM_CHANNELS, Result, Sample};

pub const QOA_SLICE_LENGTH: usize = 20;
pub const QOA_SLICES_PER_FRAME: usize = 256;
pub const QOA_FRAME_LENGTH: usize = QOA_SLICE_LENGTH * QOA_SLICES_PER_FRAME;
pub const QOA_MAXIMUM_SAMPLE_RATE: u32 = 0xFF_FFFF;

const MAGIC: [u8; 4] = *b"qoaf";
const FILE_HEADER_SIZE: usize = 8;
const FRAME_HEADER_SIZE: usize = 8;
const CHANNEL_STATE_SIZE: usize = 16;
const SLICE_SIZE: usize = 8;
const LMS_LENGTH: usize = 4;
const SCALE_FACTORS: usize = 16;

const QUANTIZE: [u8; 17] = [7, 7, 7, 5, 5, 3, 3, 1, 0, 0, 2, 2, 4, 4, 6, 6, 6];

const RECIPROCALS: [i32; SCALE_FACTORS] = [
    65536, 9363, 3121, 1457, 781, 475, 311, 216, 156, 117, 90, 71, 57, 47, 39, 32,
];

const DEQUANTIZE: [[i32; 8]; SCALE_FACTORS] = [
    [1, -1, 3, -3, 5, -5, 7, -7],
    [5, -5, 18, -18, 32, -32, 49, -49],
    [16, -16, 53, -53, 95, -95, 147, -147],
    [34, -34, 113, -113, 203, -203, 315, -315],
    [63, -63, 210, -210, 378, -378, 588, -588],
    [104, -104, 345, -345, 621, -621, 966, -966],
    [158, -158, 528, -528, 950, -950, 1477, -1477],
    [228, -228, 760, -760, 1368, -1368, 2128, -2128],
    [316, -316, 1053, -1053, 1895, -1895, 2947, -2947],
    [422, -422, 1405, -1405, 2529, -2529, 3934, -3934],
    [548, -548, 1828, -1828, 3290, -3290, 5117, -5117],
    [696, -696, 2320, -2320, 4176, -4176, 6496, -6496],
    [868, -868, 2893, -2893, 5207, -5207, 8099, -8099],
    [1064, -1064, 3548, -3548, 6386, -6386, 9933, -9933],
    [1286, -1286, 4288, -4288, 7718, -7718, 12005, -12005],
    [1536, -1536, 5120, -5120, 9216, -9216, 14336, -14336],
];

pub const fn qoa_frame_size(channels: u8, samples: usize) -> usize {
    let channels = channels as usize;
    let slices = samples.div_ceil(QOA_SLICE_LENGTH);

    FRAME_HEADER_SIZE + CHANNEL_STATE_SIZE * channels + SLICE_SIZE * slices * channels
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Lms {
    history: [i32; LMS_LENGTH],
    weights: [i32; LMS_LENGTH],
}

impl Lms {
    const INITIAL: Self = Self {
        history: [0; LMS_LENGTH],
        weights: [0, 0, -(1 << 13), 1 << 14],
    };

    fn predict(&self) -> i32 {
        self.history
            .iter()
            .zip(&self.weights)
            .fold(0i32, |sum, (history, weight)| {
                sum.wrapping_add(history.wrapping_mul(*weight))
            })
            >> 13
    }

    fn update(&mut self, sample: i32, residual: i32) {
        let delta = residual >> 4;

        for (weight, history) in self.weights.iter_mut().zip(&self.history) {
            *weight = weight.wrapping_add(if *history < 0 { -delta } else { delta });
        }

        self.history.copy_within(1.., 0);
        self.history[LMS_LENGTH - 1] = sample;
    }

    fn penalty(&self) -> i32 {
        let energy = self.weights.iter().fold(0i32, |sum, weight| {
            sum.wrapping_add(weight.wrapping_mul(*weight))
        });

        ((energy >> 18) - 0x8FF).max(0)
    }

    fn read(bytes: &[u8; CHANNEL_STATE_SIZE]) -> Self {
        let mut lms = Self::INITIAL;

        for index in 0..LMS_LENGTH {
            lms.history[index] =
                i16::from_be_bytes([bytes[index * 2], bytes[index * 2 + 1]]) as i32;
            lms.weights[index] =
                i16::from_be_bytes([bytes[8 + index * 2], bytes[8 + index * 2 + 1]]) as i32;
        }

        lms
    }

    fn write(&self, bytes: &mut [u8; CHANNEL_STATE_SIZE]) {
        for index in 0..LMS_LENGTH {
            bytes[index * 2..index * 2 + 2]
                .copy_from_slice(&(self.history[index] as i16).to_be_bytes());
            bytes[8 + index * 2..8 + index * 2 + 2]
                .copy_from_slice(&(self.weights[index] as i16).to_be_bytes());
        }
    }
}

fn clamp_i16(value: i32) -> i32 {
    value.clamp(i16::MIN as i32, i16::MAX as i32)
}

fn divide(value: i32, scale_factor: usize) -> i32 {
    let quotient = value
        .wrapping_mul(RECIPROCALS[scale_factor])
        .wrapping_add(1 << 15)
        >> 16;

    quotient + value.signum() - quotient.signum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Stage {
    FileHeader,
    FrameHeader,
    Slices,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QoaDecoder<S: Sample = i16> {
    stage: Stage,
    total: Option<u32>,
    remaining: u32,
    sample_rate: u32,
    channels: u8,
    frame_remaining: usize,
    lms: [Lms; MAXIMUM_CHANNELS],
    finishing: bool,
    sample: PhantomData<S>,
}

impl<S: Sample> QoaDecoder<S> {
    pub const fn new() -> Self {
        Self {
            stage: Stage::FileHeader,
            total: None,
            remaining: 0,
            sample_rate: 0,
            channels: 0,
            frame_remaining: 0,
            lms: [Lms::INITIAL; MAXIMUM_CHANNELS],
            finishing: false,
            sample: PhantomData,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub const fn samples(&self) -> Option<u32> {
        self.total
    }

    pub const fn is_streaming(&self) -> bool {
        matches!(self.total, Some(0))
    }

    fn read_file_header(&mut self, input: &[u8]) -> Result<Option<Progress>> {
        if input.len() < FILE_HEADER_SIZE {
            return Ok(None);
        }

        if input[..4] != MAGIC {
            return Err(Error::InvalidHeader);
        }

        let samples = u32::from_be_bytes([input[4], input[5], input[6], input[7]]);

        self.total = Some(samples);
        self.remaining = samples;
        self.stage = Stage::FrameHeader;

        Ok(Some(Progress::new(FILE_HEADER_SIZE, 0)))
    }

    fn read_frame_header(&mut self, input: &[u8]) -> Result<Option<Progress>> {
        let Some(header) = input.first_chunk::<FRAME_HEADER_SIZE>() else {
            return Ok(None);
        };

        let header = u64::from_be_bytes(*header);
        let channels = (header >> 56) as u8;
        let sample_rate = ((header >> 32) & 0xFF_FFFF) as u32;
        let samples = ((header >> 16) & 0xFFFF) as usize;
        let size = (header & 0xFFFF) as usize;

        if channels as usize > MAXIMUM_CHANNELS {
            return Err(Error::TooManyChannels);
        }

        if channels == 0
            || sample_rate == 0
            || samples == 0
            || samples > QOA_FRAME_LENGTH
            || size != qoa_frame_size(channels, samples)
        {
            return Err(Error::InvalidData);
        }

        if !self.is_streaming() {
            let remaining = self.remaining as usize;

            if (self.channels != 0
                && (channels != self.channels || sample_rate != self.sample_rate))
                || samples > remaining
                || (samples < QOA_FRAME_LENGTH && samples != remaining)
            {
                return Err(Error::InvalidData);
            }
        }

        let header_size = FRAME_HEADER_SIZE + CHANNEL_STATE_SIZE * channels as usize;

        if input.len() < header_size {
            return Ok(None);
        }

        for (lms, state) in self.lms.iter_mut().zip(
            input[FRAME_HEADER_SIZE..header_size]
                .as_chunks::<CHANNEL_STATE_SIZE>()
                .0,
        ) {
            *lms = Lms::read(state);
        }

        self.channels = channels;
        self.sample_rate = sample_rate;
        self.frame_remaining = samples;
        self.stage = Stage::Slices;

        Ok(Some(Progress::new(header_size, 0)))
    }

    fn read_slices(&mut self, input: &[u8], output: &mut [S]) -> Option<Progress> {
        let channels = self.channels as usize;
        let length = self.frame_remaining.min(QOA_SLICE_LENGTH);
        let size = SLICE_SIZE * channels;

        if input.len() < size || output.len() < length * channels {
            return None;
        }

        for (channel, (bytes, lms)) in input[..size]
            .as_chunks::<SLICE_SIZE>()
            .0
            .iter()
            .zip(&mut self.lms)
            .enumerate()
        {
            let mut slice = u64::from_be_bytes(*bytes);
            let scale_factor = (slice >> 60) as usize;
            slice <<= 4;

            for frame in output[..length * channels].chunks_exact_mut(channels) {
                let predicted = lms.predict();
                let quantized = (slice >> 61) as usize;
                let dequantized = DEQUANTIZE[scale_factor][quantized];
                let reconstructed = clamp_i16(predicted.wrapping_add(dequantized));

                frame[channel] = (reconstructed as i16).convert();
                slice <<= 3;
                lms.update(reconstructed, dequantized);
            }
        }

        self.frame_remaining -= length;

        if !self.is_streaming() {
            self.remaining -= length as u32;
        }

        if self.frame_remaining == 0 {
            self.stage = if !self.is_streaming() && self.remaining == 0 {
                Stage::End
            } else {
                Stage::FrameHeader
            };
        }

        Some(Progress::new(size, length * channels))
    }

    fn step(&mut self, input: &[u8], output: &mut [S]) -> Result<Option<Progress>> {
        match self.stage {
            Stage::FileHeader => self.read_file_header(input),
            Stage::FrameHeader => self.read_frame_header(input),
            Stage::Slices => Ok(self.read_slices(input, output)),
            Stage::End => Ok(None),
        }
    }

    fn ended(&self) -> bool {
        match self.stage {
            Stage::End => true,
            Stage::FrameHeader => self.is_streaming() && self.channels != 0,
            Stage::FileHeader | Stage::Slices => false,
        }
    }
}

impl<S: Sample> Default for QoaDecoder<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Sample> Decoder for QoaDecoder<S> {
    type Sample = S;

    fn configuration(&self) -> Option<Configuration> {
        (self.channels != 0).then(|| Configuration::new(self.sample_rate, self.channels, S::FORMAT))
    }

    fn minimum_output(&self) -> usize {
        QOA_SLICE_LENGTH * MAXIMUM_CHANNELS
    }

    fn finish(&mut self) {
        self.finishing = true;
    }

    fn decode(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        if output.len() < self.minimum_output() {
            return Err(Error::BufferTooSmall);
        }

        let mut progress = Progress::default();

        while let Some(step) = self.step(
            &input[progress.consumed..],
            &mut output[progress.produced..],
        )? {
            progress.consumed += step.consumed;
            progress.produced += step.produced;
        }

        if progress.is_stalled() && self.finishing && !self.ended() {
            return Err(Error::Incomplete);
        }

        Ok(progress)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QoaEncoder<S: Sample = i16> {
    stage: Stage,
    total: u32,
    remaining: u32,
    sample_rate: u32,
    channels: u8,
    frame_remaining: usize,
    lms: [Lms; MAXIMUM_CHANNELS],
    previous_scale_factors: [u8; MAXIMUM_CHANNELS],
    finishing: bool,
    sample: PhantomData<S>,
}

impl<S: Sample> QoaEncoder<S> {
    pub fn new(sample_rate: u32, channels: u8, samples: u32) -> Result<Self> {
        if channels == 0 || channels as usize > MAXIMUM_CHANNELS {
            return Err(Error::TooManyChannels);
        }

        if sample_rate == 0 || sample_rate > QOA_MAXIMUM_SAMPLE_RATE {
            return Err(Error::InvalidParameter);
        }

        Ok(Self {
            stage: Stage::FileHeader,
            total: samples,
            remaining: samples,
            sample_rate,
            channels,
            frame_remaining: 0,
            lms: [Lms::INITIAL; MAXIMUM_CHANNELS],
            previous_scale_factors: [0; MAXIMUM_CHANNELS],
            finishing: false,
            sample: PhantomData,
        })
    }

    pub const fn samples(&self) -> u32 {
        self.total
    }

    pub const fn is_streaming(&self) -> bool {
        self.total == 0
    }

    pub const fn is_finished(&self) -> bool {
        matches!(self.stage, Stage::End)
    }

    fn write_file_header(&mut self, output: &mut [u8]) -> Option<Progress> {
        if output.len() < FILE_HEADER_SIZE {
            return None;
        }

        output[..4].copy_from_slice(&MAGIC);
        output[4..FILE_HEADER_SIZE].copy_from_slice(&self.total.to_be_bytes());
        self.stage = Stage::FrameHeader;

        Some(Progress::new(0, FILE_HEADER_SIZE))
    }

    fn frame_length(&self, input: &[S]) -> Option<usize> {
        let channels = self.channels as usize;

        if !self.is_streaming() {
            return Some((self.remaining as usize).min(QOA_FRAME_LENGTH));
        }

        if input.len() >= QOA_FRAME_LENGTH * channels {
            Some(QOA_FRAME_LENGTH)
        } else if self.finishing {
            Some(input.len() / channels)
        } else {
            None
        }
    }

    fn write_frame_header(&mut self, input: &[S], output: &mut [u8]) -> Option<Progress> {
        let channels = self.channels as usize;
        let header_size = FRAME_HEADER_SIZE + CHANNEL_STATE_SIZE * channels;
        let length = self.frame_length(input)?;

        if length == 0 {
            self.stage = Stage::End;

            return None;
        }

        if output.len() < header_size {
            return None;
        }

        let header = (self.channels as u64) << 56
            | (self.sample_rate as u64) << 32
            | (length as u64) << 16
            | qoa_frame_size(self.channels, length) as u64;

        output[..FRAME_HEADER_SIZE].copy_from_slice(&header.to_be_bytes());

        for (state, lms) in output[FRAME_HEADER_SIZE..header_size]
            .as_chunks_mut::<CHANNEL_STATE_SIZE>()
            .0
            .iter_mut()
            .zip(&self.lms)
        {
            lms.write(state);
        }

        self.previous_scale_factors = [0; MAXIMUM_CHANNELS];
        self.frame_remaining = length;
        self.stage = Stage::Slices;

        Some(Progress::new(0, header_size))
    }

    fn encode_slice(&mut self, channel: usize, input: &[S]) -> u64 {
        let channels = self.channels as usize;
        let start = self.lms[channel];
        let mut best_rank = u64::MAX;
        let mut best_slice = 0;
        let mut best_lms = start;
        let mut best_scale_factor = 0;

        for offset in 0..SCALE_FACTORS {
            let scale_factor =
                (offset + self.previous_scale_factors[channel] as usize) % SCALE_FACTORS;
            let mut lms = start;
            let mut slice = scale_factor as u64;
            let mut rank = 0u64;

            for frame in input.chunks_exact(channels) {
                let sample = frame[channel].convert::<i16>() as i32;
                let predicted = lms.predict();
                let residual = sample.wrapping_sub(predicted);
                let scaled = divide(residual, scale_factor).clamp(-8, 8);
                let quantized = QUANTIZE[(scaled + 8) as usize];
                let dequantized = DEQUANTIZE[scale_factor][quantized as usize];
                let reconstructed = clamp_i16(predicted.wrapping_add(dequantized));
                let penalty = lms.penalty();
                let error = (sample - reconstructed) as i64;

                rank = rank
                    .wrapping_add((error * error) as u64)
                    .wrapping_add((penalty * penalty) as u64);

                if rank > best_rank {
                    break;
                }

                lms.update(reconstructed, dequantized);
                slice = (slice << 3) | quantized as u64;
            }

            if rank < best_rank {
                best_rank = rank;
                best_slice = slice;
                best_lms = lms;
                best_scale_factor = scale_factor;
            }
        }

        self.previous_scale_factors[channel] = best_scale_factor as u8;
        self.lms[channel] = best_lms;

        let length = input.len() / channels;

        best_slice << ((QOA_SLICE_LENGTH - length) * 3)
    }

    fn write_slices(&mut self, input: &[S], output: &mut [u8]) -> Option<Progress> {
        let channels = self.channels as usize;
        let length = self.frame_remaining.min(QOA_SLICE_LENGTH);
        let size = SLICE_SIZE * channels;

        if input.len() < length * channels || output.len() < size {
            return None;
        }

        let input = &input[..length * channels];

        for (channel, bytes) in output[..size]
            .as_chunks_mut::<SLICE_SIZE>()
            .0
            .iter_mut()
            .enumerate()
        {
            *bytes = self.encode_slice(channel, input).to_be_bytes();
        }

        self.frame_remaining -= length;

        if !self.is_streaming() {
            self.remaining -= length as u32;
        }

        if self.frame_remaining == 0 && !self.is_streaming() && self.remaining == 0 {
            self.stage = Stage::End;
        } else if self.frame_remaining == 0 {
            self.stage = Stage::FrameHeader;
        }

        Some(Progress::new(length * channels, size))
    }

    fn step(&mut self, input: &[S], output: &mut [u8]) -> Option<Progress> {
        match self.stage {
            Stage::FileHeader => self.write_file_header(output),
            Stage::FrameHeader => self.write_frame_header(input, output),
            Stage::Slices => self.write_slices(input, output),
            Stage::End => None,
        }
    }
}

impl<S: Sample> Encoder for QoaEncoder<S> {
    type Sample = S;

    fn configuration(&self) -> Configuration {
        Configuration::new(self.sample_rate, self.channels, S::FORMAT)
    }

    fn minimum_input(&self) -> usize {
        let frames = if self.is_streaming() && !self.finishing {
            QOA_FRAME_LENGTH
        } else {
            QOA_SLICE_LENGTH
        };

        frames * self.channels as usize
    }

    fn minimum_output(&self) -> usize {
        FRAME_HEADER_SIZE + CHANNEL_STATE_SIZE * self.channels as usize
    }

    fn flush(&mut self) {
        self.finishing = true;
    }

    fn encode(&mut self, input: &[S], output: &mut [u8]) -> Result<Progress> {
        if output.len() < self.minimum_output() {
            return Err(Error::BufferTooSmall);
        }

        let mut progress = Progress::default();

        while let Some(step) = self.step(
            &input[progress.consumed..],
            &mut output[progress.produced..],
        ) {
            progress.consumed += step.consumed;
            progress.produced += step.produced;
        }

        Ok(progress)
    }
}
