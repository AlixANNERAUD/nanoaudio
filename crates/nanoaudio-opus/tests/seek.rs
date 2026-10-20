mod common;

use common::*;
use nanoaudio_core::Error;
use nanoaudio_core::codec::{Decoder, Seek};
use nanoaudio_opus::OpusDecoder;

fn seek_and_decode(
    decoder: &mut OpusDecoder,
    file: &[u8],
    frame: u64,
    chunk: usize,
) -> (Vec<i16>, usize) {
    let mut offset = decoder.seek(frame).unwrap().byte_offset as usize;
    let mut rounds = 0;
    let mut output = vec![0; 3_000];
    let mut pending = Vec::new();
    let mut fed = offset;
    let mut samples = Vec::new();
    let mut finished = false;

    loop {
        let progress = match decoder.decode(&pending, &mut output) {
            Err(Error::SeekTo(next)) => {
                rounds += 1;
                offset = next as usize;
                fed = offset.min(file.len());
                finished = false;
                pending.clear();
                continue;
            }
            result => result.unwrap(),
        };

        pending.drain(..progress.consumed);
        samples.extend_from_slice(&output[..progress.produced]);

        if progress.is_stalled() {
            if finished {
                return (samples, rounds);
            }

            let end = (fed + chunk).min(file.len());
            pending.extend_from_slice(&file[fed..end]);
            fed = end;

            if fed == file.len() {
                decoder.finish();
                finished = true;
            }
        }
    }
}

fn snr(samples: &[i16], expected: &[i16]) -> f64 {
    let signal: f64 = expected.iter().map(|&value| (value as f64).powi(2)).sum();
    let noise: f64 = samples
        .iter()
        .zip(expected)
        .map(|(&sample, &value)| (sample as f64 - value as f64).powi(2))
        .sum();

    10.0 * (signal / noise.max(1.0)).log10()
}

fn opened(file: &[u8]) -> Box<OpusDecoder> {
    let mut decoder = decoder::<i16>();
    let mut output = vec![0; 2];

    decoder.decode(&file[..1_000], &mut output).unwrap();
    decoder
}

#[test]
fn seeks_land_on_the_requested_frame() {
    let reference = decode::<i16>(LONG);
    let frames = (reference.len() / 2) as u64;

    for length in [false, true] {
        for frame in [
            0,
            1,
            960,
            3_000,
            47_999,
            48_000,
            100_003,
            150_000,
            191_000,
            frames - 1,
        ] {
            let mut decoder = opened(LONG);

            if length {
                decoder.set_stream_length(LONG.len() as u64);
            }

            let (samples, rounds) = seek_and_decode(&mut decoder, LONG, frame, 4_096);
            let expected = &reference[2 * frame as usize..];
            let snr = snr(&samples, expected);

            assert_eq!(samples.len(), expected.len(), "frame {frame}");
            assert!(snr > 45.0, "frame {frame}: {snr:.1} dB");
            assert!(rounds <= 8, "frame {frame}: {rounds} rounds");
        }
    }
}

#[test]
fn seeking_to_the_start_is_exact() {
    let reference = decode::<i16>(LONG);
    let mut decoder = decoder::<i16>();

    decode_with(&mut decoder, LONG, LONG.len(), 4_000).unwrap();

    let (samples, rounds) = seek_and_decode(&mut decoder, LONG, 0, 4_096);

    assert_eq!(rounds, 0);
    assert_eq!(samples, reference);
}

#[test]
fn seeking_after_a_full_decode_uses_the_length() {
    let reference = decode::<i16>(LONG);
    let mut decoder = decoder::<i16>();

    decode_with(&mut decoder, LONG, LONG.len(), 4_000).unwrap();

    let frames = decoder.frames().unwrap();

    assert_eq!(decoder.seek(frames + 1), Err(Error::InvalidParameter));

    let (samples, _) = seek_and_decode(&mut decoder, LONG, frames, 4_096);

    assert!(samples.is_empty());

    let (samples, _) = seek_and_decode(&mut decoder, LONG, 120_000, 4_096);

    assert_eq!(samples.len(), reference.len() - 240_000);
}

#[test]
fn seeking_before_the_headers() {
    let mut decoder = decoder::<i16>();

    assert_eq!(decoder.seek(10), Err(Error::Incomplete));
    assert_eq!(decoder.seek(0).unwrap().byte_offset, 0);
}

#[test]
fn seeks_in_short_files() {
    for file in [MONO, STEREO] {
        let reference = decode::<i16>(file);
        let channels = if file == MONO { 1 } else { 2 };

        for frame in [0, 500, 4_800, 9_000] {
            let mut decoder = opened(file);
            let (samples, _) = seek_and_decode(&mut decoder, file, frame, 1_000);
            let expected = &reference[channels * frame as usize..];

            assert_eq!(samples.len(), expected.len());
            assert!(snr(&samples, expected) > 60.0);
        }
    }
}
