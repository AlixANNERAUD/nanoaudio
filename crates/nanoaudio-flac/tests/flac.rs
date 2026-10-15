use nanoaudio_core::codec::Decoder;
use nanoaudio_core::{Configuration, Error, Format, Sample};
use nanoaudio_flac::{FlacDecoder, FlacStreamInformation};

const STEREO_16: &[u8] = include_bytes!("data/flac_stereo_16.flac");
const STEREO_16_LEVEL_0: &[u8] = include_bytes!("data/flac_stereo_16_level_0.flac");
const STEREO_16_LEVEL_8: &[u8] = include_bytes!("data/flac_stereo_16_level_8.flac");
const STEREO_16_BLOCK_1000: &[u8] = include_bytes!("data/flac_stereo_16_block_1000.flac");
const STEREO_16_METADATA: &[u8] = include_bytes!("data/flac_stereo_16_metadata.flac");
const MONO_16: &[u8] = include_bytes!("data/flac_mono_16.flac");
const NOISE_16: &[u8] = include_bytes!("data/flac_noise_16.flac");
const MONO_8: &[u8] = include_bytes!("data/flac_mono_8.flac");
const SILENCE_16: &[u8] = include_bytes!("data/flac_silence_16.flac");
const STEREO_24: &[u8] = include_bytes!("data/flac_stereo_24.flac");
const WASTED_24: &[u8] = include_bytes!("data/flac_wasted_24.flac");
const SURROUND_16: &[u8] = include_bytes!("data/flac_surround_16.flac");
const STEREO_32: &[u8] = include_bytes!("data/flac_stereo_32.flac");

fn samples_16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes(*chunk))
        .collect()
}

fn samples_32(bytes: &[u8]) -> Vec<i32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| i32::from_le_bytes(*chunk))
        .collect()
}

fn stereo_16() -> Vec<i16> {
    samples_16(include_bytes!("data/flac_stereo_16.bin"))
}

fn decode_with<D: Decoder>(
    mut decoder: D,
    file: &[u8],
    chunk: usize,
    output_size: usize,
) -> Result<(D, Vec<D::Sample>), Error> {
    let mut output = vec![D::Sample::default(); output_size.max(decoder.minimum_output())];
    let mut pending = Vec::new();
    let mut decoded = Vec::new();
    let mut offset = 0;

    loop {
        if offset == file.len() {
            decoder.finish();
        }

        let progress = decoder.decode(&pending, &mut output)?;
        decoded.extend_from_slice(&output[..progress.produced]);
        pending.drain(..progress.consumed);

        if progress.is_stalled() {
            if offset == file.len() {
                break;
            }

            let end = (offset + chunk).min(file.len());
            pending.extend_from_slice(&file[offset..end]);
            offset = end;
        }
    }

    assert!(pending.is_empty(), "{} bytes left", pending.len());

    Ok((decoder, decoded))
}

fn decode<S: Sample>(file: &[u8]) -> Vec<S> {
    decode_with(FlacDecoder::<S>::new(), file, file.len(), 4096)
        .unwrap()
        .1
}

