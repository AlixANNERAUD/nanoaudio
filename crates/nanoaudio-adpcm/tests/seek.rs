use core::fmt::Debug;

use nanoaudio_adpcm::{ImaAdpcm, MsAdpcm};
use nanoaudio_core::codec::{Decoder, Encoder, Seek, SeekRequest};
use nanoaudio_core::{Error, Sample};

fn feed<D: Decoder>(decoder: &mut D, input: &[u8], chunk: usize) -> Vec<D::Sample> {
    let mut output = vec![D::Sample::SILENCE; decoder.minimum_output()];
    let mut decoded = Vec::new();
    let mut position = 0;
    let mut end = chunk.min(input.len());

    loop {
        if end == input.len() {
            decoder.finish();
        }

        let progress = decoder.decode(&input[position..end], &mut output).unwrap();

        decoded.extend_from_slice(&output[..progress.produced]);
        position += progress.consumed;

        if progress.is_stalled() {
            if end == input.len() {
                return decoded;
            }

            end = (end + chunk).min(input.len());
        }
    }
}

fn prime<D: Decoder>(decoder: &mut D, input: &[u8]) {
    let mut output = vec![D::Sample::SILENCE; decoder.minimum_output() * 4];

    decoder
        .decode(&input[..input.len().min(256)], &mut output)
        .unwrap();
}

fn resume<D: Decoder + Seek>(
    decoder: &mut D,
    input: &[u8],
    frame: u64,
    chunk: usize,
) -> Vec<D::Sample> {
    let request = decoder.seek(frame).unwrap();
    let start = (request.byte_offset as usize).min(input.len());

    feed(decoder, &input[start..], chunk)
}

fn targets(frames: u64, block: u64) -> Vec<u64> {
    let last = frames / block * block;
    let mut targets = vec![0, 1, 2, frames / 2, frames.saturating_sub(1), frames];

    for multiple in 1..=3 {
        targets.extend([multiple * block - 1, multiple * block, multiple * block + 1]);
    }

    targets.extend([last.saturating_sub(1), last, last + 1, last + block / 2]);
    targets.retain(|target| *target <= frames);
    targets.sort_unstable();
    targets.dedup();
    targets
}

fn check<D: Decoder + Seek>(
    create: impl Fn() -> D,
    input: &[u8],
    frames: u64,
    block: u64,
    bounded: bool,
) where
    D::Sample: Debug,
{
    let mut decoder = create();
    let full = feed(&mut decoder, input, 7);
    let channels = decoder.configuration().unwrap().channels as usize;
    let slice = |frame: u64| &full[frame as usize * channels..];

    assert_eq!(full.len() as u64, frames * channels as u64);

    for target in targets(frames, block) {
        for chunk in [1, 5, 64, usize::MAX] {
            let mut decoder = create();

            prime(&mut decoder, input);

            let decoded = resume(&mut decoder, input, target, chunk);

            assert_eq!(decoded, slice(target), "frame {target}, chunk {chunk}");
        }
    }

    for (first, second) in [
        (frames / 2, 1),
        (frames.saturating_sub(1), frames / 3),
        (frames, 0),
        (frames / 3, frames / 3),
        (1, frames / 2),
    ] {
        let mut decoder = create();

        prime(&mut decoder, input);

        let request = decoder.seek(first).unwrap();

        prime(
            &mut decoder,
            &input[(request.byte_offset as usize).min(input.len())..],
        );

        let decoded = resume(&mut decoder, input, second, 13);

        assert_eq!(decoded, slice(second), "frame {first} then {second}");
    }

    let mut decoder = create();

    prime(&mut decoder, input);

    if bounded {
        assert_eq!(decoder.seek(frames + 1), Err(Error::InvalidParameter));
        assert_eq!(decoder.seek(u64::MAX), Err(Error::InvalidParameter));
    } else {
        assert!(resume(&mut decoder, input, frames + 1, 3).is_empty());
    }

    assert!(resume(&mut decoder, input, frames, 3).is_empty());
}

fn signal(samples: usize) -> Vec<i16> {
    let mut state = 12_345u32;

    (0..samples)
        .map(|index| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345) & 0x7FFF_FFFF;

            let tone = if (index / 37) % 2 == 0 { 8_000 } else { -8_000 };

            (tone + (state >> 20) as i32 - 1_024) as i16
        })
        .collect()
}

fn encode<E: Encoder<Sample = i16>>(mut encoder: E, blocks: usize, size: usize) -> Vec<u8> {
    let input = signal(encoder.minimum_input() * blocks);
    let mut encoded = vec![0u8; encoder.minimum_output() * blocks];
    let progress = encoder.encode(&input, &mut encoded).unwrap();

    assert_eq!(progress.consumed, input.len());
    encoded.truncate(size.min(progress.produced));
    encoded
}

#[test]
fn ima_adpcm_seeks_to_exact_frames() {
    for channels in [1u8, 2] {
        let create = || ImaAdpcm::with_samples_per_block(8_000, channels, 65).unwrap();
        let block_align = create().block_align() as usize;
        let full = encode(create(), 6, usize::MAX);

        check(create, &full, 6 * 65, 65, false);

        let partial = 4 * channels as usize * 4;
        let truncated = encode(create(), 6, 5 * block_align + partial);

        check(create, &truncated, 5 * 65 + 3 * 8 + 1, 65, false);
    }
}

#[test]
fn ms_adpcm_seeks_to_exact_frames() {
    for channels in [1u8, 2] {
        let create = || MsAdpcm::with_samples_per_block(8_000, channels, 64).unwrap();
        let encoded = encode(create(), 6, usize::MAX);

        check(create, &encoded, 6 * 64, 64, false);
    }
}

#[test]
fn block_codecs_report_skipped_frames() {
    let mut ima = ImaAdpcm::with_samples_per_block(8_000, 2, 65).unwrap();
    let mut ms = MsAdpcm::with_samples_per_block(8_000, 1, 64).unwrap();

    assert_eq!(
        ima.seek(65 * 3 + 10),
        Ok(SeekRequest {
            byte_offset: 3 * ima.block_align() as u64,
            skip_frames: 10,
        })
    );
    assert_eq!(
        ms.seek(64 * 2 + 63),
        Ok(SeekRequest {
            byte_offset: 2 * ms.block_align() as u64,
            skip_frames: 63,
        })
    );
}
