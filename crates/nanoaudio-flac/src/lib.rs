#![no_std]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

#[cfg(any(
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "aarch64",
    all(target_arch = "wasm32", target_feature = "simd128")
))]
mod simd;

use core::marker::PhantomData;

use nanoaudio_core::bits::BitReader;
use nanoaudio_core::codec::{Decoder, Progress, Seek, SeekRequest};
use nanoaudio_core::crc;
use nanoaudio_core::{Configuration, Error, Result, Sample};

/// Default largest block size, in samples per channel, that [`FlacDecoder`] accepts.
///
/// 4608 covers the blocks written by common encoders at every compression level.
pub const FLAC_DEFAULT_MAXIMUM_BLOCK_SIZE: usize = 4608;
/// Default largest channel count that [`FlacDecoder`] accepts.
pub const FLAC_DEFAULT_MAXIMUM_CHANNELS: usize = 2;
/// Seek points kept from a SEEKTABLE block; a larger table keeps a regular subset of its points.
pub const FLAC_SEEK_POINTS: usize = 32;

const MARKER: &[u8; 4] = b"fLaC";
const MARKER_SIZE: usize = 4;
const BLOCK_HEADER_SIZE: usize = 4;
const STREAM_INFORMATION_SIZE: usize = 34;
const STREAM_INFORMATION_TYPE: u8 = 0;
const SEEK_TABLE_TYPE: u8 = 3;
const SEEK_POINT_SIZE: usize = 18;
const PLACEHOLDER_SEEK_POINT: u64 = u64::MAX;
const UNKNOWN: u64 = u64::MAX;
const LINEAR_FRAMES: u64 = 2;
const MAXIMUM_SEEK_ROUNDS: u32 = 64;
const FORBIDDEN_BLOCK_TYPE: u8 = 127;
const MINIMUM_BLOCK_SIZE: usize = 16;
const MAXIMUM_STREAM_BLOCK_SIZE: usize = 65535;
const MINIMUM_BITS_PER_SAMPLE: u32 = 4;
const MAXIMUM_FRAME_HEADER_SIZE: usize = 16;
const FRAME_FOOTER_SIZE: usize = 2;
const MAXIMUM_ORDER: usize = 32;
const CHUNK: usize = 256;

const FIXED_COEFFICIENTS: [&[i32]; 5] = [&[], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]];

type Crc8 = crc::Crc8<0x07>;
type Crc16 = crc::Crc16<0x8005>;

/// The contents of a STREAMINFO metadata block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlacStreamInformation {
    /// Smallest block size in the stream, in samples per channel.
    pub minimum_block_size: u16,
    /// Largest block size in the stream, in samples per channel.
    pub maximum_block_size: u16,
    /// Smallest frame size in bytes, or 0 when unknown.
    pub minimum_frame_size: u32,
    /// Largest frame size in bytes, or 0 when unknown.
    pub maximum_frame_size: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of channels, from 1 to 8.
    pub channels: u8,
    /// Bits per sample, from 4 to 32.
    pub bits_per_sample: u8,
    /// Total number of frames (samples per channel), or 0 when unknown.
    pub total_samples: u64,
}

