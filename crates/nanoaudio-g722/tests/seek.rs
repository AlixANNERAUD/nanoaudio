use nanoaudio_core::Sample;
use nanoaudio_core::codec::{Decoder, Seek, SeekRequest};
use nanoaudio_g722::{G722BitRate, G722Decoder};

const ENCODED: &[u8] = include_bytes!("data/g722_encoded.bin");
const BIT_RATES: [G722BitRate; 3] = [
    G722BitRate::Rate64000,
    G722BitRate::Rate56000,
    G722BitRate::Rate48000,
];
const SETTLING_FRAMES: usize = 4_800;
const SETTLED_ERROR: i32 = 1_024;

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

fn restarted(bit_rate: G722BitRate, frame: u64) -> Vec<i16> {
    let decoded = feed(
        &mut G722Decoder::new(bit_rate),
        &ENCODED[frame as usize / 2..],
        7,
    );

    decoded[frame as usize % 2..].to_vec()
}

fn frames() -> u64 {
    ENCODED.len() as u64 * 2
}

#[test]
fn seeks_to_exact_frame_positions() {
    for bit_rate in BIT_RATES {
        for target in targets(frames(), 1_000) {
            let expected = restarted(bit_rate, target);

            for chunk in [3, 4_096] {
                let mut decoder = G722Decoder::new(bit_rate);

                prime(&mut decoder, ENCODED);

                let decoded = resume(&mut decoder, ENCODED, target, chunk);

                assert_eq!(decoded, expected, "frame {target}, chunk {chunk}");
            }
        }
    }
}

#[test]
fn seeking_to_the_start_is_exact() {
    for bit_rate in BIT_RATES {
        let full = feed(&mut G722Decoder::new(bit_rate), ENCODED, 7);
        let mut decoder = G722Decoder::new(bit_rate);

        prime(&mut decoder, ENCODED);

        assert_eq!(resume(&mut decoder, ENCODED, 0, 5), full);
    }
}

#[test]
fn output_settles_after_the_reset() {
    for bit_rate in BIT_RATES {
        let full = feed(&mut G722Decoder::new(bit_rate), ENCODED, 7);

        for target in (0..frames()).step_by(397) {
            let mut decoder = G722Decoder::new(bit_rate);

            prime(&mut decoder, ENCODED);

            let decoded = resume(&mut decoder, ENCODED, target, usize::MAX);
            let expected = &full[target as usize..];
            let error = decoded
                .iter()
                .zip(expected)
                .skip(SETTLING_FRAMES)
                .map(|(decoded, expected)| (*decoded as i32 - *expected as i32).abs())
                .max()
                .unwrap_or(0);

            assert_eq!(decoded.len(), expected.len(), "frame {target}");
            assert!(error <= SETTLED_ERROR, "frame {target}, error {error}");
        }
    }
}

#[test]
fn seeks_twice_and_backwards() {
    let frames = frames();

    for (first, second) in [
        (frames / 2, 1),
        (frames - 1, frames / 3),
        (frames, 0),
        (frames / 3 + 1, frames / 3 + 1),
        (1, frames / 2),
    ] {
        let mut decoder = G722Decoder::default();

        prime(&mut decoder, ENCODED);

        let request = decoder.seek(first).unwrap();

        prime(
            &mut decoder,
            &ENCODED[(request.byte_offset as usize).min(ENCODED.len())..],
        );

        let decoded = resume(&mut decoder, ENCODED, second, 13);

        assert_eq!(
            decoded,
            restarted(G722BitRate::Rate64000, second),
            "frame {first} then {second}"
        );
    }
}

#[test]
fn seeking_at_or_after_the_end_produces_nothing() {
    let mut decoder = G722Decoder::default();

    prime(&mut decoder, ENCODED);

    assert!(resume(&mut decoder, ENCODED, frames(), 3).is_empty());
    assert!(resume(&mut decoder, ENCODED, frames() + 1, 3).is_empty());
    assert!(resume(&mut decoder, ENCODED, frames() + 2, 3).is_empty());
}

#[test]
fn byte_offset_counts_pairs() {
    let mut decoder = G722Decoder::default();

    assert_eq!(
        decoder.seek(21),
        Ok(SeekRequest {
            byte_offset: 10,
            skip_frames: 1,
        })
    );
    assert_eq!(
        decoder.seek(20),
        Ok(SeekRequest {
            byte_offset: 10,
            skip_frames: 0,
        })
    );
}
