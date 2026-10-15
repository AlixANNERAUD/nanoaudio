use nanoaudio_core::Error;
use nanoaudio_core::codec::{Decoder, Seek};
use nanoaudio_mp3::{MP3_DEFAULT_MINIMUM_INPUT, Mp3Decoder};

struct Fixture {
    free: &'static [u8],
    source: &'static [u8],
    windows: [usize; 2],
    block: u64,
    tagged: bool,
}

const FIXTURES: [Fixture; 2] = [
    Fixture {
        free: include_bytes!("data/free_cbr.mp3"),
        source: include_bytes!("data/gapless_cbr.mp3"),
        windows: [MP3_DEFAULT_MINIMUM_INPUT, 2 * 1_150 + 5],
        block: 1152,
        tagged: true,
    },
    Fixture {
        free: include_bytes!("data/free_low.mp3"),
        source: include_bytes!("data/seek_cbr_low.mp3"),
        windows: [MP3_DEFAULT_MINIMUM_INPUT, 1_445],
        block: 576,
        tagged: false,
    },
];

const CHUNKS: [usize; 4] = [1, 7, 512, usize::MAX];

const LIMIT: usize = 16 * 1152 * 2;

fn feed(decoder: &mut Mp3Decoder, input: &[u8], chunk: usize, limit: usize) -> Vec<i16> {
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

fn decode(input: &[u8], window: usize, gapless: bool, chunk: usize) -> (Vec<i16>, Option<u64>) {
    let mut decoder = Mp3Decoder::with_minimum_input(window);

    decoder.set_gapless(gapless);

    let decoded = feed(&mut decoder, input, chunk, usize::MAX);

    (decoded, decoder.frames())
}

fn junk(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index * 37 % 127) as u8).collect()
}

#[test]
fn decodes_like_the_standard_stream() {
    for fixture in &FIXTURES {
        for gapless in [true, false] {
            let (expected, frames) =
                decode(fixture.source, MP3_DEFAULT_MINIMUM_INPUT, gapless, 512);

            assert!(!expected.is_empty());
            assert_eq!(frames.is_some(), fixture.tagged);

            for window in fixture.windows {
                for chunk in CHUNKS {
                    let decoded = decode(fixture.free, window, gapless, chunk);

                    assert!(
                        decoded == (expected.clone(), frames),
                        "window {window}, chunk {chunk}, gapless {gapless}"
                    );
                }
            }
        }
    }
}

#[test]
fn finds_the_first_frame_after_junk() {
    for fixture in &FIXTURES {
        let (expected, _) = decode(fixture.free, MP3_DEFAULT_MINIMUM_INPUT, true, 512);

        for length in [1, 1_000, 2_000, 2_300, 3_000, 5_000] {
            for window in fixture.windows {
                let mut input = junk(length);

                input.extend_from_slice(fixture.free);

                for chunk in [1, 512] {
                    let (decoded, _) = decode(&input, window, true, chunk);

                    assert!(
                        decoded == expected,
                        "junk {length}, window {window}, chunk {chunk}"
                    );
                }
            }
        }
    }
}

fn targets(frames: u64, block: u64) -> Vec<u64> {
    let mut targets = vec![0, 1, 2, frames / 3, frames / 2, frames - 1, frames];

    for multiple in [1, 2, 3, 8, 12, 20] {
        targets.extend([multiple * block - 1, multiple * block, multiple * block + 1]);
    }

    targets.retain(|target| *target <= frames);
    targets.sort_unstable();
    targets.dedup();
    targets
}

#[test]
fn seeks_to_exact_frames() {
    for fixture in &FIXTURES {
        for gapless in [true, false] {
            let mut reference = Mp3Decoder::with_minimum_input(fixture.free.len());

            reference.set_gapless(gapless);

            let full = feed(&mut reference, fixture.free, 7, usize::MAX);
            let channels = reference.configuration().unwrap().channels as usize;
            let frames = (full.len() / channels) as u64;

            for window in fixture.windows {
                for (index, target) in targets(frames, fixture.block).into_iter().enumerate() {
                    let chunk = CHUNKS[index % CHUNKS.len()];
                    let mut decoder = Mp3Decoder::with_minimum_input(window);

                    decoder.set_gapless(gapless);
                    feed(&mut decoder, fixture.free, 512, 1);

                    let request = decoder.seek(target).unwrap();
                    let start = (request.byte_offset as usize).min(fixture.free.len());
                    let decoded = feed(&mut decoder, &fixture.free[start..], chunk, LIMIT);
                    let expected = &full[target as usize * channels..];

                    assert!(
                        decoded == expected[..expected.len().min(LIMIT)],
                        "frame {target}, window {window}, chunk {chunk}, gapless {gapless}"
                    );
                }

                if fixture.tagged {
                    let mut decoder = Mp3Decoder::with_minimum_input(window);

                    decoder.set_gapless(gapless);
                    feed(&mut decoder, fixture.free, 512, 1);

                    assert_eq!(decoder.frames(), Some(frames));
                    assert_eq!(decoder.seek(frames + 1), Err(Error::InvalidParameter));
                }
            }
        }
    }
}