#[test]
fn stereo_16_matches_reference() {
    let expected = stereo_16();

    for file in [
        STEREO_16,
        STEREO_16_LEVEL_0,
        STEREO_16_LEVEL_8,
        STEREO_16_BLOCK_1000,
        STEREO_16_METADATA,
    ] {
        assert_eq!(decode::<i16>(file), expected);
        assert_eq!(
            decode::<i32>(file),
            expected
                .iter()
                .map(|sample| (*sample as i32) << 16)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn mono_16_matches_reference() {
    assert_eq!(
        decode::<i16>(MONO_16),
        samples_16(include_bytes!("data/flac_mono_16.bin"))
    );
    assert_eq!(
        decode::<i16>(NOISE_16),
        samples_16(include_bytes!("data/flac_noise_16.bin"))
    );
}

#[test]
fn mono_8_matches_reference() {
    let expected = samples_16(include_bytes!("data/flac_mono_8.bin"));

    assert_eq!(
        decode::<i16>(MONO_8),
        expected
            .iter()
            .map(|sample| sample << 8)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        decode::<u8>(MONO_8),
        expected
            .iter()
            .map(|sample| (sample + 0x80) as u8)
            .collect::<Vec<_>>()
    );
}

#[test]
fn silence_is_zero() {
    let samples = decode::<i16>(SILENCE_16);

    assert_eq!(samples.len(), 9_000);
    assert!(samples.iter().all(|sample| *sample == 0));
}

#[test]
fn deep_files_match_reference() {
    let expected_24 = samples_32(include_bytes!("data/flac_stereo_24.bin"));
    let expected_32 = samples_32(include_bytes!("data/flac_stereo_32.bin"));

    assert_eq!(
        decode::<i32>(STEREO_24),
        expected_24
            .iter()
            .map(|sample| sample << 8)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        decode::<i32>(WASTED_24),
        samples_32(include_bytes!("data/flac_wasted_24.bin"))
            .iter()
            .map(|sample| sample << 8)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        decode::<i16>(STEREO_24),
        expected_24
            .iter()
            .map(|sample| (sample.signum() * ((sample.abs() + 0x80) >> 8)).min(0x7FFF) as i16)
            .collect::<Vec<_>>()
    );
    assert_eq!(decode::<i32>(STEREO_32), expected_32);
}

#[test]
fn surround_needs_enough_channels() {
    let expected = samples_16(include_bytes!("data/flac_surround_16.bin"));
    let (decoder, samples) = decode_with(
        FlacDecoder::<i16, 4608, 8>::new(),
        SURROUND_16,
        SURROUND_16.len(),
        4096,
    )
    .unwrap();

    assert_eq!(samples, expected);
    assert_eq!(
        decoder.configuration(),
        Some(Configuration::new(48_000, 6, Format::I16))
    );
    assert_eq!(
        decode_with(FlacDecoder::<i16>::new(), SURROUND_16, 1_000, 4096).err(),
        Some(Error::TooManyChannels)
    );
}

#[test]
fn small_chunks_match_one_shot() {
    for file in [
        STEREO_16,
        STEREO_16_BLOCK_1000,
        STEREO_16_METADATA,
        MONO_16,
        STEREO_24,
        STEREO_32,
    ] {
        let reference = decode::<i32>(file);

        for (chunk, output) in [(7, 4096), (512, 2), (513, 7), (4096, 1000)] {
            let (_, samples) = decode_with(FlacDecoder::<i32>::new(), file, chunk, output).unwrap();

            assert_eq!(samples, reference, "chunk {chunk}, output {output}");
        }
    }

    for file in [MONO_8, WASTED_24, SILENCE_16, STEREO_16_METADATA] {
        let (_, samples) = decode_with(FlacDecoder::<i32>::new(), file, 1, 4096).unwrap();

        assert_eq!(samples, decode::<i32>(file));
    }
}

#[test]
fn configuration_and_stream_information() {
    let mut decoder = FlacDecoder::<i16>::new();

    assert_eq!(decoder.configuration(), None);
    assert_eq!(decoder.minimum_output(), 2);
    assert_eq!(decoder.minimum_input(), 4);

    let mut output = [0i16; 2];
    let progress = decoder.decode(&STEREO_16[..42], &mut output).unwrap();

    assert_eq!((progress.consumed, progress.produced), (42, 0));
    assert_eq!(
        decoder.configuration(),
        Some(Configuration::new(44_100, 2, Format::I16))
    );

    let information = decoder.stream_information().unwrap();

    assert_eq!(
        information,
        FlacStreamInformation {
            minimum_block_size: 4096,
            maximum_block_size: 4096,
            minimum_frame_size: information.minimum_frame_size,
            maximum_frame_size: information.maximum_frame_size,
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            total_samples: 4_500,
        }
    );
    assert!(information.maximum_frame_size > 0);
    assert_eq!(decoder.minimum_input(), 4);

    let (decoder, samples) = decode_with(decoder, &STEREO_16[42..], 4096, 4096).unwrap();

    assert_eq!(samples, stereo_16());

    assert_eq!(
        decoder.minimum_input(),
        information.maximum_frame_size as usize
    );

    let (decoder, _) = decode_with(FlacDecoder::<f32>::new(), MONO_8, 64, 64).unwrap();

    assert_eq!(
        decoder.configuration(),
        Some(Configuration::new(8_000, 1, Format::F32))
    );
    assert_eq!(decoder.minimum_output(), 1);
    assert_eq!(decoder.stream_information().unwrap().bits_per_sample, 8);
}

#[test]
fn float_output_matches_integer_output() {
    let integers = decode::<i16>(STEREO_16);
    let floats = decode::<f32>(STEREO_16);

    for (float, integer) in floats.iter().zip(&integers) {
        assert_eq!(*float, *integer as f32 / 32_768.0);
    }
}

#[test]
fn rejects_unsupported_streams() {
    let mut output = [0i16; 4096];

    assert_eq!(
        FlacDecoder::<i16>::new().decode(b"RIFF", &mut output),
        Err(Error::InvalidHeader)
    );
    assert_eq!(
        FlacDecoder::<i16, 1024>::new().decode(STEREO_16, &mut output),
        Ok(nanoaudio_core::codec::Progress::new(8, 0))
    );
    assert_eq!(
        decode_with(FlacDecoder::<i16, 1024>::new(), STEREO_16, 4096, 4096).err(),
        Some(Error::UnsupportedFormat)
    );
    assert_eq!(
        FlacDecoder::<i16>::new().decode(STEREO_16, &mut output[..1]),
        Err(Error::BufferTooSmall)
    );

    let mut missing_stream_information = STEREO_16.to_vec();
    missing_stream_information[4] = 0x01;

    assert_eq!(
        decode_with(
            FlacDecoder::<i16>::new(),
            &missing_stream_information,
            4096,
            4096
        )
        .err(),
        Some(Error::InvalidHeader)
    );
}

#[test]
fn buffer_of_minimum_input_is_enough() {
    for file in [STEREO_16, STEREO_16_METADATA, MONO_16, STEREO_24, STEREO_32] {
        let mut decoder = FlacDecoder::<i32>::new();
        let mut output = vec![0i32; 64];
        let mut pending = Vec::new();
        let mut decoded = Vec::new();
        let mut offset = 0;

        loop {
            let capacity = decoder.minimum_input();
            let end = (offset + capacity.saturating_sub(pending.len()).min(100)).min(file.len());
            pending.extend_from_slice(&file[offset..end]);
            offset = end;

            if offset == file.len() {
                decoder.finish();
            }

            let progress = decoder.decode(&pending, &mut output).unwrap();
            decoded.extend_from_slice(&output[..progress.produced]);
            pending.drain(..progress.consumed);

            if progress.is_stalled() && offset == file.len() {
                break;
            }

            assert!(!progress.is_stalled() || pending.len() < capacity);
        }

        assert_eq!(decoded, decode::<i32>(file));
    }
}

fn first_frame_offset(file: &[u8]) -> usize {
    let mut position = 4;

    loop {
        let header = file[position];
        let length = u32::from_be_bytes([
            0,
            file[position + 1],
            file[position + 2],
            file[position + 3],
        ]) as usize;

        position += 4 + length;

        if header & 0x80 != 0 {
            return position;
        }
    }
}

#[test]
fn detects_corruption() {
    let frame = first_frame_offset(STEREO_16);

    for (offset, mask) in [
        (frame + 2, 0x10),
        (frame + 6, 0x01),
        (frame + 100, 0x20),
        (STEREO_16.len() - 1, 0x01),
        (STEREO_16.len() - 3_000, 0x80),
    ] {
        let mut corrupt = STEREO_16.to_vec();
        corrupt[offset] ^= mask;

        let result = decode_with(FlacDecoder::<i16>::new(), &corrupt, 4096, 4096);

        assert!(
            matches!(
                result,
                Err(Error::InvalidData | Error::UnsupportedFormat | Error::Incomplete)
            ),
            "offset {offset}"
        );
    }

    let mut corrupt = STEREO_16.to_vec();
    let length = corrupt.len();
    corrupt[length - 1] ^= 0xFF;

    let mut decoder = FlacDecoder::<i16>::new();
    let mut output = vec![0i16; 16_384];
    let progress = decoder.decode(&corrupt, &mut output).unwrap();

    assert!(progress.consumed < corrupt.len());
    assert_eq!(
        decoder.decode(&corrupt[progress.consumed..], &mut output),
        Err(Error::InvalidData)
    );
}

#[test]
fn truncated_input_never_panics() {
    for file in [STEREO_16_METADATA, STEREO_24, STEREO_32, MONO_8] {
        let reference = decode::<i32>(file);

        for length in (0..file.len()).step_by(97) {
            match decode_with(FlacDecoder::<i32>::new(), &file[..length], 300, 4096) {
                Ok((_, samples)) => assert!(reference.starts_with(&samples)),
                Err(error) => assert_eq!(error, Error::Incomplete, "length {length}"),
            }
        }
    }
}

#[test]
fn random_corruption_never_panics() {
    let mut state = 0x2545_F491_4F6C_DD1Du64;

    for _ in 0..300 {
        let mut corrupt = STEREO_32.to_vec();

        for _ in 0..4 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;

            let offset = 42 + (state as usize) % (corrupt.len() - 42);
            corrupt[offset] ^= (state >> 56) as u8 | 1;
        }

        let _ = decode_with(FlacDecoder::<i32>::new(), &corrupt, 1024, 4096);
    }
}

struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            used: 8,
        }
    }

    fn write(&mut self, value: u64, count: u32) {
        for bit in (0..count).rev() {
            if self.used == 8 {
                self.bytes.push(0);
                self.used = 0;
            }

            let last = self.bytes.len() - 1;
            self.bytes[last] |= (((value >> bit) & 1) as u8) << (7 - self.used);
            self.used += 1;
        }
    }

    fn write_signed(&mut self, value: i64, count: u32) {
        if count == 0 {
            assert_eq!(value, 0);
            return;
        }

        assert!(
            count == 64 || (value >= -(1 << (count - 1)) && value < (1 << (count - 1))),
            "{value} does not fit {count} bits"
        );

        self.write(value as u64, count);
    }

    fn write_unary(&mut self, zeros: u64) {
        for _ in 0..zeros {
            self.write(0, 1);
        }

        self.write(1, 1);
    }

    fn align(&mut self) {
        self.used = 8;
    }
}