impl FlacStreamInformation {
    /// Parses the 34-byte body of a STREAMINFO block, without its block header.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Incomplete`] with fewer than 34 bytes, and [`Error::InvalidHeader`] when a field is out of range.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let bytes = bytes
            .get(..STREAM_INFORMATION_SIZE)
            .ok_or(Error::Incomplete)?;
        let packed = u64::from_be_bytes([
            bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17],
        ]);

        let information = Self {
            minimum_block_size: u16::from_be_bytes([bytes[0], bytes[1]]),
            maximum_block_size: u16::from_be_bytes([bytes[2], bytes[3]]),
            minimum_frame_size: u32::from_be_bytes([0, bytes[4], bytes[5], bytes[6]]),
            maximum_frame_size: u32::from_be_bytes([0, bytes[7], bytes[8], bytes[9]]),
            sample_rate: (packed >> 44) as u32,
            channels: ((packed >> 41) & 0x07) as u8 + 1,
            bits_per_sample: ((packed >> 36) & 0x1F) as u8 + 1,
            total_samples: packed & 0x0F_FFFF_FFFF,
        };

        if (information.bits_per_sample as u32) < MINIMUM_BITS_PER_SAMPLE
            || (information.minimum_block_size as usize) < MINIMUM_BLOCK_SIZE
            || information.maximum_block_size < information.minimum_block_size
        {
            return Err(Error::InvalidHeader);
        }

        Ok(information)
    }

    const fn maximum_frame_size_bound(&self) -> usize {
        if self.maximum_frame_size != 0 {
            return self.maximum_frame_size as usize;
        }

        let subframe_bits =
            8 + 33 + self.maximum_block_size as usize * (self.bits_per_sample as usize + 1);

        MAXIMUM_FRAME_HEADER_SIZE
            + (self.channels as usize * subframe_bits).div_ceil(8)
            + FRAME_FOOTER_SIZE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum State {
    Marker,
    BlockHeader,
    StreamInformation {
        last: bool,
    },
    Skip {
        remaining: usize,
        last: bool,
    },
    SeekTable {
        remaining: usize,
        index: usize,
        stride: usize,
        last: bool,
    },
    Frames,
    Search,
}

impl State {
    const fn after_block(last: bool) -> Self {
        if last {
            State::Frames
        } else {
            State::BlockHeader
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Assignment {
    Independent(usize),
    LeftSide,
    RightSide,
    MidSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameHeader {
    sample: u64,
    block_size: usize,
    assignment: Assignment,
    bits_per_sample: u32,
    size: usize,
}

impl FrameHeader {
    fn parse(bytes: &[u8], information: &FlacStreamInformation) -> Result<Self> {
        let byte = |index: usize| bytes.get(index).copied().ok_or(Error::Incomplete);

        if byte(0)? != 0xFF || byte(1)? & 0xFE != 0xF8 {
            return Err(Error::InvalidData);
        }

        let block_size_code = byte(2)? >> 4;
        let sample_rate_code = byte(2)? & 0x0F;
        let channel_code = byte(3)? >> 4;
        let bits_per_sample_code = (byte(3)? >> 1) & 0x07;

        if byte(3)? & 0x01 != 0 {
            return Err(Error::InvalidData);
        }

        let mut position = 5 + match byte(4)?.leading_ones() {
            0 => 0,
            length @ 2..=7 => length as usize - 1,
            _ => return Err(Error::InvalidData),
        };
        let mut number = u64::from(byte(4)? & (0x7F >> (position - 5 + usize::from(position > 5))));

        for index in 5..position {
            let byte = byte(index)?;

            if byte & 0xC0 != 0x80 {
                return Err(Error::InvalidData);
            }

            number = number << 6 | u64::from(byte & 0x3F);
        }

        let block_size = match block_size_code {
            0 => return Err(Error::InvalidData),
            1 => 192,
            2..=5 => 144 << block_size_code,
            6 => {
                position += 1;
                byte(position - 1)? as usize + 1
            }
            7 => {
                position += 2;
                u16::from_be_bytes([byte(position - 2)?, byte(position - 1)?]) as usize + 1
            }
            _ => 1 << block_size_code,
        };

        position += match sample_rate_code {
            12 => 1,
            13 | 14 => 2,
            15 => return Err(Error::InvalidData),
            _ => 0,
        };

        let bits_per_sample = match bits_per_sample_code {
            0 => information.bits_per_sample as u32,
            1 => 8,
            2 => 12,
            4 => 16,
            5 => 20,
            6 => 24,
            7 => 32,
            _ => return Err(Error::InvalidData),
        };

        let assignment = match channel_code {
            0..=7 => Assignment::Independent(channel_code as usize + 1),
            8 => Assignment::LeftSide,
            9 => Assignment::RightSide,
            10 => Assignment::MidSide,
            _ => return Err(Error::InvalidData),
        };

        if byte(position)? != Crc8::update(0, &bytes[..position]) {
            return Err(Error::InvalidData);
        }

        let channels = match assignment {
            Assignment::Independent(channels) => channels,
            _ => 2,
        };

        if channels != information.channels as usize
            || bits_per_sample != information.bits_per_sample as u32
        {
            return Err(Error::UnsupportedFormat);
        }

        if block_size > information.maximum_block_size as usize {
            return Err(Error::InvalidData);
        }

        let sample = if byte(1)? & 0x01 != 0 {
            number
        } else if information.minimum_block_size == information.maximum_block_size {
            number * u64::from(information.maximum_block_size)
        } else {
            number * block_size as u64
        };

        Ok(Self {
            sample,
            block_size,
            assignment,
            bits_per_sample,
            size: position + 1,
        })
    }
}

fn read_signed_wide(reader: &mut BitReader, count: u32) -> Result<i64> {
    if count <= 32 {
        return Ok(reader.read_signed(count)?.into());
    }

    let high = u64::from(reader.read(count - 32)?);
    let low = u64::from(reader.read(32)?);
    let shift = 64 - count;

    Ok((((high << 32) | low) << shift) as i64 >> shift)
}

fn read_rice(reader: &mut BitReader, parameter: u32) -> Result<i32> {
    let quotient = u64::from(reader.read_unary()?);
    let remainder = u64::from(reader.read(parameter)?);
    let folded =
        u32::try_from((quotient << parameter) | remainder).map_err(|_| Error::InvalidData)?;

    Ok((folded >> 1) as i32 ^ -((folded & 1) as i32))
}

struct SubframeHeader {
    kind: u32,
    wasted_bits: u32,
    bits_per_sample: u32,
}

impl SubframeHeader {
    fn read(reader: &mut BitReader, bits_per_sample: u32) -> Result<Self> {
        if reader.read(1)? != 0 {
            return Err(Error::InvalidData);
        }

        let kind = reader.read(6)?;
        let wasted_bits = if reader.read(1)? != 0 {
            reader.read_unary()?.saturating_add(1)
        } else {
            0
        };

        if wasted_bits >= bits_per_sample {
            return Err(Error::InvalidData);
        }

        Ok(Self {
            kind,
            wasted_bits,
            bits_per_sample: bits_per_sample - wasted_bits,
        })
    }
}

enum Predictor {
    Fixed(usize),
    Linear {
        coefficients: [i32; MAXIMUM_ORDER],
        order: usize,
        precision: u32,
        shift: u32,
    },
}

impl Predictor {
    fn order(&self) -> usize {
        match self {
            Predictor::Fixed(order) => *order,
            Predictor::Linear { order, .. } => *order,
        }
    }

    fn read_parameters(&mut self, reader: &mut BitReader) -> Result<()> {
        if let Predictor::Linear {
            coefficients,
            order,
            precision,
            shift,
        } = self
        {
            *precision = reader.read(4)? + 1;

            if *precision > 15 {
                return Err(Error::InvalidData);
            }

            *shift = u32::try_from(reader.read_signed(5)?).map_err(|_| Error::InvalidData)?;

            for coefficient in &mut coefficients[..*order] {
                *coefficient = reader.read_signed(*precision)?;
            }
        }

        Ok(())
    }

    fn coefficients(&self) -> (&[i32], u32) {
        match self {
            Predictor::Fixed(order) => (FIXED_COEFFICIENTS[*order], 0),
            Predictor::Linear {
                coefficients,
                order,
                shift,
                ..
            } => (&coefficients[..*order], *shift),
        }
    }
}

fn read_predictor(kind: u32) -> Result<Option<Predictor>> {
    match kind {
        0 | 1 => Ok(None),
        8..=12 => Ok(Some(Predictor::Fixed(kind as usize - 8))),
        32..=63 => Ok(Some(Predictor::Linear {
            coefficients: [0; MAXIMUM_ORDER],
            order: kind as usize - 31,
            precision: 0,
            shift: 0,
        })),
        _ => Err(Error::InvalidData),
    }
}

fn decode_residual(reader: &mut BitReader, samples: &mut [i32], order: usize) -> Result<()> {
    let parameter_bits = match reader.read(2)? {
        0 => 4,
        1 => 5,
        _ => return Err(Error::InvalidData),
    };
    let escape = (1 << parameter_bits) - 1;
    let partition_order = reader.read(4)?;
    let partition_size = samples.len() >> partition_order;

    if partition_size << partition_order != samples.len() || partition_size < order {
        return Err(Error::InvalidData);
    }

    let mut start = order;

    for partition in samples.chunks_exact_mut(partition_size) {
        let partition = &mut partition[start..];
        let parameter = reader.read(parameter_bits)?;

        if parameter == escape {
            let bits = reader.read(5)?;

            for sample in partition {
                *sample = reader.read_signed(bits)?;
            }
        } else {
            for sample in partition {
                *sample = read_rice(reader, parameter)?;
            }
        }

        start = 0;
    }

    Ok(())
}

fn predict_fixed(samples: &mut [i32], order: usize) {
    match order {
        1 => {
            for index in 1..samples.len() {
                samples[index] = samples[index].wrapping_add(samples[index - 1]);
            }
        }
        2 => {
            for index in 2..samples.len() {
                let prediction = samples[index - 1]
                    .wrapping_mul(2)
                    .wrapping_sub(samples[index - 2]);

                samples[index] = samples[index].wrapping_add(prediction);
            }
        }
        3 => {
            for index in 3..samples.len() {
                let prediction = samples[index - 1]
                    .wrapping_sub(samples[index - 2])
                    .wrapping_mul(3)
                    .wrapping_add(samples[index - 3]);

                samples[index] = samples[index].wrapping_add(prediction);
            }
        }
        4 => {
            for index in 4..samples.len() {
                let prediction = samples[index - 1]
                    .wrapping_add(samples[index - 3])
                    .wrapping_mul(4)
                    .wrapping_sub(samples[index - 2].wrapping_mul(6))
                    .wrapping_sub(samples[index - 4]);

                samples[index] = samples[index].wrapping_add(prediction);
            }
        }
        _ => {}
    }
}

fn predict_linear(
    samples: &mut [i32],
    coefficients: &[i32],
    precision: u32,
    shift: u32,
    bits_per_sample: u32,
) {
    let order = coefficients.len();
    let sum_bits = usize::BITS - (order - 1).leading_zeros();

    #[cfg(any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "aarch64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ))]
    if bits_per_sample + precision + sum_bits <= 32 {
        simd::predict_narrow(samples, coefficients, shift);
    } else {
        simd::predict_wide(samples, coefficients, shift);
    }

    #[cfg(not(any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "aarch64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    if bits_per_sample + precision + sum_bits <= 32 {
        predict_narrow(samples, coefficients, shift);
    } else {
        predict_wide(samples, coefficients, shift);
    }
}

fn predict_narrow(samples: &mut [i32], coefficients: &[i32], shift: u32) {
    let order = coefficients.len();

    for index in order..samples.len() {
        let prediction = coefficients
            .iter()
            .zip(samples[index - order..index].iter().rev())
            .fold(0i32, |sum, (coefficient, sample)| {
                sum.wrapping_add(coefficient.wrapping_mul(*sample))
            });

        samples[index] = samples[index].wrapping_add(prediction >> shift);
    }
}

fn predict_wide(samples: &mut [i32], coefficients: &[i32], shift: u32) {
    let order = coefficients.len();

    for index in order..samples.len() {
        let prediction = coefficients
            .iter()
            .zip(samples[index - order..index].iter().rev())
            .fold(0i64, |sum, (coefficient, sample)| {
                sum + i64::from(*coefficient) * i64::from(*sample)
            });

        samples[index] = (i64::from(samples[index]) + (prediction >> shift)) as i32;
    }
}

fn decode_subframe(
    reader: &mut BitReader,
    samples: &mut [i32],
    bits_per_sample: u32,
) -> Result<()> {
    let header = SubframeHeader::read(reader, bits_per_sample)?;
    let bits = header.bits_per_sample;

    match read_predictor(header.kind)? {
        None if header.kind == 0 => samples.fill(reader.read_signed(bits)?),
        None => {
            for sample in samples.iter_mut() {
                *sample = reader.read_signed(bits)?;
            }
        }
        Some(mut predictor) => {
            let order = predictor.order();

            if order > samples.len() {
                return Err(Error::InvalidData);
            }

            for sample in &mut samples[..order] {
                *sample = reader.read_signed(bits)?;
            }

            predictor.read_parameters(reader)?;
            decode_residual(reader, samples, order)?;

            match &predictor {
                Predictor::Fixed(order) => predict_fixed(samples, *order),
                Predictor::Linear {
                    coefficients,
                    order,
                    precision,
                    shift,
                } => predict_linear(samples, &coefficients[..*order], *precision, *shift, bits),
            }
        }
    }

    if header.wasted_bits > 0 {
        for sample in samples.iter_mut() {
            *sample <<= header.wasted_bits;
        }
    }

    Ok(())
}

struct Residual {
    escape: u32,
    parameter_bits: u32,
    partition_size: usize,
    remaining: usize,
    parameter: u32,
    escaped: bool,
}

impl Residual {
    fn new(reader: &mut BitReader, block_size: usize, order: usize) -> Result<Self> {
        let parameter_bits = match reader.read(2)? {
            0 => 4,
            1 => 5,
            _ => return Err(Error::InvalidData),
        };
        let partition_order = reader.read(4)?;
        let partition_size = block_size >> partition_order;

        if partition_size << partition_order != block_size || partition_size < order {
            return Err(Error::InvalidData);
        }

        let mut residual = Self {
            escape: (1 << parameter_bits) - 1,
            parameter_bits,
            partition_size,
            remaining: 0,
            parameter: 0,
            escaped: false,
        };

        residual.start(reader, partition_size - order)?;

        Ok(residual)
    }

    fn start(&mut self, reader: &mut BitReader, size: usize) -> Result<()> {
        let parameter = reader.read(self.parameter_bits)?;

        self.escaped = parameter == self.escape;
        self.parameter = if self.escaped {
            reader.read(5)?
        } else {
            parameter
        };
        self.remaining = size;

        Ok(())
    }

    fn read(&mut self, reader: &mut BitReader, mut samples: &mut [i32]) -> Result<()> {
        while !samples.is_empty() {
            if self.remaining == 0 {
                self.start(reader, self.partition_size)?;
            }

            let count = self.remaining.min(samples.len());
            let (head, tail) = core::mem::take(&mut samples).split_at_mut(count);

            if self.escaped {
                for sample in head {
                    *sample = reader.read_signed(self.parameter)?;
                }
            } else {
                for sample in head {
                    *sample = read_rice(reader, self.parameter)?;
                }
            }

            self.remaining -= count;
            samples = tail;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Channel(usize),
    Left,
    Right,
    Side,
}

fn store<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
    samples: &mut [[i16; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
    target: Target,
    start: usize,
    values: &[i32],
    wasted_bits: u32,
) -> Result<()> {
    let end = start + values.len();

    if let Target::Channel(channel) = target {
        for (sample, value) in samples[channel][start..end].iter_mut().zip(values) {
            *sample = (value << wasted_bits) as i16;
        }

        return Ok(());
    }

    let [first, second] = samples
        .get_disjoint_mut([0, 1])
        .map_err(|_| Error::UnsupportedFormat)?;
    let pairs = first[start..end].iter_mut().zip(&mut second[start..end]);

    match target {
        Target::Channel(_) => {}
        Target::Left => {
            for ((left, right), value) in pairs.zip(values) {
                *right = left.wrapping_sub((value << wasted_bits) as i16);
            }
        }
        Target::Right => {
            for ((side, right), value) in pairs.zip(values) {
                *right = (value << wasted_bits) as i16;
                *side = side.wrapping_add(*right);
            }
        }
        Target::Side => {
            for ((mid, side), value) in pairs.zip(values) {
                let difference = value << wasted_bits;
                let sum = (i32::from(*mid) << 1) | (difference & 1);

                *mid = (sum.wrapping_add(difference) >> 1) as i16;
                *side = (sum.wrapping_sub(difference) >> 1) as i16;
            }
        }
    }

    Ok(())
}

fn decode_narrow_subframe<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
    reader: &mut BitReader,
    samples: &mut [[i16; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
    block_size: usize,
    bits_per_sample: u32,
    target: Target,
) -> Result<()> {
    let header = SubframeHeader::read(reader, bits_per_sample)?;
    let bits = header.bits_per_sample;
    let wasted_bits = header.wasted_bits;
    let mut scratch = [0i32; MAXIMUM_ORDER + CHUNK];

    let Some(mut predictor) = read_predictor(header.kind)? else {
        let constant = if header.kind == 0 {
            let value = reader.read_signed(bits)?;

            scratch.fill(value);

            true
        } else {
            false
        };
        let mut start = 0;

        while start < block_size {
            let count = CHUNK.min(block_size - start);

            if !constant {
                for sample in &mut scratch[..count] {
                    *sample = reader.read_signed(bits)?;
                }
            }

            store(samples, target, start, &scratch[..count], wasted_bits)?;
            start += count;
        }

        return Ok(());
    };

    let order = predictor.order();

    if order > block_size {
        return Err(Error::InvalidData);
    }

    for sample in &mut scratch[..order] {
        *sample = reader.read_signed(bits)?;
    }

    predictor.read_parameters(reader)?;

    let mut residual = Residual::new(reader, block_size, order)?;
    let mut start = order;

    store(samples, target, 0, &scratch[..order], wasted_bits)?;

    while start < block_size {
        let count = CHUNK.min(block_size - start);
        let window = &mut scratch[..order + count];

        residual.read(reader, &mut window[order..])?;

        match &predictor {
            Predictor::Fixed(order) => predict_fixed(window, *order),
            Predictor::Linear {
                coefficients,
                order,
                precision,
                shift,
            } => predict_linear(window, &coefficients[..*order], *precision, *shift, bits),
        }

        store(samples, target, start, &window[order..], wasted_bits)?;
        scratch.copy_within(count..count + order, 0);
        start += count;
    }

    Ok(())
}

fn decode_narrow_frame<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
    reader: &mut BitReader,
    samples: &mut [[i16; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
    header: &FrameHeader,
) -> Result<()> {
    let block_size = header.block_size;
    let bits = header.bits_per_sample;
    let [first, second] = match header.assignment {
        Assignment::Independent(channels) => {
            for channel in 0..channels {
                decode_narrow_subframe(
                    reader,
                    samples,
                    block_size,
                    bits,
                    Target::Channel(channel),
                )?;
            }

            return Ok(());
        }
        Assignment::LeftSide => [(bits, Target::Channel(0)), (bits + 1, Target::Left)],
        Assignment::RightSide => [(bits + 1, Target::Channel(0)), (bits, Target::Right)],
        Assignment::MidSide => [(bits, Target::Channel(0)), (bits + 1, Target::Side)],
    };

    if MAXIMUM_CHANNELS < 2 {
        return Err(Error::UnsupportedFormat);
    }

    decode_narrow_subframe(reader, samples, block_size, first.0, first.1)?;
    decode_narrow_subframe(reader, samples, block_size, second.0, second.1)
}

fn decode_wide_subframe<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
    reader: &mut BitReader,
    samples: &mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
    block_size: usize,
    side: usize,
    mid_side: bool,
) -> Result<()> {
    let header = SubframeHeader::read(reader, 33)?;
    let bits = header.bits_per_sample;
    let wasted_bits = header.wasted_bits;
    let store =
        |samples: &mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS], index: usize, value: i64| {
            let value = value << wasted_bits;

            if mid_side {
                let sum = (i64::from(samples[0][index]) << 1) | (value & 1);

                samples[0][index] = (sum.wrapping_add(value) >> 1) as i32;
                samples[1][index] = (sum.wrapping_sub(value) >> 1) as i32;
            } else {
                samples[side][index] = value as i32;
            }
        };

    let Some(mut predictor) = read_predictor(header.kind)? else {
        if header.kind == 0 {
            let value = read_signed_wide(reader, bits)?;

            for index in 0..block_size {
                store(samples, index, value);
            }
        } else {
            for index in 0..block_size {
                let value = read_signed_wide(reader, bits)?;

                store(samples, index, value);
            }
        }

        return Ok(());
    };

    let order = predictor.order();

    if order > block_size {
        return Err(Error::InvalidData);
    }

    let mut history = [0i64; MAXIMUM_ORDER];

    for index in 0..order {
        history[order - 1 - index] = read_signed_wide(reader, bits)?;
    }

    predictor.read_parameters(reader)?;
    decode_residual(reader, &mut samples[side][..block_size], order)?;

    for index in 0..order {
        store(samples, index, history[order - 1 - index]);
    }

    let (coefficients, shift) = predictor.coefficients();

    for index in order..block_size {
        let prediction =
            coefficients
                .iter()
                .zip(&history)
                .fold(0i64, |sum, (coefficient, sample)| {
                    sum.wrapping_add(i64::from(*coefficient).wrapping_mul(*sample))
                });
        let value = i64::from(samples[side][index]).wrapping_add(prediction >> shift);

        if order > 0 {
            history.copy_within(..order - 1, 1);
            history[0] = value;
        }

        store(samples, index, value);
    }

    Ok(())
}

fn decode_channel<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
    reader: &mut BitReader,
    samples: &mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
    block_size: usize,
    channel: usize,
    bits_per_sample: u32,
    mid_side: bool,
) -> Result<bool> {
    if bits_per_sample > 32 {
        decode_wide_subframe(reader, samples, block_size, channel, mid_side)?;

        return Ok(mid_side);
    }

    decode_subframe(reader, &mut samples[channel][..block_size], bits_per_sample)?;

    Ok(false)
}

fn stereo<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
    samples: &mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
) -> Result<[&mut [i32; MAXIMUM_BLOCK_SIZE]; 2]> {
    samples
        .get_disjoint_mut([0, 1])
        .map_err(|_| Error::UnsupportedFormat)
}

fn decode_wide_frame<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
    reader: &mut BitReader,
    samples: &mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
    header: &FrameHeader,
) -> Result<()> {
    let block_size = header.block_size;
    let bits = header.bits_per_sample;

    match header.assignment {
        Assignment::Independent(channels) => {
            for channel in 0..channels {
                decode_channel(reader, samples, block_size, channel, bits, false)?;
            }
        }
        Assignment::LeftSide => {
            decode_channel(reader, samples, block_size, 0, bits, false)?;
            decode_channel(reader, samples, block_size, 1, bits + 1, false)?;

            let [left, side] = stereo(samples)?;

            for (side, left) in side[..block_size].iter_mut().zip(&left[..block_size]) {
                *side = left.wrapping_sub(*side);
            }
        }
        Assignment::RightSide => {
            decode_channel(reader, samples, block_size, 0, bits + 1, false)?;
            decode_channel(reader, samples, block_size, 1, bits, false)?;

            let [side, right] = stereo(samples)?;

            for (side, right) in side[..block_size].iter_mut().zip(&right[..block_size]) {
                *side = side.wrapping_add(*right);
            }
        }
        Assignment::MidSide => {
            decode_channel(reader, samples, block_size, 0, bits, false)?;

            if !decode_channel(reader, samples, block_size, 1, bits + 1, true)? {
                let [mid, side] = stereo(samples)?;

                for (mid, side) in mid[..block_size].iter_mut().zip(&mut side[..block_size]) {
                    let sum = (i64::from(*mid) << 1) | i64::from(*side & 1);
                    let difference = i64::from(*side);

                    *mid = ((sum + difference) >> 1) as i32;
                    *side = ((sum - difference) >> 1) as i32;
                }
            }
        }
    }

    Ok(())
}

mod sealed {
    pub trait Sealed: Copy {
        const ZERO: Self;
        const BITS: u32;

        fn widen(self) -> i32;

        fn wide<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
            samples: &mut [[Self; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
        ) -> Option<&mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS]>;

        fn narrow<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
            samples: &mut [[Self; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
        ) -> Option<&mut [[i16; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS]>;
    }

    impl Sealed for i32 {
        const ZERO: Self = 0;
        const BITS: u32 = 32;

        #[inline(always)]
        fn widen(self) -> i32 {
            self
        }

        fn wide<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
            samples: &mut [[Self; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
        ) -> Option<&mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS]> {
            Some(samples)
        }

        fn narrow<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
            _: &mut [[Self; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
        ) -> Option<&mut [[i16; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS]> {
            None
        }
    }

    impl Sealed for i16 {
        const ZERO: Self = 0;
        const BITS: u32 = 16;

        #[inline(always)]
        fn widen(self) -> i32 {
            self.into()
        }

        fn wide<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
            _: &mut [[Self; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
        ) -> Option<&mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS]> {
            None
        }

        fn narrow<const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>(
            samples: &mut [[Self; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
        ) -> Option<&mut [[i16; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS]> {
            Some(samples)
        }
    }
}

/// The integer type [`FlacDecoder`] keeps a decoded block in.
///
/// `i32`, the default, decodes every stream. `i16` halves the block buffer and decodes streams of up to 16 bits per sample; deeper streams are rejected with [`Error::UnsupportedFormat`].
pub trait FlacBlockSample: sealed::Sealed {}

impl FlacBlockSample for i32 {}

impl FlacBlockSample for i16 {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Point {
    byte: u64,
    sample: u64,
}

impl Point {
    const ZERO: Self = Self { byte: 0, sample: 0 };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Search {
    target: u64,
    low: Point,
    high: Point,
    probe: u64,
    bisect: bool,
    stretch: u32,
    rounds: u32,
}

/// A FLAC decoder that does not allocate.
///
/// `MAXIMUM_BLOCK_SIZE` (in samples per channel) and `MAXIMUM_CHANNELS` size the block buffer, which holds one `B` per sample: `size_of` grows with their product, about 37 KiB with the defaults (4608 samples, 2 channels, `i32`) and 33 KiB for 4096-sample stereo blocks. A stream whose STREAMINFO announces larger blocks is rejected with [`Error::UnsupportedFormat`], and one with more channels with [`Error::TooManyChannels`].
///
/// With `B = i16` the buffer is half as large (about 18 KiB with the defaults) and decodes streams of up to 16 bits per sample, which covers most files; deeper streams are rejected with [`Error::UnsupportedFormat`]. Decoding a subframe then also uses about 1 KiB of stack.
///
/// ```
/// use nanoaudio_flac::FlacDecoder;
///
/// let decoder = FlacDecoder::<i16, 4608, 2, i16>::new();
///
/// assert!(size_of_val(&decoder) < 20 * 1024);
/// ```
///
/// Streams of 4 to 32 bits per sample decode to any [`Sample`] type, scaled to its full range: a narrower type is rounded (24-bit audio to `i16`), a wider one is shifted up, and `f32` stays within -1.0 to 1.0. Use `i32` to keep every bit of any stream.
///
/// Seeking goes through [`Seek`] once the metadata is read. After a request, `decode` can return [`Error::SeekTo`] a few times while the search narrows: drop buffered input and resume from that offset each time.
///
/// ```
/// use nanoaudio_core::Error;
/// use nanoaudio_core::codec::{Decoder, Seek};
/// use nanoaudio_flac::FlacDecoder;
///
/// fn decode_from(file: &[u8], frame: u64, output: &mut [i16]) -> Result<usize, Error> {
///     let mut decoder = FlacDecoder::<i16>::new();
///
///     decoder.set_stream_length(file.len() as u64);
///     decoder.decode(file, output)?;
///
///     let mut offset = decoder.seek(frame)?.byte_offset as usize;
///     let progress = loop {
///         decoder.finish();
///         match decoder.decode(&file[offset..], output) {
///             Err(Error::SeekTo(next)) => offset = next as usize,
///             result => break result?,
///         }
///     };
///
///     Ok(progress.produced)
/// }
/// ```
#[derive(Clone)]
pub struct FlacDecoder<
    S: Sample = i16,
    const MAXIMUM_BLOCK_SIZE: usize = FLAC_DEFAULT_MAXIMUM_BLOCK_SIZE,
    const MAXIMUM_CHANNELS: usize = FLAC_DEFAULT_MAXIMUM_CHANNELS,
    B: FlacBlockSample = i32,
> {
    state: State,
    information: Option<FlacStreamInformation>,
    samples: [[B; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
    block_size: usize,
    position: usize,
    required: usize,
    finishing: bool,
    offset: u64,
    audio_offset: u64,
    length: Option<u64>,
    last: Option<Point>,
    skip: u64,
    search: Search,
    seek_points: [Point; FLAC_SEEK_POINTS],
    seek_point_count: usize,
    sample: PhantomData<S>,
}

impl<S: Sample, const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize, B: FlacBlockSample>
    Default for FlacDecoder<S, MAXIMUM_BLOCK_SIZE, MAXIMUM_CHANNELS, B>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Sample, const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize, B: FlacBlockSample>
    FlacDecoder<S, MAXIMUM_BLOCK_SIZE, MAXIMUM_CHANNELS, B>
{
    /// Creates a decoder waiting for the `fLaC` marker.
    ///
    /// Fails to compile unless `MAXIMUM_BLOCK_SIZE` is between 16 and 65535 and `MAXIMUM_CHANNELS` between 1 and 8.
    pub const fn new() -> Self {
        const {
            assert!(
                MAXIMUM_BLOCK_SIZE >= MINIMUM_BLOCK_SIZE
                    && MAXIMUM_BLOCK_SIZE <= MAXIMUM_STREAM_BLOCK_SIZE,
                "the maximum block size must be between 16 and 65535 samples"
            );
            assert!(
                MAXIMUM_CHANNELS >= 1 && MAXIMUM_CHANNELS <= nanoaudio_core::MAXIMUM_CHANNELS,
                "the maximum channel count must be between 1 and 8"
            );
        }

        Self {
            state: State::Marker,
            information: None,
            samples: [[B::ZERO; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
            block_size: 0,
            position: 0,
            required: 0,
            finishing: false,
            offset: 0,
            audio_offset: 0,
            length: None,
            last: None,
            skip: 0,
            search: Search {
                target: 0,
                low: Point::ZERO,
                high: Point::ZERO,
                probe: 0,
                bisect: false,
                stretch: 0,
                rounds: 0,
            },
            seek_points: [Point::ZERO; FLAC_SEEK_POINTS],
            seek_point_count: 0,
            sample: PhantomData,
        }
    }

    /// Returns to the state of [`new`](Self::new), to decode another stream from its `fLaC` marker.
    pub fn reset(&mut self) {
        self.state = State::Marker;
        self.information = None;
        self.block_size = 0;
        self.position = 0;
        self.required = 0;
        self.finishing = false;
        self.offset = 0;
        self.audio_offset = 0;
        self.length = None;
        self.last = None;
        self.skip = 0;
        self.seek_point_count = 0;
    }

    /// Gives the stream length in bytes, counted from `fLaC`, so that seeking can interpolate from the first request.
    pub fn set_stream_length(&mut self, length: u64) {
        self.length = Some(length);
    }

    /// Returns the STREAMINFO block, once the decoder has read it.
    pub const fn stream_information(&self) -> Option<FlacStreamInformation> {
        self.information
    }

    fn read_metadata(&mut self, input: &[u8]) -> Result<usize> {
        match self.state {
            State::Marker => {
                let marker = input.get(..MARKER_SIZE).ok_or(Error::Incomplete)?;

                if marker != MARKER {
                    return Err(Error::InvalidHeader);
                }

                self.state = State::BlockHeader;

                Ok(MARKER_SIZE)
            }
            State::BlockHeader => {
                let header = input.get(..BLOCK_HEADER_SIZE).ok_or(Error::Incomplete)?;
                let last = header[0] & 0x80 != 0;
                let kind = header[0] & 0x7F;
                let length = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;

                self.state = match (self.information.is_some(), kind) {
                    (_, FORBIDDEN_BLOCK_TYPE) => return Err(Error::InvalidHeader),
                    (false, STREAM_INFORMATION_TYPE) if length == STREAM_INFORMATION_SIZE => {
                        State::StreamInformation { last }
                    }
                    (false, _) | (true, STREAM_INFORMATION_TYPE) => {
                        return Err(Error::InvalidHeader);
                    }
                    (true, _) if length == 0 => State::after_block(last),
                    (true, SEEK_TABLE_TYPE)
                        if self.seek_point_count == 0 && length.is_multiple_of(SEEK_POINT_SIZE) =>
                    {
                        let points = length / SEEK_POINT_SIZE;

                        State::SeekTable {
                            remaining: points,
                            index: 0,
                            stride: points.div_ceil(FLAC_SEEK_POINTS),
                            last,
                        }
                    }
                    (true, _) => State::Skip {
                        remaining: length,
                        last,
                    },
                };

                Ok(BLOCK_HEADER_SIZE)
            }
            State::StreamInformation { last } => {
                let information = FlacStreamInformation::parse(input)?;

                if information.channels as usize > MAXIMUM_CHANNELS {
                    return Err(Error::TooManyChannels);
                }

                if information.maximum_block_size as usize > MAXIMUM_BLOCK_SIZE
                    || information.bits_per_sample as u32 > B::BITS
                    || information.sample_rate == 0
                {
                    return Err(Error::UnsupportedFormat);
                }

                self.information = Some(information);
                self.state = State::after_block(last);

                Ok(STREAM_INFORMATION_SIZE)
            }
            State::Skip { remaining, last } => {
                if input.is_empty() {
                    return Err(Error::Incomplete);
                }

                let skipped = remaining.min(input.len());

                self.state = match remaining - skipped {
                    0 => State::after_block(last),
                    remaining => State::Skip { remaining, last },
                };

                Ok(skipped)
            }
            State::SeekTable {
                remaining,
                index,
                stride,
                last,
            } => {
                let point = input
                    .first_chunk::<SEEK_POINT_SIZE>()
                    .ok_or(Error::Incomplete)?;
                let (sample, rest) = point.split_first_chunk::<8>().ok_or(Error::Incomplete)?;
                let (offset, _) = rest.split_first_chunk::<8>().ok_or(Error::Incomplete)?;
                let sample = u64::from_be_bytes(*sample);

                if sample != PLACEHOLDER_SEEK_POINT
                    && index.is_multiple_of(stride)
                    && self.seek_point_count < FLAC_SEEK_POINTS
                {
                    self.seek_points[self.seek_point_count] = Point {
                        byte: u64::from_be_bytes(*offset),
                        sample,
                    };
                    self.seek_point_count += 1;
                }

                self.state = match remaining - 1 {
                    0 => State::after_block(last),
                    remaining => State::SeekTable {
                        remaining,
                        index: index + 1,
                        stride,
                        last,
                    },
                };

                Ok(SEEK_POINT_SIZE)
            }
            State::Frames | State::Search => Err(Error::InvalidParameter),
        }
    }

    fn decode_frame(&mut self, input: &[u8]) -> Result<usize> {
        let information = self.information.ok_or(Error::InvalidHeader)?;
        let header = FrameHeader::parse(input, &information)?;
        let mut reader = BitReader::new(&input[header.size..]);

        if let Some(samples) = B::wide(&mut self.samples) {
            decode_wide_frame(&mut reader, samples, &header)?;
        } else if let Some(samples) = B::narrow(&mut self.samples) {
            decode_narrow_frame(&mut reader, samples, &header)?;
        }

        reader.align();

        let footer = header.size + reader.byte_position();
        let crc = input
            .get(footer..footer + FRAME_FOOTER_SIZE)
            .ok_or(Error::Incomplete)?;

        if u16::from_be_bytes([crc[0], crc[1]]) != Crc16::update(0, &input[..footer]) {
            return Err(Error::InvalidData);
        }

        self.block_size = header.block_size;
        self.position = 0;
        self.last = Some(Point {
            byte: self.offset,
            sample: header.sample,
        });

        Ok(footer + FRAME_FOOTER_SIZE)
    }

    fn apply_skip(&mut self) {
        let skipped = self.skip.min(self.block_size as u64);

        self.position = skipped as usize;
        self.skip -= skipped;
    }

    fn frame_length(
        input: &[u8],
        header: &FrameHeader,
        information: &FlacStreamInformation,
    ) -> Option<usize> {
        let end = header.sample + header.block_size as u64;
        let limit = input.len().min(information.maximum_frame_size_bound() + 1);
        let mut start = header.size + FRAME_FOOTER_SIZE;

        while let Some(found) = input
            .get(start..limit)?
            .iter()
            .position(|byte| *byte == 0xFF)
        {
            let position = start + found;

            start = position + 1;

            if *input.get(position + 1)? != input[1] {
                continue;
            }

            match FrameHeader::parse(&input[position..], information) {
                Ok(next) if next.sample == end => return Some(position),
                Err(Error::Incomplete) => return None,
                _ => {}
            }
        }

        None
    }

    fn boundary(&self, input: &[u8], header: &FrameHeader) -> Result<Option<usize>> {
        let Some(information) = self.information else {
            return Ok(None);
        };

        match Self::frame_length(input, header, &information) {
            Some(length) => Ok(Some(length)),
            None if !self.finishing && input.len() < information.maximum_frame_size_bound() => {
                Err(Error::Incomplete)
            }
            None => Ok(None),
        }
    }

    fn header(&self, input: &[u8]) -> Option<FrameHeader> {
        FrameHeader::parse(input, self.information.as_ref()?).ok()
    }

    fn skip_frame(&mut self, input: &[u8]) -> Result<Option<usize>> {
        let Some(header) = self
            .header(input)
            .filter(|header| header.block_size as u64 <= self.skip)
        else {
            return Ok(None);
        };
        let Some(length) = self.boundary(input, &header)? else {
            return Ok(None);
        };

        self.skip -= header.block_size as u64;
        self.block_size = 0;
        self.position = 0;
        self.last = Some(Point {
            byte: self.offset,
            sample: header.sample,
        });

        Ok(Some(length))
    }

    fn next_frame(&mut self, input: &[u8]) -> Result<usize> {
        if self.skip != 0
            && let Some(consumed) = self.skip_frame(input)?
        {
            return Ok(consumed);
        }

        let consumed = self.decode_frame(input)?;

        if self.skip != 0 {
            self.apply_skip();
        }

        Ok(consumed)
    }

    fn search_step(&mut self, input: &[u8]) -> Result<usize> {
        match input
            .windows(2)
            .position(|pair| pair[0] == 0xFF && pair[1] & 0xFE == 0xF8)
        {
            Some(0) => {}
            Some(start) => return Ok(start),
            None if self.finishing => return Ok(input.len()),
            None => {
                let kept = usize::from(input.last() == Some(&0xFF));

                return match input.len() - kept {
                    0 => Err(Error::Incomplete),
                    skipped => Ok(skipped),
                };
            }
        }

        if let Some(header) = self.header(input)
            && self.boundary(input, &header)?.is_some()
        {
            let found = Point {
                byte: self.offset,
                sample: header.sample,
            };

            self.last = Some(found);
            self.land(found)?;

            return Ok(0);
        }

        let consumed = match self.decode_frame(input) {
            Ok(consumed) => consumed,
            Err(Error::Incomplete) if !self.finishing => return Err(Error::Incomplete),
            Err(Error::Incomplete | Error::InvalidData | Error::UnsupportedFormat) => {
                self.block_size = 0;

                return Ok(1);
            }
            Err(error) => return Err(error),
        };
        let found = Point {
            byte: self.offset,
            sample: self.last.map_or(0, |last| last.sample),
        };

        self.land(found)?;
        self.apply_skip();

        Ok(consumed)
    }

    fn land(&mut self, found: Point) -> Result<()> {
        let search = self.search;

        if found.sample > search.target {
            let high = Point {
                byte: search.probe,
                sample: found.sample,
            };

            return Err(self.narrow(search.low, high));
        }

        if !self.is_close(found) {
            return Err(self.narrow(found, search.high));
        }

        self.state = State::Frames;
        self.skip = search.target - found.sample;

        Ok(())
    }

    fn end_of_search(&mut self) -> Error {
        let high = Point {
            byte: self.search.probe,
            sample: UNKNOWN,
        };

        self.narrow(self.search.low, high)
    }

    fn narrow(&mut self, low: Point, high: Point) -> Error {
        let before = self.search.high.byte.saturating_sub(self.search.low.byte);

        self.search.low = low;
        self.search.high = high;
        self.search.bisect = high.byte != UNKNOWN && high.byte - low.byte > before / 2;
        self.search.rounds += 1;

        if self.search.rounds > MAXIMUM_SEEK_ROUNDS {
            self.state = State::Frames;
            self.block_size = 0;
            self.position = 0;

            return Error::InvalidData;
        }

        Error::SeekTo(self.start_search())
    }

    fn start_search(&mut self) -> u64 {
        let probe = self.probe();

        self.search.probe = probe;
        self.state = State::Search;
        self.offset = probe;
        self.block_size = 0;
        self.position = 0;
        self.skip = 0;
        self.required = 0;
        self.finishing = false;

        probe
    }

    fn is_close(&self, point: Point) -> bool {
        let Some(information) = self.information else {
            return true;
        };

        self.search.target - point.sample <= LINEAR_FRAMES * information.maximum_block_size as u64
            || self.search.high.byte.saturating_sub(point.byte)
                <= LINEAR_FRAMES * information.maximum_frame_size_bound() as u64
    }

    fn probe(&mut self) -> u64 {
        let search = self.search;
        let low = search.low;
        let high = search.high;

        let Some(information) = self.information.filter(|_| !self.is_close(low)) else {
            return low.byte;
        };
        let aim = search
            .target
            .saturating_sub(information.maximum_block_size as u64)
            .max(low.sample);
        let distance = u128::from(aim - low.sample);
        let end = match high.sample {
            UNKNOWN if information.total_samples > low.sample => information.total_samples,
            sample => sample,
        };
        let probe = if high.byte == UNKNOWN {
            let frame = information.maximum_frame_size_bound() as u64;
            let known = match self.last {
                Some(last) if last.sample > low.sample => last,
                _ => low,
            };
            let (bytes, samples) = match known.sample {
                0 => (frame, information.maximum_block_size as u64),
                samples => (known.byte - self.audio_offset, samples),
            };
            let estimate = distance.saturating_mul(u128::from(bytes)) / u128::from(samples);
            let stretch = search.stretch.min(32);
            let reach = u128::from(low.byte - self.audio_offset + frame) << (stretch + 1);

            self.search.stretch += 1;

            u128::from(low.byte).saturating_add((estimate << stretch).min(reach))
        } else if search.bisect || end == UNKNOWN {
            u128::from(low.byte + (high.byte - low.byte) / 2)
        } else {
            u128::from(low.byte)
                + u128::from(high.byte - low.byte).saturating_mul(distance)
                    / u128::from(end - low.sample)
        };

        (probe.min(u128::from(UNKNOWN)) as u64)
            .clamp(low.byte + 1, high.byte.saturating_sub(1).max(low.byte + 1))
    }

    fn drain(&mut self, output: &mut [S]) -> usize {
        let Some(information) = self.information else {
            return 0;
        };
        let channels = information.channels as usize;
        let shift = 32 - information.bits_per_sample as u32;
        let frames = (self.block_size - self.position).min(output.len() / channels);

        for (frame, output) in output.chunks_exact_mut(channels).take(frames).enumerate() {
            for (channel, output) in output.iter_mut().enumerate() {
                *output =
                    S::from_i32(self.samples[channel][self.position + frame].widen() << shift);
            }
        }

        self.position += frames;

        frames * channels
    }
}

impl<S: Sample, const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize, B: FlacBlockSample>
    Decoder for FlacDecoder<S, MAXIMUM_BLOCK_SIZE, MAXIMUM_CHANNELS, B>
{
    type Sample = S;

    fn configuration(&self) -> Option<Configuration> {
        self.information.map(|information| {
            Configuration::new(information.sample_rate, information.channels, S::FORMAT)
        })
    }

    /// Returns the input needed to make progress, in bytes.
    ///
    /// Once the metadata is read, this is STREAMINFO's maximum frame size, or a worst-case bound from the block size, channels and bit depth when the stream does not give it. A smaller input works while each frame fits in it; when one does not, `decode` consumes nothing until the caller passes more input or calls `finish`.
    fn minimum_input(&self) -> usize {
        match (self.state, self.information) {
            (State::Marker, _) => MARKER_SIZE,
            (State::BlockHeader, _) => BLOCK_HEADER_SIZE,
            (State::StreamInformation { .. }, _) => STREAM_INFORMATION_SIZE,
            (State::SeekTable { .. }, _) => SEEK_POINT_SIZE,
            (State::Skip { .. }, _) | (State::Frames | State::Search, None) => 1,
            (State::Frames | State::Search, Some(information)) => {
                information.maximum_frame_size_bound()
            }
        }
    }

    fn minimum_output(&self) -> usize {
        self.information.map_or(MAXIMUM_CHANNELS, |information| {
            information.channels as usize
        })
    }

    fn finish(&mut self) {
        self.finishing = true;
    }

    /// Decodes metadata and frames from `input` into interleaved samples in `output`.
    ///
    /// A frame whose CRC does not match returns [`Error::InvalidData`] without being consumed. After a seek, `decode` can return [`Error::SeekTo`] until it reaches the requested frame.
    fn decode(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        if output.len() < self.minimum_output() {
            return Err(Error::BufferTooSmall);
        }

        let mut progress = Progress::default();

        loop {
            if self.position < self.block_size {
                progress.produced += self.drain(&mut output[progress.produced..]);

                if self.position < self.block_size {
                    return Ok(progress);
                }
            }

            let remaining = &input[progress.consumed..];
            let frames = matches!(self.state, State::Frames | State::Search);
            let result = match self.state {
                State::Search if remaining.is_empty() && self.finishing => {
                    return Err(self.end_of_search());
                }
                State::Frames | State::Search
                    if remaining.is_empty()
                        || (!self.finishing && remaining.len() < self.required) =>
                {
                    return Ok(progress);
                }
                State::Frames => self.next_frame(remaining),
                State::Search => self.search_step(remaining),
                _ => self.read_metadata(remaining),
            };

            match result {
                Ok(consumed) => {
                    progress.consumed += consumed;
                    self.offset += consumed as u64;
                    self.required = 0;

                    if !frames && self.state == State::Frames {
                        self.audio_offset = self.offset;
                    }
                }
                Err(error @ Error::SeekTo(_)) => return Err(error),
                Err(Error::Incomplete) if !self.finishing || remaining.is_empty() => {
                    if frames {
                        self.required = remaining
                            .len()
                            .saturating_mul(2)
                            .min(self.minimum_input())
                            .max(remaining.len() + 1);
                    }

                    return Ok(progress);
                }
                Err(_) if !progress.is_stalled() => return Ok(progress),
                Err(error) => return Err(error),
            }
        }
    }
}

impl<S: Sample, const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize, B: FlacBlockSample>
    Seek for FlacDecoder<S, MAXIMUM_BLOCK_SIZE, MAXIMUM_CHANNELS, B>
{
    /// Starts a search for `frame`, from the seek table when there is one, and returns the byte offset to resume from, counted from `fLaC`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Incomplete`] before the metadata is read, unless `frame` is 0, and [`Error::InvalidParameter`] past the end of the stream.
    fn seek(&mut self, frame: u64) -> Result<SeekRequest> {
        let information = match (self.state, self.information) {
            (State::Frames | State::Search, Some(information)) => information,
            _ if frame == 0 => {
                let length = self.length;

                self.reset();
                self.length = length;

                return Ok(SeekRequest::default());
            }
            _ => return Err(Error::Incomplete),
        };

        if information.total_samples != 0 && frame > information.total_samples {
            return Err(Error::InvalidParameter);
        }

        let mut low = Point {
            byte: self.audio_offset,
            sample: 0,
        };
        let mut high = Point {
            byte: self.length.unwrap_or(UNKNOWN),
            sample: UNKNOWN,
        };
        let points = self.seek_points[..self.seek_point_count]
            .iter()
            .map(|point| Point {
                byte: self.audio_offset.saturating_add(point.byte),
                sample: point.sample,
            });

        for point in points.chain(self.last) {
            if point.sample <= frame {
                if point.sample >= low.sample {
                    low = point;
                }
            } else if point.byte < high.byte {
                high = point;
            }
        }

        self.search = Search {
            target: frame,
            low,
            high,
            probe: low.byte,
            bisect: false,
            stretch: 0,
            rounds: 0,
        };

        let byte_offset = self.start_search();
        let skip_frames = if byte_offset == low.byte {
            u32::try_from(frame - low.sample).unwrap_or(u32::MAX)
        } else {
            0
        };

        Ok(SeekRequest {
            byte_offset,
            skip_frames,
        })
    }
}
