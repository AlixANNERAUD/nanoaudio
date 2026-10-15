use nanoaudio_core::codec::{Decoder, Seek, SeekRequest};
use nanoaudio_core::{Error, Result};
use nanoaudio_flac::FlacDecoder;

type Flac = FlacDecoder<i32, 4608, 6>;

const FILES: [&[u8]; 16] = [
    include_bytes!("data/flac_seek_mono.flac"),
    include_bytes!("data/flac_seek_variable.flac"),
    include_bytes!("data/flac_seek_table.flac"),
    include_bytes!("data/flac_mono_16.flac"),
    include_bytes!("data/flac_mono_8.flac"),
    include_bytes!("data/flac_noise_16.flac"),
    include_bytes!("data/flac_silence_16.flac"),
    include_bytes!("data/flac_stereo_16.flac"),
    include_bytes!("data/flac_stereo_16_block_1000.flac"),
    include_bytes!("data/flac_stereo_16_level_0.flac"),
    include_bytes!("data/flac_stereo_16_level_8.flac"),
    include_bytes!("data/flac_stereo_16_metadata.flac"),
    include_bytes!("data/flac_stereo_24.flac"),
    include_bytes!("data/flac_stereo_32.flac"),
    include_bytes!("data/flac_surround_16.flac"),
    include_bytes!("data/flac_wasted_24.flac"),
];

const MAXIMUM_ROUNDS: usize = 12;

fn feed(decoder: &mut Flac, input: &[u8], chunk: usize) -> Result<Vec<i32>> {
    let mut output = vec![0; 4096];
    let mut decoded = Vec::new();
    let mut position = 0;
    let mut end = chunk.min(input.len());

    loop {
        if end == input.len() {
            decoder.finish();
        }

        let progress = decoder.decode(&input[position..end], &mut output)?;

        decoded.extend_from_slice(&output[..progress.produced]);
        position += progress.consumed;

        if progress.is_stalled() {
            if end == input.len() {
                return Ok(decoded);
            }

            end = (end + chunk).min(input.len());
        }
    }
}

fn play(decoder: &mut Flac, input: &[u8], offset: u64, chunk: usize) -> (Vec<i32>, usize) {
    let mut start = offset;
    let mut rounds = 0;

    loop {
        match feed(decoder, &input[(start as usize).min(input.len())..], chunk) {
            Ok(decoded) => return (decoded, rounds),
            Err(Error::SeekTo(offset)) => {
                start = offset;
                rounds += 1;

                assert!(rounds <= MAXIMUM_ROUNDS, "too many round trips");
            }
            Err(error) => panic!("{error}"),
        }
    }
}

fn prime(decoder: &mut Flac, input: &[u8]) {
    let mut output = vec![0; 4096];
    let input = &input[..input.len().min(16_384)];
    let mut position = 0;

    loop {
        let progress = decoder.decode(&input[position..], &mut output).unwrap();

        position += progress.consumed;

        if progress.is_stalled() || position == input.len() {
            return;
        }
    }
}

fn resume(decoder: &mut Flac, input: &[u8], frame: u64, chunk: usize) -> (Vec<i32>, usize) {
    let request = decoder.seek(frame).unwrap();

    play(decoder, input, request.byte_offset, chunk)
}

fn create(input: &[u8], length: bool) -> Flac {
    let mut decoder = Flac::new();

    if length {
        decoder.set_stream_length(input.len() as u64);
    }

    decoder
}

fn targets(frames: u64, block: u64) -> Vec<u64> {
    let last = (frames - 1) / block * block;
    let previous = last.saturating_sub(1);
    let mut targets = vec![0, 1, 2, frames / 3, frames / 2, frames - 1, frames];

    for multiple in 1..=3 {
        targets.extend([multiple * block - 1, multiple * block, multiple * block + 1]);
    }

    targets.extend([previous, last, last + 1]);
    targets.retain(|target| *target <= frames);
    targets.sort_unstable();
    targets.dedup();
    targets
}

fn information(input: &[u8]) -> (u64, u64, usize) {
    let mut decoder = Flac::new();

    prime(&mut decoder, input);

    let information = decoder.stream_information().unwrap();

    (
        information.total_samples,
        information.maximum_block_size as u64,
        information.channels as usize,
    )
}