fn reference_crc_8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;

    for byte in bytes {
        crc ^= byte;

        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }

    crc
}

fn reference_crc_16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;

    for byte in bytes {
        crc ^= (*byte as u16) << 8;

        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }

    crc
}

#[derive(Clone)]
enum Coding {
    Rice(Option<u32>),
    Escape,
}

#[derive(Clone)]
enum Kind {
    Constant,
    Verbatim,
    Fixed(usize),
    Linear {
        coefficients: Vec<i64>,
        precision: u32,
        shift: u32,
    },
}

#[derive(Clone)]
struct Subframe {
    kind: Kind,
    wasted_bits: u32,
    parameter_bits: u32,
    partition_order: u32,
    codings: Vec<Coding>,
}

impl Subframe {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            wasted_bits: 0,
            parameter_bits: 4,
            partition_order: 0,
            codings: vec![Coding::Rice(None)],
        }
    }

    fn wasted(mut self, wasted_bits: u32) -> Self {
        self.wasted_bits = wasted_bits;
        self
    }

    fn partitions(mut self, parameter_bits: u32, partition_order: u32, codings: &[Coding]) -> Self {
        self.parameter_bits = parameter_bits;
        self.partition_order = partition_order;
        self.codings = codings.to_vec();
        self
    }

    fn write(&self, writer: &mut BitWriter, samples: &[i64], bits: u32) {
        let values: Vec<i64> = samples
            .iter()
            .map(|sample| {
                assert_eq!(sample & ((1 << self.wasted_bits) - 1), 0);
                sample >> self.wasted_bits
            })
            .collect();
        let bits = bits - self.wasted_bits;
        let (kind, coefficients, shift): (u64, Vec<i64>, u32) = match &self.kind {
            Kind::Constant => (0, Vec::new(), 0),
            Kind::Verbatim => (1, Vec::new(), 0),
            Kind::Fixed(order) => (
                8 + *order as u64,
                [
                    vec![],
                    vec![1],
                    vec![2, -1],
                    vec![3, -3, 1],
                    vec![4, -6, 4, -1],
                ][*order]
                    .clone(),
                0,
            ),
            Kind::Linear {
                coefficients,
                shift,
                ..
            } => (31 + coefficients.len() as u64, coefficients.clone(), *shift),
        };

        writer.write(0, 1);
        writer.write(kind, 6);

        if self.wasted_bits > 0 {
            writer.write(1, 1);
            writer.write_unary(self.wasted_bits as u64 - 1);
        } else {
            writer.write(0, 1);
        }

        match &self.kind {
            Kind::Constant => {
                assert!(values.iter().all(|value| *value == values[0]));
                writer.write_signed(values[0], bits);
                return;
            }
            Kind::Verbatim => {
                for value in &values {
                    writer.write_signed(*value, bits);
                }
                return;
            }
            _ => {}
        }

        let order = coefficients.len();

        for value in &values[..order] {
            writer.write_signed(*value, bits);
        }

        if let Kind::Linear {
            coefficients,
            precision,
            shift,
        } = &self.kind
        {
            writer.write(*precision as u64 - 1, 4);
            writer.write_signed(*shift as i64, 5);

            for coefficient in coefficients {
                writer.write_signed(*coefficient, *precision);
            }
        }

        let residuals: Vec<i64> = (order..values.len())
            .map(|index| {
                let prediction: i64 = coefficients
                    .iter()
                    .enumerate()
                    .map(|(offset, coefficient)| coefficient * values[index - 1 - offset])
                    .sum();
                let residual = values[index] - (prediction >> shift);

                assert!(residual.abs() < 1 << 31);
                residual
            })
            .collect();

        writer.write(self.parameter_bits as u64 - 4, 2);
        writer.write(self.partition_order as u64, 4);

        let partition_size = values.len() >> self.partition_order;
        let escape = (1u64 << self.parameter_bits) - 1;
        let mut start = 0;

        for (partition, coding) in self.codings.iter().enumerate() {
            let count = if partition == 0 {
                partition_size - order
            } else {
                partition_size
            };
            let residuals = &residuals[start..start + count];
            start += count;

            match coding {
                Coding::Escape => {
                    let width = residuals
                        .iter()
                        .map(|residual| 65 - residual.leading_zeros().min(residual.leading_ones()))
                        .max()
                        .unwrap_or(0)
                        .min(31);
                    let width = if residuals.iter().all(|residual| *residual == 0) {
                        0
                    } else {
                        width
                    };

                    writer.write(escape, self.parameter_bits);
                    writer.write(width as u64, 5);

                    for residual in residuals {
                        writer.write_signed(*residual, width);
                    }
                }
                Coding::Rice(parameter) => {
                    let folded: Vec<u64> = residuals
                        .iter()
                        .map(|residual| ((residual << 1) ^ (residual >> 63)) as u64)
                        .collect();
                    let parameter = parameter.unwrap_or_else(|| {
                        let largest = folded.iter().copied().max().unwrap_or(0);

                        (64 - largest.leading_zeros())
                            .saturating_sub(2)
                            .min(escape as u32 - 1)
                    });

                    writer.write(parameter as u64, self.parameter_bits);

                    for folded in folded {
                        writer.write_unary(folded >> parameter);
                        writer.write(folded & ((1 << parameter) - 1), parameter);
                    }
                }
            }
        }

        assert_eq!(start, residuals.len());
    }
}

