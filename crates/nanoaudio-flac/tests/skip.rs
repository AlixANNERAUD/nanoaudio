use std::time::Instant;

use nanoaudio_core::codec::{Decoder, Seek};
use nanoaudio_core::{Error, Result};
use nanoaudio_flac::FlacDecoder;

type Flac = FlacDecoder<i32, 4096, 2>;

const CHANNELS: usize = 2;
const BLOCK: usize = 4096;
const HEADER: usize = 4 + 4 + 34;

struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
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

    fn write_unary(&mut self, zeros: u64) {
        for _ in 0..zeros {
            self.write(0, 1);
        }

        self.write(1, 1);
    }
}

fn crc_8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |mut crc, byte| {
        crc ^= byte;

        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }

        crc
    })
}

fn crc_16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0, |mut crc, byte| {
        crc ^= u16::from(*byte) << 8;

        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }

        crc
    })
}

fn coded_number(number: u64) -> Vec<u8> {
    if number < 0x80 {
        return vec![number as u8];
    }

    let length = (2..=7u32)
        .find(|length| number < 1 << (7 - length + 6 * (length - 1)))
        .unwrap();
    let mut bytes = vec![(0xFF00u16 >> length) as u8 | (number >> (6 * (length - 1))) as u8];

    for index in (0..length - 1).rev() {
        bytes.push(0x80 | ((number >> (6 * index)) & 0x3F) as u8);
    }

    bytes
}

fn subframe(writer: &mut BitWriter, samples: &[i32]) {
    let residuals: Vec<u64> = (2..samples.len())
        .map(|index| {
            let residual = i64::from(samples[index]) - 2 * i64::from(samples[index - 1])
                + i64::from(samples[index - 2]);

            ((residual << 1) ^ (residual >> 63)) as u64
        })
        .collect();
    let mean = residuals.iter().sum::<u64>() / residuals.len().max(1) as u64;
    let parameter = (64 - mean.leading_zeros()).min(14);

    writer.write(10 << 1, 8);

    for sample in &samples[..2] {
        writer.write(*sample as u16 as u64, 16);
    }

    writer.write(0, 2);
    writer.write(0, 4);
    writer.write(u64::from(parameter), 4);

    for folded in residuals {
        writer.write_unary(folded >> parameter);
        writer.write(folded & ((1 << parameter) - 1), parameter);
    }
}

fn encode(signal: &[Vec<i32>], blocks: &[usize], variable: bool) -> (Vec<u8>, Vec<usize>) {
    let total: usize = blocks.iter().sum();
    let minimum = blocks[..blocks.len() - 1]
        .iter()
        .copied()
        .min()
        .unwrap_or(BLOCK);
    let maximum = blocks.iter().copied().max().unwrap();
    let mut bytes = b"fLaC".to_vec();

    bytes.extend_from_slice(&[0x80, 0, 0, 34]);
    bytes.extend_from_slice(&(minimum.max(16) as u16).to_be_bytes());
    bytes.extend_from_slice(&(maximum as u16).to_be_bytes());
    bytes.extend_from_slice(&[0; 6]);

    let packed = (44_100u64 << 44) | ((CHANNELS as u64 - 1) << 41) | (15 << 36) | total as u64;

    bytes.extend_from_slice(&packed.to_be_bytes());
    bytes.extend_from_slice(&[0; 16]);

    let mut offsets = Vec::new();
    let mut start = 0;

    for (index, block) in blocks.iter().enumerate() {
        let number = if variable { start } else { index };
        let mut header = vec![
            0xFF,
            0xF8 | u8::from(variable),
            0x79,
            ((CHANNELS as u8 - 1) << 4) | (4 << 1),
        ];

        header.extend(coded_number(number as u64));
        header.extend_from_slice(&(*block as u16 - 1).to_be_bytes());
        header.push(crc_8(&header));

        let mut writer = BitWriter {
            bytes: header,
            used: 8,
        };

        for channel in signal {
            subframe(&mut writer, &channel[start..start + block]);
        }

        let mut frame = writer.bytes;
        let crc = crc_16(&frame);

        frame.extend_from_slice(&crc.to_be_bytes());
        offsets.push(bytes.len());
        bytes.extend(frame);
        start += block;
    }

    (bytes, offsets)
}

