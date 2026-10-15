use core::fmt::Debug;

use nanoaudio_aiff::{AiffDecoder, AiffHeader};
use nanoaudio_core::codec::{Decoder, Seek, SeekRequest};
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

fn check_file<S: Sample + Debug>(file: &[u8]) {
    let header = AiffHeader::parse(file).unwrap();
    let data = &file[header.data_offset..][..header.data_size as usize];

    check(
        || AiffDecoder::<S>::new(&header).unwrap(),
        data,
        header.frames() as u64,
        50,
        true,
    );
}

#[test]
fn pcm_seeks_to_exact_frames() {
    check_file::<i16>(include_bytes!("data/aiff_pcm_s8.aiff"));
    check_file::<i16>(include_bytes!("data/aiff_pcm_16.aiff"));
    check_file::<i32>(include_bytes!("data/aiff_pcm_24.aiff"));
    check_file::<i32>(include_bytes!("data/aiff_pcm_32.aiff"));
    check_file::<i16>(include_bytes!("data/aiff_none.aifc"));
    check_file::<i16>(include_bytes!("data/aiff_sowt_16.aifc"));
    check_file::<i32>(include_bytes!("data/aiff_sowt_32.aifc"));
    check_file::<f32>(include_bytes!("data/aiff_float.aifc"));
}

#[test]
fn g711_seeks_to_exact_frames() {
    check_file::<i16>(include_bytes!("data/aiff_alaw.aifc"));
    check_file::<f32>(include_bytes!("data/aiff_ulaw.aifc"));
}

#[test]
fn byte_offset_starts_at_data_offset() {
    let header = AiffHeader::parse(include_bytes!("data/aiff_pcm_24.aiff")).unwrap();
    let mut decoder = AiffDecoder::<i32>::new(&header).unwrap();

    assert_eq!(
        decoder.seek(7),
        Ok(SeekRequest {
            byte_offset: 7 * header.frame_size() as u64,
            skip_frames: 0,
        })
    );
}