fn frame(
    number: u64,
    channel_code: u8,
    bits: u32,
    channels: &[(Subframe, Vec<i64>, u32)],
) -> Vec<u8> {
    let block_size = channels[0].1.len();
    let bits_code = match bits {
        8 => 1,
        12 => 2,
        16 => 4,
        20 => 5,
        24 => 6,
        32 => 7,
        _ => 0,
    };
    let block_code = if block_size <= 256 { 6 } else { 7 };
    let mut header = vec![
        0xFF,
        0xF8,
        (block_code << 4) | 0x0D,
        (channel_code << 4) | (bits_code << 1),
    ];

    if number < 0x80 {
        header.push(number as u8);
    } else {
        header.push(0xC0 | (number >> 6) as u8);
        header.push(0x80 | (number & 0x3F) as u8);
    }

    if block_code == 6 {
        header.push((block_size - 1) as u8);
    } else {
        header.extend_from_slice(&(block_size as u16 - 1).to_be_bytes());
    }

    header.extend_from_slice(&12_345u16.to_be_bytes());
    header.push(reference_crc_8(&header));

    let mut writer = BitWriter::new();
    writer.bytes = header;

    for (subframe, samples, subframe_bits) in channels {
        subframe.write(&mut writer, samples, *subframe_bits);
    }

    writer.align();

    let mut bytes = writer.bytes;
    let crc = reference_crc_16(&bytes);
    bytes.extend_from_slice(&crc.to_be_bytes());
    bytes
}

