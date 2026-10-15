use nanoaudio_core::Error;
use nanoaudio_core::codec::{Decoder, Seek, SeekRequest};
use nanoaudio_mp3::Mp3Decoder;

const CONSTANT: [(&[u8], bool); 4] = [
    (include_bytes!("data/mono_440.mp3"), false),
    (include_bytes!("data/stereo_440_1000.mp3"), false),
    (include_bytes!("data/seek_cbr_info.mp3"), true),
    (include_bytes!("data/seek_cbr_low.mp3"), false),
];

const VARIABLE: [&[u8]; 2] = [
    include_bytes!("data/seek_vbr_xing.mp3"),
    include_bytes!("data/seek_vbr_vbri.mp3"),
];

const WINDOW: usize = 16 * 1152 * 2;

fn feed(decoder: &mut Mp3Decoder, input: &[u8], chunk: usize) -> Vec<i16> {
    feed_until(decoder, input, chunk, usize::MAX)
}

fn feed_until(decoder: &mut Mp3Decoder, input: &[u8], chunk: usize, limit: usize) -> Vec<i16> {
    let mut output = vec![0; decoder.minimum_output()];
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

        if decoded.len() >= limit {
            decoded.truncate(limit);

            return decoded;
        }

        if progress.is_stalled() {
            if end == input.len() {
                return decoded;
            }

            end = (end + chunk).min(input.len());
        }
    }
}

fn prime(decoder: &mut Mp3Decoder, input: &[u8]) {
    let mut output = vec![0; decoder.minimum_output()];
    let prefix = &input[..input.len().min(20_000)];

    if prefix.len() < decoder.minimum_input() {
        decoder.finish();
    }

    decoder.decode(prefix, &mut output).unwrap();
}

fn resume(decoder: &mut Mp3Decoder, input: &[u8], frame: u64, chunk: usize) -> Vec<i16> {
    let request = decoder.seek(frame).unwrap();
    let start = (request.byte_offset as usize).min(input.len());

    feed_until(decoder, &input[start..], chunk, WINDOW)
}

fn window(samples: &[i16]) -> &[i16] {
    &samples[..samples.len().min(WINDOW)]
}

fn chunks(index: usize) -> &'static [usize] {
    const ALL: [usize; 4] = [1, 7, 512, usize::MAX];

    if index.is_multiple_of(4) {
        &ALL
    } else {
        &ALL[index % 4..index % 4 + 1]
    }
}

fn targets(frames: u64, block: u64) -> Vec<u64> {
    let last = (frames - 1) / block * block;
    let mut targets = vec![0, 1, 2, frames / 3, frames / 2, frames - 1, frames];

    for multiple in 1..=3 {
        targets.extend([multiple * block - 1, multiple * block, multiple * block + 1]);
    }

    for multiple in [8, 9, 12] {
        targets.extend([multiple * block - 1, multiple * block + 1]);
    }

    targets.extend([last - 1, last, last + 1, last - 10 * block]);
    targets.retain(|target| *target <= frames);
    targets.sort_unstable();
    targets.dedup();
    targets
}

struct Reference {
    full: Vec<i16>,
    channels: usize,
    block: u64,
    frames: u64,
}

fn reference(input: &[u8]) -> Reference {
    let mut decoder = Mp3Decoder::new();
    let full = feed(&mut decoder, input, 7);
    let configuration = decoder.configuration().unwrap();
    let channels = configuration.channels as usize;
    let block = if configuration.sample_rate >= 32_000 {
        1152
    } else {
        576
    };

    Reference {
        frames: (full.len() / channels) as u64,
        full,
        channels,
        block,
    }
}