fn check(input: &[u8]) -> usize {
    let (frames, block, channels) = information(input);
    let full = feed(&mut Flac::new(), input, 7).unwrap();
    let slice = |frame: u64| &full[frame as usize * channels..];
    let mut rounds = 0;

    assert_eq!(full.len() as u64, frames * channels as u64);

    for target in targets(frames, block) {
        for chunk in [1, 7, 512, usize::MAX] {
            for length in [false, true] {
                let mut decoder = create(input, length);

                prime(&mut decoder, input);

                let (decoded, trips) = resume(&mut decoder, input, target, chunk);

                assert_eq!(
                    decoded,
                    slice(target),
                    "frame {target}, chunk {chunk}, length {length}"
                );
                rounds = rounds.max(trips);
            }
        }
    }

    for (first, second) in [
        (frames / 2, 1),
        (frames - 1, frames / 3),
        (frames, 0),
        (frames / 3, frames / 3),
        (1, frames / 2),
    ] {
        let mut decoder = create(input, false);

        prime(&mut decoder, input);

        let request = decoder.seek(first).unwrap();
        let (decoded, _) = play(&mut decoder, input, request.byte_offset, 512);

        assert_eq!(decoded, slice(first), "frame {first}");

        let (decoded, _) = resume(&mut decoder, input, second, 13);

        assert_eq!(decoded, slice(second), "frame {first} then {second}");
    }

    let mut decoder = create(input, false);

    prime(&mut decoder, input);
    decoder.seek(frames / 2).unwrap();

    let (decoded, _) = resume(&mut decoder, input, frames / 4, 64);

    assert_eq!(decoded, slice(frames / 4), "seek during a seek");
    assert_eq!(decoder.seek(frames + 1), Err(Error::InvalidParameter));
    assert_eq!(decoder.seek(u64::MAX), Err(Error::InvalidParameter));
    assert!(resume(&mut decoder, input, frames, 3).0.is_empty());

    rounds
}

fn without_total(input: &[u8]) -> Vec<u8> {
    let mut input = input.to_vec();
    let position = 4 + 4 + 13;

    input[position] &= 0xF0;
    input[position + 1..position + 5].fill(0);
    input
}

fn audio_offset(input: &[u8]) -> u64 {
    let mut position = 4;

    loop {
        let header = input[position];

        position += 4 + u32::from_be_bytes([
            0,
            input[position + 1],
            input[position + 2],
            input[position + 3],
        ]) as usize;

        if header & 0x80 != 0 {
            return position as u64;
        }
    }
}

#[test]
fn seeks_to_exact_frames() {
    for file in FILES {
        check(file);
    }
}

#[test]
fn needs_few_round_trips() {
    for file in &FILES[..3] {
        assert!(check(file) <= 6);
    }
}

#[test]
fn seeks_without_a_sample_count() {
    for file in &FILES[..3] {
        let input = without_total(file);
        let (frames, block, channels) = information(file);
        let full = feed(&mut Flac::new(), file, 512).unwrap();

        assert_eq!(information(&input).0, 0);

        for target in targets(frames, block) {
            for length in [false, true] {
                let mut decoder = create(&input, length);

                prime(&mut decoder, &input);

                let (decoded, _) = resume(&mut decoder, &input, target, 512);

                assert_eq!(
                    decoded,
                    &full[target as usize * channels..],
                    "frame {target}"
                );
            }
        }

        for target in [frames + 1, frames + 10 * block, u64::MAX] {
            let mut decoder = create(&input, false);

            prime(&mut decoder, &input);

            assert!(resume(&mut decoder, &input, target, 512).0.is_empty());
        }
    }
}

#[test]
fn uses_the_seek_table() {
    let file = FILES[2];
    let audio = audio_offset(file);
    let mut decoder = create(file, false);

    prime(&mut decoder, file);

    let request = decoder.seek(576 * 20 + 10).unwrap();
    let offset = u64::from_be_bytes(
        file[46 + 18 * 20 + 8..46 + 18 * 20 + 16]
            .try_into()
            .unwrap(),
    );

    assert_eq!(
        request,
        SeekRequest {
            byte_offset: audio + offset,
            skip_frames: 10,
        }
    );

    let (_, rounds) = play(&mut decoder, file, request.byte_offset, 512);

    assert_eq!(rounds, 0);
}

#[test]
fn byte_offset_starts_at_the_marker() {
    let file = FILES[11];
    let mut decoder = create(file, false);

    prime(&mut decoder, file);

    assert_eq!(
        decoder.seek(100),
        Ok(SeekRequest {
            byte_offset: audio_offset(file),
            skip_frames: 100,
        })
    );
}

#[test]
fn seeking_requires_the_metadata() {
    let file = FILES[11];
    let mut decoder = create(file, false);

    assert_eq!(decoder.seek(1), Err(Error::Incomplete));
    assert_eq!(decoder.seek(0), Ok(SeekRequest::default()));

    let mut output = vec![0; 4096];

    decoder.decode(&file[..1000], &mut output).unwrap();

    assert_eq!(decoder.seek(1), Err(Error::Incomplete));
    assert_eq!(decoder.seek(0), Ok(SeekRequest::default()));
    assert!(decoder.stream_information().is_none());
}