fn stream(channels: u8, bits: u32, maximum_block_size: u16, frames: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend_from_slice(&[0x80, 0, 0, 34]);
    bytes.extend_from_slice(&16u16.to_be_bytes());
    bytes.extend_from_slice(&maximum_block_size.to_be_bytes());
    bytes.extend_from_slice(&[0; 6]);

    let packed: u64 = (12_345u64 << 44) | ((channels as u64 - 1) << 41) | ((bits as u64 - 1) << 36);
    bytes.extend_from_slice(&packed.to_be_bytes());
    bytes.extend_from_slice(&[0; 16]);

    for frame in frames {
        bytes.extend_from_slice(frame);
    }

    bytes
}

fn wave(length: usize, amplitude: f64, period: f64, phase: f64) -> Vec<i64> {
    (0..length)
        .map(|index| {
            (amplitude * (core::f64::consts::TAU * index as f64 / period + phase).sin()) as i64
        })
        .collect()
}

fn scaled(samples: &[i64], bits: u32) -> Vec<i32> {
    samples
        .iter()
        .map(|sample| (*sample as i32) << (32 - bits))
        .collect()
}

fn interleave(channels: &[&[i64]]) -> Vec<i64> {
    (0..channels[0].len())
        .flat_map(|index| channels.iter().map(move |channel| channel[index]))
        .collect()
}