fn check_constant(input: &[u8], bounded: bool) {
    let Reference {
        full,
        channels,
        block,
        frames,
    } = reference(input);
    let slice = |frame: u64| &full[frame as usize * channels..];

    assert_eq!(frames % block, 0);

    for (index, target) in targets(frames, block).into_iter().enumerate() {
        for &chunk in chunks(index) {
            let mut decoder = Mp3Decoder::new();

            prime(&mut decoder, input);

            let decoded = resume(&mut decoder, input, target, chunk);

            assert!(
                decoded == window(slice(target)),
                "frame {target}, chunk {chunk}: {} samples instead of {}",
                decoded.len(),
                window(slice(target)).len()
            );
        }
    }

    for (first, second) in [
        (frames / 2, 1),
        (frames - 1, frames / 3),
        (frames, 0),
        (frames / 3, frames / 3),
        (1, frames / 2),
    ] {
        let mut decoder = Mp3Decoder::new();

        prime(&mut decoder, input);

        let request = decoder.seek(first).unwrap();

        prime(
            &mut decoder,
            &input[(request.byte_offset as usize).min(input.len())..],
        );

        let decoded = resume(&mut decoder, input, second, 13);

        assert!(
            decoded == window(slice(second)),
            "frame {first} then {second}"
        );
    }

    let mut decoder = Mp3Decoder::new();

    prime(&mut decoder, input);

    if bounded {
        assert_eq!(decoder.seek(frames + 1), Err(Error::InvalidParameter));
        assert_eq!(decoder.seek(u64::MAX), Err(Error::InvalidParameter));
    } else {
        assert!(resume(&mut decoder, input, frames + 1, 3).is_empty());
        assert!(resume(&mut decoder, input, frames + 100 * block, 512).is_empty());
        assert!(resume(&mut decoder, input, u64::MAX, 512).is_empty());
    }

    assert!(resume(&mut decoder, input, frames, 3).is_empty());
}

fn aligned(full: &[i16], channels: usize, block: u64, target: u64, decoded: &[i16]) -> u64 {
    let within = target % block;
    let total = (full.len() / channels) as u64;

    (0..=total / block)
        .map(|index| index * block + within)
        .filter(|position| *position <= total)
        .filter(|position| decoded == window(&full[*position as usize * channels..]))
        .min_by_key(|position| position.abs_diff(target))
        .unwrap_or_else(|| panic!("frame {target}: the output matches no frame"))
}

fn check_variable(input: &[u8]) -> u64 {
    let Reference {
        full,
        channels,
        block,
        frames,
    } = reference(input);
    let mut error = 0;

    for (index, target) in targets(frames, block).into_iter().enumerate() {
        for &chunk in chunks(index) {
            let mut decoder = Mp3Decoder::new();

            prime(&mut decoder, input);

            let decoded = resume(&mut decoder, input, target, chunk);

            if target == frames {
                assert!(decoded.is_empty());

                continue;
            }

            let position = aligned(&full, channels, block, target, &decoded);

            error = error.max(position.abs_diff(target) / block);
        }
    }

    let mut decoder = Mp3Decoder::new();

    prime(&mut decoder, input);

    assert_eq!(decoder.seek(frames + 1), Err(Error::InvalidParameter));
    assert_eq!(decoder.seek(u64::MAX), Err(Error::InvalidParameter));

    for target in [frames / 2, frames / 5, 3 * block + 7] {
        let decoded = resume(&mut decoder, input, target, 512);
        let position = aligned(&full, channels, block, target, &decoded);

        error = error.max(position.abs_diff(target) / block);
    }

    error
}

#[test]
fn seeks_to_exact_frames_in_constant_bitrate_streams() {
    for (file, bounded) in CONSTANT {
        check_constant(file, bounded);
    }
}

#[test]
fn seeks_near_the_target_in_variable_bitrate_streams() {
    for file in VARIABLE {
        let Reference { block, frames, .. } = reference(file);
        let error = check_variable(file);

        assert!(error <= frames / block / 100 + 2, "{error} frames away");
    }
}

#[test]
fn byte_offset_starts_at_the_first_byte_fed() {
    let (file, _) = CONSTANT[2];
    let tag = 10 + 1200;
    let mut decoder = Mp3Decoder::new();

    prime(&mut decoder, file);

    assert_eq!(
        decoder.seek(100),
        Ok(SeekRequest {
            byte_offset: tag,
            skip_frames: 100,
        })
    );

    let request = decoder.seek(20 * 1152).unwrap();

    assert!(request.byte_offset > tag + 10 * 417);
    assert!(request.byte_offset < tag + 20 * 418);
}

#[test]
fn seeking_requires_a_parsed_frame() {
    let (file, _) = CONSTANT[1];
    let mut decoder = Mp3Decoder::new();

    assert_eq!(decoder.seek(1), Err(Error::Incomplete));
    assert_eq!(decoder.seek(0), Ok(SeekRequest::default()));
    assert_eq!(feed(&mut decoder, file, 512), reference(file).full);
}
