use core::fmt::Debug;

use nanoaudio_core::codec::{Decoder, Encoder, Seek, SeekRequest};
use nanoaudio_core::{Error, Sample};
use nanoaudio_qoa::{QOA_FRAME_LENGTH, QoaDecoder, QoaEncoder, qoa_frame_size};

const FILES: [&[u8]; 2] = [
    include_bytes!("data/qoa_mono.qoa"),
    include_bytes!("data/qoa_stereo.qoa"),
];

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

fn total(file: &[u8]) -> u64 {
    u32::from_be_bytes([file[4], file[5], file[6], file[7]]) as u64
}

fn streaming(channels: u8, frames: usize) -> Vec<u8> {
    let input: Vec<i16> = (0..frames * channels as usize)
        .map(|index| ((index * 2_311) % 30_000) as i16 - 15_000)
        .collect();
    let mut encoder = QoaEncoder::<i16>::new(44_100, channels, 0).unwrap();
    let mut encoded = vec![0u8; 8 + qoa_frame_size(channels, QOA_FRAME_LENGTH) * 4];

    encoder.flush();

    let progress = encoder.encode(&input, &mut encoded).unwrap();

    assert_eq!(progress.consumed, input.len());
    encoded.truncate(progress.produced);
    encoded
}

#[test]
fn seeks_to_exact_frames() {
    for file in FILES {
        check(
            QoaDecoder::<i16>::new,
            file,
            total(file),
            QOA_FRAME_LENGTH as u64,
            true,
        );
        check(
            QoaDecoder::<f32>::new,
            file,
            total(file),
            QOA_FRAME_LENGTH as u64,
            true,
        );
    }
}

#[test]
fn streaming_files_seek_to_exact_frames() {
    for channels in [1, 2] {
        let file = streaming(channels, 2 * QOA_FRAME_LENGTH + 333);

        check(
            QoaDecoder::<i16>::new,
            &file,
            2 * QOA_FRAME_LENGTH as u64 + 333,
            QOA_FRAME_LENGTH as u64,
            false,
        );
    }
}

#[test]
fn byte_offset_starts_at_file_start() {
    let file = FILES[1];
    let mut decoder = QoaDecoder::<i16>::new();
    let frame_size = qoa_frame_size(2, QOA_FRAME_LENGTH) as u64;

    prime(&mut decoder, file);

    assert_eq!(
        decoder.seek(QOA_FRAME_LENGTH as u64 + 21),
        Ok(SeekRequest {
            byte_offset: 8 + frame_size,
            skip_frames: 21,
        })
    );
    assert_eq!(
        decoder.seek(total(file)),
        Ok(SeekRequest {
            byte_offset: file.len() as u64,
            skip_frames: 0,
        })
    );
}

#[test]
fn seeking_requires_a_parsed_header() {
    let file = FILES[0];
    let mut decoder = QoaDecoder::<i16>::new();

    assert_eq!(decoder.seek(1), Err(Error::Incomplete));
    assert_eq!(decoder.seek(0), Ok(SeekRequest::default()));

    let mut output = vec![0i16; decoder.minimum_output()];

    decoder.decode(&file[..8], &mut output).unwrap();

    assert_eq!(decoder.samples(), Some(total(file) as u32));
    assert_eq!(decoder.seek(1), Err(Error::Incomplete));
    assert_eq!(decoder.seek(0), Ok(SeekRequest::default()));
    assert_eq!(decoder.samples(), None);
}