fn decode_handcrafted<const CHANNELS: usize>(file: &[u8]) -> Result<Vec<i32>, Error> {
    let one_shot = decode_with(
        FlacDecoder::<i32, 4608, CHANNELS>::new(),
        file,
        file.len(),
        64,
    )?
    .1;
    let chunked = decode_with(FlacDecoder::<i32, 4608, CHANNELS>::new(), file, 3, 64)?.1;

    assert_eq!(one_shot, chunked);

    Ok(one_shot)
}

#[test]
fn handcrafted_mono_bit_depths() {
    let constant = vec![-5i64; 40];
    let verbatim: Vec<i64> = (0..40).map(|index| (index % 16) - 8).collect();
    let file = stream(
        1,
        4,
        64,
        &[
            frame(
                0,
                0,
                4,
                &[(Subframe::new(Kind::Constant), constant.clone(), 4)],
            ),
            frame(
                1,
                0,
                4,
                &[(Subframe::new(Kind::Verbatim), verbatim.clone(), 4)],
            ),
        ],
    );

    assert_eq!(
        decode_handcrafted::<1>(&file).unwrap(),
        scaled(&[constant, verbatim].concat(), 4)
    );

    let mut twelve = wave(200, 1_900.0, 37.0, 0.3);
    twelve[150] = 2_047;
    twelve[151] = -2_048;
    let subframe = Subframe::new(Kind::Fixed(2)).partitions(
        4,
        3,
        &[
            Coding::Escape,
            Coding::Rice(Some(0)),
            Coding::Escape,
            Coding::Rice(None),
            Coding::Rice(Some(14)),
            Coding::Escape,
            Coding::Rice(None),
            Coding::Rice(Some(1)),
        ],
    );
    let quiet = vec![0i64; 200];
    let file = stream(
        1,
        12,
        256,
        &[
            frame(0, 0, 12, &[(subframe.clone(), twelve.clone(), 12)]),
            frame(
                200,
                0,
                12,
                &[(
                    Subframe::new(Kind::Fixed(1)).partitions(
                        4,
                        1,
                        &[Coding::Escape, Coding::Escape],
                    ),
                    quiet.clone(),
                    12,
                )],
            ),
        ],
    );

    assert_eq!(
        decode_handcrafted::<1>(&file).unwrap(),
        scaled(&[twelve, quiet].concat(), 12)
    );

    let twenty = wave(1_000, 500_000.0, 61.0, 0.0);
    let linear = Subframe::new(Kind::Linear {
        coefficients: vec![7_000, -3_500, 500],
        precision: 15,
        shift: 12,
    })
    .partitions(
        5,
        2,
        &[
            Coding::Rice(None),
            Coding::Rice(Some(20)),
            Coding::Escape,
            Coding::Rice(None),
        ],
    );
    let file = stream(
        1,
        20,
        1_000,
        &[frame(0, 0, 20, &[(linear, twenty.clone(), 20)])],
    );

    assert_eq!(decode_handcrafted::<1>(&file).unwrap(), scaled(&twenty, 20));
}

