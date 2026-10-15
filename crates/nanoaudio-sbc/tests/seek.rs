use nanoaudio_core::Error;
use nanoaudio_core::codec::{Decoder, Seek, SeekRequest};
use nanoaudio_sbc::{SbcDecoder, SbcParameters};

const FILES: [&[u8]; 6] = [
    include_bytes!("data/sbc_44100_joint_stereo.sbc"),
    include_bytes!("data/sbc_48000_stereo.sbc"),
    include_bytes!("data/sbc_16000_mono.sbc"),
    include_bytes!("data/sbc_32000_snr.sbc"),
    include_bytes!("data/sbc_44100_dual_channel.sbc"),
    include_bytes!("data/msbc.sbc"),
];

fn feed(decoder: &mut SbcDecoder, input: &[u8], chunk: usize) -> Vec<i16> {
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

        if progress.is_stalled() {
            if end == input.len() {
                return decoded;
            }

            end = (end + chunk).min(input.len());
        }
    }
}

fn prime(decoder: &mut SbcDecoder, input: &[u8]) {
    let mut output = vec![0; decoder.minimum_output() * 4];

    decoder
        .decode(&input[..input.len().min(1024)], &mut output)
        .unwrap();
}

fn resume(decoder: &mut SbcDecoder, input: &[u8], frame: u64, chunk: usize) -> Vec<i16> {
    let request = decoder.seek(frame).unwrap();
    let start = (request.byte_offset as usize).min(input.len());

    feed(decoder, &input[start..], chunk)
}

fn targets(frames: u64, block: u64) -> Vec<u64> {
    let mut targets = vec![0, 1, 2, frames / 2, frames - 1, frames];

    for multiple in 1..=3 {
        targets.extend([multiple * block - 1, multiple * block, multiple * block + 1]);
    }

    targets.extend([frames - block - 1, frames - block, frames - block + 1]);
    targets.retain(|target| *target <= frames);
    targets.sort_unstable();
    targets.dedup();
    targets
}

fn parameters(file: &[u8]) -> SbcParameters {
    SbcParameters::parse(file).unwrap()
}

fn check(input: &[u8]) {
    let parameters = parameters(input);
    let channels = parameters.channels() as usize;
    let block = parameters.samples_per_channel() as u64;
    let frames = (input.len() / parameters.frame_length()) as u64 * block;
    let full = feed(&mut SbcDecoder::new(), input, 7);
    let slice = |frame: u64| &full[frame as usize * channels..];

    assert_eq!(full.len() as u64, frames * channels as u64);

    for target in targets(frames, block) {
        for chunk in [1, 7, 512, usize::MAX] {
            let mut decoder = SbcDecoder::new();

            prime(&mut decoder, input);

            let decoded = resume(&mut decoder, input, target, chunk);

            assert_eq!(decoded, slice(target), "frame {target}, chunk {chunk}");
        }
    }

    for (first, second) in [
        (frames / 2, 1),
        (frames - 1, frames / 3),
        (frames, 0),
        (frames / 3, frames / 3),
        (1, frames / 2),
    ] {
        let mut decoder = SbcDecoder::new();

        prime(&mut decoder, input);

        let request = decoder.seek(first).unwrap();

        prime(
            &mut decoder,
            &input[(request.byte_offset as usize).min(input.len())..],
        );

        let decoded = resume(&mut decoder, input, second, 13);

        assert_eq!(decoded, slice(second), "frame {first} then {second}");
    }

    let mut decoder = SbcDecoder::new();

    prime(&mut decoder, input);

    assert!(resume(&mut decoder, input, frames + 1, 3).is_empty());
    assert!(resume(&mut decoder, input, frames + 10 * block, 3).is_empty());
    assert!(resume(&mut decoder, input, frames, 3).is_empty());
}

#[test]
fn seeks_to_exact_frames() {
    for file in FILES {
        check(file);
    }
}

#[test]
fn byte_offset_starts_at_stream_start() {
    let file = FILES[0];
    let length = parameters(file).frame_length() as u64;
    let mut decoder = SbcDecoder::new();

    prime(&mut decoder, file);

    assert_eq!(
        decoder.seek(5 * 128 + 3),
        Ok(SeekRequest {
            byte_offset: 4 * length,
            skip_frames: 131,
        })
    );
    assert_eq!(
        decoder.seek(72),
        Ok(SeekRequest {
            byte_offset: 0,
            skip_frames: 72,
        })
    );
}

#[test]
fn starts_early_enough_to_fill_the_synthesis_history() {
    let file = FILES[2];
    let length = parameters(file).frame_length() as u64;
    let mut decoder = SbcDecoder::new();

    prime(&mut decoder, file);

    assert_eq!(
        decoder.seek(131),
        Ok(SeekRequest {
            byte_offset: length,
            skip_frames: 83,
        })
    );
    assert_eq!(
        decoder.seek(132),
        Ok(SeekRequest {
            byte_offset: 2 * length,
            skip_frames: 36,
        })
    );
}

#[test]
fn seeking_requires_a_parsed_header() {
    let file = FILES[0];
    let mut decoder = SbcDecoder::new();

    assert_eq!(decoder.seek(1), Err(Error::Incomplete));
    assert_eq!(decoder.seek(0), Ok(SeekRequest::default()));
    assert_eq!(
        feed(&mut decoder, file, 512),
        feed(&mut SbcDecoder::new(), file, 512)
    );
}