fn signal(length: usize, noise: i32) -> Vec<Vec<i32>> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;

    (0..CHANNELS)
        .map(|channel| {
            (0..length)
                .map(|index| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;

                    let tone =
                        (2_000.0 * (index as f64 / (40.0 + channel as f64 * 7.0)).sin()) as i32;

                    tone + (state % (2 * noise as u64 + 1)) as i32 - noise
                })
                .collect()
        })
        .collect()
}

fn reference(signal: &[Vec<i32>]) -> Vec<i32> {
    (0..signal[0].len())
        .flat_map(|index| signal.iter().map(move |channel| channel[index] << 16))
        .collect()
}

fn prime(decoder: &mut Flac, input: &[u8]) {
    let mut output = vec![0; 4096];
    let progress = decoder.decode(&input[..HEADER], &mut output).unwrap();

    assert_eq!(progress.consumed, HEADER);
    assert!(decoder.stream_information().is_some());
}

fn read(
    decoder: &mut Flac,
    input: &[u8],
    offset: u64,
    chunk: usize,
    wanted: usize,
) -> Result<Vec<i32>> {
    let mut output = vec![0; 4096];
    let mut decoded = Vec::new();
    let mut position = (offset as usize).min(input.len());
    let mut end = position;

    while decoded.len() < wanted {
        end = end.max(position.saturating_add(chunk).min(input.len()));

        if end == input.len() {
            decoder.finish();
        }

        let progress = decoder.decode(&input[position..end], &mut output)?;

        decoded.extend_from_slice(&output[..progress.produced]);
        position += progress.consumed;

        if progress.is_stalled() {
            if end == input.len() {
                break;
            }

            end = end.saturating_add(chunk).min(input.len());
        }
    }

    decoded.truncate(wanted);

    Ok(decoded)
}

fn seek(
    decoder: &mut Flac,
    input: &[u8],
    target: u64,
    chunk: usize,
    wanted: usize,
) -> Result<Vec<i32>> {
    let mut offset = decoder.seek(target)?.byte_offset;

    for _ in 0..64 {
        match read(decoder, input, offset, chunk, wanted) {
            Err(Error::SeekTo(next)) => offset = next,
            result => return result,
        }
    }

    panic!("too many round trips");
}

fn check(input: &[u8], expected: &[i32], targets: &[u64]) {
    let frames = (expected.len() / CHANNELS) as u64;

    for &target in targets {
        for chunk in [7, 1000, usize::MAX] {
            for length in [false, true] {
                let mut decoder = Flac::new();

                if length {
                    decoder.set_stream_length(input.len() as u64);
                }

                prime(&mut decoder, input);

                let wanted = 3 * BLOCK * CHANNELS;
                let decoded = seek(&mut decoder, input, target, chunk, wanted).unwrap();
                let start = target.min(frames) as usize * CHANNELS;
                let end = (start + wanted).min(expected.len());

                assert_eq!(
                    decoded,
                    &expected[start..end],
                    "frame {target}, chunk {chunk}, length {length}"
                );
            }
        }
    }
}

fn quiet(blocks: usize) -> (Vec<Vec<i32>>, Vec<usize>) {
    (signal(blocks * BLOCK, 1), vec![BLOCK; blocks])
}

fn varying(count: usize) -> Vec<usize> {
    (0..count)
        .map(|index| [4096, 1152, 256, 3000, 17, 2048, 4000][index % 7])
        .collect()
}

#[test]
fn skips_long_distances() {
    let (signal, blocks) = quiet(120);
    let (input, _) = encode(&signal, &blocks, false);
    let expected = reference(&signal);
    let frames = (BLOCK * 120) as u64;
    let targets: Vec<u64> = [
        1,
        BLOCK as u64 * 7 + 3,
        frames / 3,
        frames / 2 + 1,
        frames - BLOCK as u64 - 1,
        frames - 1,
        frames,
    ]
    .into_iter()
    .chain((1..12).map(|index| index * frames / 12 + index * 31))
    .collect();

    check(&input, &expected, &targets);
}