fn stereo_frame(
    left: &[i64],
    right: &[i64],
    bits: u32,
    mode: u8,
    subframes: [Subframe; 2],
) -> Vec<u8> {
    let side: Vec<i64> = left
        .iter()
        .zip(right)
        .map(|(left, right)| left - right)
        .collect();
    let mid: Vec<i64> = left
        .iter()
        .zip(right)
        .map(|(left, right)| (left + right) >> 1)
        .collect();
    let [first, second] = subframes;
    let channels = match mode {
        1 => [(first, left.to_vec(), bits), (second, right.to_vec(), bits)],
        8 => [(first, left.to_vec(), bits), (second, side, bits + 1)],
        9 => [(first, side, bits + 1), (second, right.to_vec(), bits)],
        _ => [(first, mid, bits), (second, side, bits + 1)],
    };

    frame(0, mode, bits, &channels)
}

#[test]
fn handcrafted_stereo_decorrelation() {
    let length = 300;

    for bits in [16u32, 24, 32] {
        let amplitude = ((1i64 << (bits - 1)) - 1) as f64 * 0.95;
        let left = wave(length, amplitude, 64.0, 0.0);
        let right: Vec<i64> = wave(length, amplitude, 64.0, 3.0)
            .iter()
            .map(|sample| sample & !3)
            .collect();
        let expected = scaled(&interleave(&[&left, &right]), bits);
        let linear = || Kind::Linear {
            coefficients: vec![8_100, -4_096],
            precision: 15,
            shift: 12,
        };
        let rice = |subframe: Subframe| {
            subframe.partitions(
                5,
                2,
                &[
                    Coding::Rice(None),
                    Coding::Escape,
                    Coding::Rice(None),
                    Coding::Rice(None),
                ],
            )
        };

        for mode in [1u8, 8, 9, 10] {
            for subframes in [
                [
                    rice(Subframe::new(Kind::Fixed(1))),
                    rice(Subframe::new(Kind::Fixed(2))),
                ],
                [rice(Subframe::new(linear())), rice(Subframe::new(linear()))],
                [Subframe::new(Kind::Verbatim), Subframe::new(Kind::Verbatim)],
            ] {
                let file = stream(
                    2,
                    bits,
                    length as u16,
                    &[stereo_frame(&left, &right, bits, mode, subframes)],
                );

                assert_eq!(
                    decode_handcrafted::<2>(&file).unwrap(),
                    expected,
                    "bits {bits}, mode {mode}"
                );
            }
        }

        let wasted_right = [
            Subframe::new(Kind::Verbatim),
            rice(Subframe::new(Kind::Fixed(1))).wasted(2),
        ];
        let file = stream(
            2,
            bits,
            length as u16,
            &[stereo_frame(&left, &right, bits, 1, wasted_right)],
        );

        assert_eq!(decode_handcrafted::<2>(&file).unwrap(), expected);

        let constant = vec![(1i64 << (bits - 1)) - 1; length];
        let opposite = vec![-(1i64 << (bits - 1)); length];
        let expected = scaled(&interleave(&[&constant, &opposite]), bits);

        for mode in [8u8, 9, 10] {
            let file = stream(
                2,
                bits,
                length as u16,
                &[stereo_frame(
                    &constant,
                    &opposite,
                    bits,
                    mode,
                    [Subframe::new(Kind::Constant), Subframe::new(Kind::Constant)],
                )],
            );

            assert_eq!(
                decode_handcrafted::<2>(&file).unwrap(),
                expected,
                "bits {bits}, mode {mode}"
            );
        }
    }
}

