use core::marker::PhantomData;

use crate::{Configuration, Error, Result, Sample};

use super::{Decoder, Progress};

pub const FLAC_DEFAULT_MAXIMUM_BLOCK_SIZE: usize = 4608;
pub const FLAC_DEFAULT_MAXIMUM_CHANNELS: usize = 2;

const MARKER: &[u8; 4] = b"fLaC";
const MARKER_SIZE: usize = 4;
const BLOCK_HEADER_SIZE: usize = 4;
const STREAM_INFORMATION_SIZE: usize = 34;
const STREAM_INFORMATION_TYPE: u8 = 0;
const FORBIDDEN_BLOCK_TYPE: u8 = 127;
const MINIMUM_BLOCK_SIZE: usize = 16;
const MAXIMUM_STREAM_BLOCK_SIZE: usize = 65535;
const MINIMUM_BITS_PER_SAMPLE: u32 = 4;
const MAXIMUM_FRAME_HEADER_SIZE: usize = 16;
const FRAME_FOOTER_SIZE: usize = 2;
const MAXIMUM_ORDER: usize = 32;

const FIXED_COEFFICIENTS: [&[i32]; 5] = [&[], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]];

const CRC_8_TABLE: [u8; 256] = crc_8_table();
const CRC_16_TABLE: [u16; 256] = crc_16_table();

const fn crc_8_table() -> [u8; 256] {
    let mut table = [0; 256];
    let mut index = 0;

    while index < 256 {
        let mut value = index as u8;
        let mut bit = 0;

        while bit < 8 {
            value = if value & 0x80 != 0 {
                (value << 1) ^ 0x07
            } else {
                value << 1
            };
            bit += 1;
        }

        table[index] = value;
        index += 1;
    }

    table
}

const fn crc_16_table() -> [u16; 256] {
    let mut table = [0; 256];
    let mut index = 0;

    while index < 256 {
        let mut value = (index as u16) << 8;
        let mut bit = 0;

        while bit < 8 {
            value = if value & 0x8000 != 0 {
                (value << 1) ^ 0x8005
            } else {
                value << 1
            };
            bit += 1;
        }

        table[index] = value;
        index += 1;
    }

    table
}

fn crc_8(bytes: &[u8]) -> u8 {
    bytes
        .iter()
        .fold(0, |crc, byte| CRC_8_TABLE[(crc ^ byte) as usize])
}