#[test]
fn skips_variable_block_sizes() {
    for variable in [true, false] {
        let blocks = if variable {
            varying(140)
        } else {
            vec![BLOCK; 100]
        };
        let total: usize = blocks.iter().sum();
        let signal = signal(total, 3);
        let (input, _) = encode(&signal, &blocks, variable);
        let expected = reference(&signal);
        let mut boundaries = vec![0u64];

        for block in &blocks {
            boundaries.push(boundaries.last().unwrap() + *block as u64);
        }

        let targets: Vec<u64> = boundaries
            .iter()
            .step_by(9)
            .flat_map(|boundary| [boundary.saturating_sub(1), *boundary, boundary + 1])
            .filter(|target| *target <= total as u64)
            .collect();

        check(&input, &expected, &targets);
    }
}

fn corrupt_body(input: &mut [u8], offsets: &[usize], frame: usize) {
    let start = offsets[frame];
    let end = offsets.get(frame + 1).copied().unwrap_or(input.len());
    let position = (start + end) / 2;

    input[position] = if input[position] == 0xFE {
        0x01
    } else {
        input[position] ^ 0x01
    };
}

#[test]
fn ignores_corruption_in_skipped_frames() {
    let (signal, blocks) = quiet(60);
    let (clean, offsets) = encode(&signal, &blocks, false);
    let expected = reference(&signal);

    for target_frame in [5, 30, 59] {
        let mut input = clean.clone();

        for frame in 0..target_frame {
            corrupt_body(&mut input, &offsets, frame);
        }

        let target = (target_frame * BLOCK + 100) as u64;

        check(&input, &expected, &[target]);
    }
}

#[test]
fn never_returns_wrong_samples_around_corrupt_headers() {
    let (signal, blocks) = quiet(60);
    let (clean, offsets) = encode(&signal, &blocks, false);
    let expected = reference(&signal);

    for corrupted in [10, 28, 29] {
        for byte in [2, 4, 5] {
            let mut input = clean.clone();

            input[offsets[corrupted] + byte] ^= 0x10;

            for length in [false, true] {
                let mut decoder = Flac::new();

                if length {
                    decoder.set_stream_length(input.len() as u64);
                }

                prime(&mut decoder, &input);

                let target = (30 * BLOCK + 5) as u64;
                let start = target as usize * CHANNELS;

                match seek(&mut decoder, &input, target, 1000, BLOCK * CHANNELS) {
                    Ok(decoded) => assert_eq!(decoded, &expected[start..start + BLOCK * CHANNELS]),
                    Err(error) => {
                        assert_eq!(error, Error::InvalidData, "frame {corrupted}, byte {byte}")
                    }
                }
            }
        }
    }
}

#[test]
fn checks_the_decoded_frame() {
    let (signal, blocks) = quiet(60);
    let (mut input, offsets) = encode(&signal, &blocks, false);

    corrupt_body(&mut input, &offsets, 40);

    for length in [false, true] {
        let mut decoder = Flac::new();

        if length {
            decoder.set_stream_length(input.len() as u64);
        }

        prime(&mut decoder, &input);

        assert_eq!(
            seek(
                &mut decoder,
                &input,
                (40 * BLOCK + 9) as u64,
                usize::MAX,
                BLOCK * CHANNELS
            ),
            Err(Error::InvalidData)
        );
    }
}

#[test]
#[ignore = "benchmark, run with --release -- --ignored --nocapture"]
fn seek_benchmark() {
    let blocks = 3 * 60 * 44_100 / BLOCK;

    for (name, noise) in [("quiet", 1), ("loud", 2_000)] {
        let signal = signal(blocks * BLOCK, noise);
        let (input, _) = encode(&signal, &vec![BLOCK; blocks], false);
        let expected = reference(&signal);
        let frames = (blocks * BLOCK) as u64;
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let targets: Vec<u64> = (0..300)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state % frames
            })
            .collect();

        for (chunk, label) in [(usize::MAX, "whole input"), (64 * 1024, "64 KiB reads")] {
            let mut decoder = Flac::new();

            decoder.set_stream_length(input.len() as u64);
            prime(&mut decoder, &input);

            let started = Instant::now();

            for target in &targets {
                let decoded = seek(&mut decoder, &input, *target, chunk, CHANNELS).unwrap();
                let start = *target as usize * CHANNELS;

                assert_eq!(decoded, &expected[start..start + CHANNELS]);
            }

            let elapsed = started.elapsed();

            println!(
                "{name} ({} KiB), {label}: {:.1} us per seek",
                input.len() / 1024,
                elapsed.as_secs_f64() * 1e6 / targets.len() as f64
            );
        }
    }
}