#[test]
fn handcrafted_wide_side_with_wasted_bits() {
    let length = 128;
    let left: Vec<i64> = wave(length, 2.0e9, 50.0, 0.0)
        .iter()
        .map(|sample| sample & !0xFF)
        .collect();
    let right: Vec<i64> = left.iter().map(|sample| -sample - 512).collect();
    let expected = scaled(&interleave(&[&left, &right]), 32);
    let side = Subframe::new(Kind::Linear {
        coefficients: vec![7_000, -3_000],
        precision: 14,
        shift: 12,
    })
    .wasted(8)
    .partitions(5, 1, &[Coding::Rice(None), Coding::Escape]);

    for mode in [8u8, 9, 10] {
        let subframes = if mode == 9 {
            [side.clone(), Subframe::new(Kind::Fixed(3)).wasted(8)]
        } else {
            [Subframe::new(Kind::Fixed(3)).wasted(8), side.clone()]
        };
        let file = stream(
            2,
            32,
            length as u16,
            &[stereo_frame(&left, &right, 32, mode, subframes)],
        );

        assert_eq!(
            decode_handcrafted::<2>(&file).unwrap(),
            expected,
            "mode {mode}"
        );
    }
}

#[test]
fn handcrafted_invalid_streams() {
    let samples = wave(64, 1_000.0, 16.0, 0.0);
    let valid = frame(
        0,
        0,
        16,
        &[(Subframe::new(Kind::Fixed(2)), samples.clone(), 16)],
    );
    let header = valid.len() - 2;

    assert_eq!(
        decode_handcrafted::<1>(&stream(1, 16, 64, core::slice::from_ref(&valid))).unwrap(),
        scaled(&samples, 16)
    );

    assert_eq!(
        decode_handcrafted::<2>(&stream(2, 16, 64, core::slice::from_ref(&valid))),
        Err(Error::UnsupportedFormat)
    );
    assert_eq!(
        decode_handcrafted::<1>(&stream(1, 16, 32, core::slice::from_ref(&valid))),
        Err(Error::InvalidData)
    );

    let mut reserved = valid.clone();
    reserved[9] |= 0x80;
    let crc = reference_crc_16(&reserved[..header]);
    reserved[header..].copy_from_slice(&crc.to_be_bytes());

    assert_eq!(
        decode_handcrafted::<1>(&stream(1, 16, 64, &[reserved])),
        Err(Error::InvalidData)
    );

    let mut crc_16 = valid;
    crc_16[header] ^= 1;

    assert_eq!(
        decode_handcrafted::<1>(&stream(1, 16, 64, &[crc_16])),
        Err(Error::InvalidData)
    );
}