fn crc_16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0, |crc, byte| {
        (crc << 8) ^ CRC_16_TABLE[((crc >> 8) as u8 ^ byte) as usize]
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlacStreamInformation {
    pub minimum_block_size: u16,
    pub maximum_block_size: u16,
    pub minimum_frame_size: u32,
    pub maximum_frame_size: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub total_samples: u64,
}

impl FlacStreamInformation {
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
    StreamInformation { last: bool },
    Skip { remaining: usize, last: bool },
    Frames,
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

        for index in 5..position {
            if byte(index)? & 0xC0 != 0x80 {
                return Err(Error::InvalidData);
            }
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

        if byte(position)? != crc_8(&bytes[..position]) {
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

        Ok(Self {
            block_size,
            assignment,
            bits_per_sample,
            size: position + 1,
        })
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    position: usize,
    cache: u64,
    available: u32,
}

impl<'a> BitReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            cache: 0,
            available: 0,
        }
    }

    fn refill(&mut self) {
        while self.available <= 56 {
            let Some(&byte) = self.bytes.get(self.position) else {
                break;
            };

            self.cache |= u64::from(byte) << (56 - self.available);
            self.available += 8;
            self.position += 1;
        }
    }

    fn read(&mut self, count: u32) -> Result<u32> {
        if count == 0 {
            return Ok(0);
        }

        if self.available < count {
            self.refill();

            if self.available < count {
                return Err(Error::Incomplete);
            }
        }

        let value = (self.cache >> (64 - count)) as u32;
        self.cache <<= count;
        self.available -= count;

        Ok(value)
    }

    fn read_signed(&mut self, count: u32) -> Result<i32> {
        if count == 0 {
            return Ok(0);
        }

        let shift = 32 - count;

        Ok(((self.read(count)? << shift) as i32) >> shift)
    }

    fn read_signed_wide(&mut self, count: u32) -> Result<i64> {
        if count <= 32 {
            return Ok(self.read_signed(count)?.into());
        }

        let high = u64::from(self.read(count - 32)?);
        let low = u64::from(self.read(32)?);
        let shift = 64 - count;

        Ok((((high << 32) | low) << shift) as i64 >> shift)
    }

    fn read_unary(&mut self) -> Result<u32> {
        let mut zeros = 0u32;

        loop {
            if self.available == 0 {
                self.refill();

                if self.available == 0 {
                    return Err(Error::Incomplete);
                }
            }

            let leading = self.cache.leading_zeros();

            if leading < self.available {
                let consumed = leading + 1;

                self.cache = self.cache.checked_shl(consumed).unwrap_or(0);
                self.available -= consumed;

                return Ok(zeros.saturating_add(leading));
            }

            zeros = zeros.saturating_add(self.available);
            self.cache = 0;
            self.available = 0;
        }
    }

    fn read_rice(&mut self, parameter: u32) -> Result<i32> {
        let quotient = u64::from(self.read_unary()?);
        let remainder = u64::from(self.read(parameter)?);
        let folded =
            u32::try_from((quotient << parameter) | remainder).map_err(|_| Error::InvalidData)?;

        Ok((folded >> 1) as i32 ^ -((folded & 1) as i32))
    }

    fn align(&mut self) {
        let padding = self.available % 8;

        self.cache <<= padding;
        self.available -= padding;
    }

    fn byte_position(&self) -> usize {
        self.position - (self.available / 8) as usize
    }
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
                *sample = reader.read_rice(parameter)?;
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

    if bits_per_sample + precision + sum_bits <= 32 {
        for index in order..samples.len() {
            let prediction = coefficients
                .iter()
                .zip(samples[index - order..index].iter().rev())
                .fold(0i32, |sum, (coefficient, sample)| {
                    sum.wrapping_add(coefficient.wrapping_mul(*sample))
                });

            samples[index] = samples[index].wrapping_add(prediction >> shift);
        }
    } else {
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

#[derive(Clone)]
pub struct FlacDecoder<
    S: Sample = i16,
    const MAXIMUM_BLOCK_SIZE: usize = FLAC_DEFAULT_MAXIMUM_BLOCK_SIZE,
    const MAXIMUM_CHANNELS: usize = FLAC_DEFAULT_MAXIMUM_CHANNELS,
> {
    state: State,
    information: Option<FlacStreamInformation>,
    samples: [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
    block_size: usize,
    position: usize,
    required: usize,
    finishing: bool,
    sample: PhantomData<S>,
}

impl<S: Sample, const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize> Default
    for FlacDecoder<S, MAXIMUM_BLOCK_SIZE, MAXIMUM_CHANNELS>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Sample, const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize>
    FlacDecoder<S, MAXIMUM_BLOCK_SIZE, MAXIMUM_CHANNELS>
{
    pub const fn new() -> Self {
        const {
            assert!(
                MAXIMUM_BLOCK_SIZE >= MINIMUM_BLOCK_SIZE
                    && MAXIMUM_BLOCK_SIZE <= MAXIMUM_STREAM_BLOCK_SIZE,
                "the maximum block size must be between 16 and 65535 samples"
            );
            assert!(
                MAXIMUM_CHANNELS >= 1 && MAXIMUM_CHANNELS <= crate::MAXIMUM_CHANNELS,
                "the maximum channel count must be between 1 and 8"
            );
        }

        Self {
            state: State::Marker,
            information: None,
            samples: [[0; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
            block_size: 0,
            position: 0,
            required: 0,
            finishing: false,
            sample: PhantomData,
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Marker;
        self.information = None;
        self.block_size = 0;
        self.position = 0;
        self.required = 0;
        self.finishing = false;
    }

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
            State::Frames => Err(Error::InvalidParameter),
        }
    }

    fn decode_wide_subframe(
        &mut self,
        reader: &mut BitReader,
        block_size: usize,
        side: usize,
        mid_side: bool,
    ) -> Result<()> {
        let header = SubframeHeader::read(reader, 33)?;
        let bits = header.bits_per_sample;
        let wasted_bits = header.wasted_bits;
        let samples = &mut self.samples;
        let store = |samples: &mut [[i32; MAXIMUM_BLOCK_SIZE]; MAXIMUM_CHANNELS],
                     index: usize,
                     value: i64| {
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
                let value = reader.read_signed_wide(bits)?;

                for index in 0..block_size {
                    store(samples, index, value);
                }
            } else {
                for index in 0..block_size {
                    let value = reader.read_signed_wide(bits)?;

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
            history[order - 1 - index] = reader.read_signed_wide(bits)?;
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

    fn decode_channel(
        &mut self,
        reader: &mut BitReader,
        block_size: usize,
        channel: usize,
        bits_per_sample: u32,
        mid_side: bool,
    ) -> Result<bool> {
        if bits_per_sample > 32 {
            self.decode_wide_subframe(reader, block_size, channel, mid_side)?;

            return Ok(mid_side);
        }

        decode_subframe(
            reader,
            &mut self.samples[channel][..block_size],
            bits_per_sample,
        )?;

        Ok(false)
    }

    fn decode_frame(&mut self, input: &[u8]) -> Result<usize> {
        let information = self.information.ok_or(Error::InvalidHeader)?;
        let header = FrameHeader::parse(input, &information)?;
        let block_size = header.block_size;
        let bits = header.bits_per_sample;
        let mut reader = BitReader::new(&input[header.size..]);

        match header.assignment {
            Assignment::Independent(channels) => {
                for channel in 0..channels {
                    self.decode_channel(&mut reader, block_size, channel, bits, false)?;
                }
            }
            Assignment::LeftSide => {
                self.decode_channel(&mut reader, block_size, 0, bits, false)?;
                self.decode_channel(&mut reader, block_size, 1, bits + 1, false)?;

                let [left, side] = self.stereo()?;

                for (side, left) in side[..block_size].iter_mut().zip(&left[..block_size]) {
                    *side = left.wrapping_sub(*side);
                }
            }
            Assignment::RightSide => {
                self.decode_channel(&mut reader, block_size, 0, bits + 1, false)?;
                self.decode_channel(&mut reader, block_size, 1, bits, false)?;

                let [side, right] = self.stereo()?;

                for (side, right) in side[..block_size].iter_mut().zip(&right[..block_size]) {
                    *side = side.wrapping_add(*right);
                }
            }
            Assignment::MidSide => {
                self.decode_channel(&mut reader, block_size, 0, bits, false)?;

                if !self.decode_channel(&mut reader, block_size, 1, bits + 1, true)? {
                    let [mid, side] = self.stereo()?;

                    for (mid, side) in mid[..block_size].iter_mut().zip(&mut side[..block_size]) {
                        let sum = (i64::from(*mid) << 1) | i64::from(*side & 1);
                        let difference = i64::from(*side);

                        *mid = ((sum + difference) >> 1) as i32;
                        *side = ((sum - difference) >> 1) as i32;
                    }
                }
            }
        }

        reader.align();

        let footer = header.size + reader.byte_position();
        let crc = input
            .get(footer..footer + FRAME_FOOTER_SIZE)
            .ok_or(Error::Incomplete)?;

        if u16::from_be_bytes([crc[0], crc[1]]) != crc_16(&input[..footer]) {
            return Err(Error::InvalidData);
        }

        self.block_size = block_size;
        self.position = 0;

        Ok(footer + FRAME_FOOTER_SIZE)
    }

    fn stereo(&mut self) -> Result<[&mut [i32; MAXIMUM_BLOCK_SIZE]; 2]> {
        self.samples
            .get_disjoint_mut([0, 1])
            .map_err(|_| Error::UnsupportedFormat)
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
                *output = S::from_i32(self.samples[channel][self.position + frame] << shift);
            }
        }

        self.position += frames;

        frames * channels
    }
}

impl<S: Sample, const MAXIMUM_BLOCK_SIZE: usize, const MAXIMUM_CHANNELS: usize> Decoder
    for FlacDecoder<S, MAXIMUM_BLOCK_SIZE, MAXIMUM_CHANNELS>
{
    type Sample = S;

    fn configuration(&self) -> Option<Configuration> {
        self.information.map(|information| {
            Configuration::new(information.sample_rate, information.channels, S::FORMAT)
        })
    }

    fn minimum_input(&self) -> usize {
        match (self.state, self.information) {
            (State::Marker, _) => MARKER_SIZE,
            (State::BlockHeader, _) => BLOCK_HEADER_SIZE,
            (State::StreamInformation { .. }, _) => STREAM_INFORMATION_SIZE,
            (State::Skip { .. }, _) | (State::Frames, None) => 1,
            (State::Frames, Some(information)) => information.maximum_frame_size_bound(),
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
            let result = match self.state {
                State::Frames
                    if remaining.is_empty()
                        || (!self.finishing && remaining.len() < self.required) =>
                {
                    return Ok(progress);
                }
                State::Frames => self.decode_frame(remaining),
                _ => self.read_metadata(remaining),
            };

            match result {
                Ok(consumed) => {
                    progress.consumed += consumed;
                    self.required = 0;
                }
                Err(Error::Incomplete) if !self.finishing || remaining.is_empty() => {
                    if self.state == State::Frames {
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
