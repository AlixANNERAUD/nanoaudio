use core::fmt::Debug;

use nanoaudio_adpcm::ImaAdpcm;
use nanoaudio_core::codec::{Decoder, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Sample};
use nanoaudio_wav::{Encoding, WavDecoder, WavHeader};

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

fn block(header: &WavHeader) -> u64 {
    let Configuration {
        sample_rate,
        channels,
        ..
    } = header.configuration;

    match header.encoding {
        Encoding::ImaAdpcm { block_align } => ImaAdpcm::new(sample_rate, channels, block_align)
            .unwrap()
            .samples_per_block() as u64,
        Encoding::MsAdpcm {
            samples_per_block, ..
        } => samples_per_block as u64,
        _ => 100,
    }
}

fn check_file<S: Sample + Debug>(file: &[u8]) {
    let header = WavHeader::parse(file).unwrap();
    let data = &file[header.data_offset..][..header.data_size as usize];

    check(
        || WavDecoder::<S>::new(&header).unwrap(),
        data,
        header.frames() as u64,
        block(&header),
        true,
    );
}

fn g711_file(tag: u16, channels: u16, frames: usize) -> Vec<u8> {
    let data: Vec<u8> = (0..frames * channels as usize)
        .map(|index| (index * 37 % 256) as u8)
        .collect();
    let mut file = Vec::new();

    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(4 + 8 + 16 + 8 + data.len() as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&tag.to_le_bytes());
    file.extend_from_slice(&channels.to_le_bytes());
    file.extend_from_slice(&8_000u32.to_le_bytes());
    file.extend_from_slice(&(8_000 * channels as u32).to_le_bytes());
    file.extend_from_slice(&channels.to_le_bytes());
    file.extend_from_slice(&8u16.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(&data);
    file
}

#[test]
fn pcm_seeks_to_exact_frames() {
    check_file::<i16>(include_bytes!("data/pcm_24.wav"));
    check_file::<i32>(include_bytes!("data/pcm_24.wav"));
    check_file::<f32>(include_bytes!("data/pcm_24_extensible.wav"));
}

#[test]
fn g711_seeks_to_exact_frames() {
    for tag in [6, 7] {
        for channels in [1, 2] {
            check_file::<i16>(&g711_file(tag, channels, 777));
        }
    }
}

#[test]
fn ima_adpcm_seeks_to_exact_frames() {
    check_file::<i16>(include_bytes!("data/ima_adpcm_mono.wav"));
    check_file::<i16>(include_bytes!("data/ima_adpcm_stereo.wav"));
    check_file::<i16>(include_bytes!("data/ima_adpcm_mono_truncated.wav"));
    check_file::<i32>(include_bytes!("data/ima_adpcm_stereo_truncated.wav"));
}

#[test]
fn ms_adpcm_seeks_to_exact_frames() {
    check_file::<i16>(include_bytes!("data/ms_adpcm_mono.wav"));
    check_file::<i16>(include_bytes!("data/ms_adpcm_stereo.wav"));
    check_file::<f32>(include_bytes!("data/ms_adpcm_encoded_stereo.wav"));
}

#[test]
fn byte_offset_starts_at_data_offset() {
    let header = WavHeader::parse(include_bytes!("data/pcm_24.wav")).unwrap();
    let mut decoder = WavDecoder::<i16>::new(&header).unwrap();
    let frame_size = header.configuration.channels as u64 * 3;

    assert_eq!(
        decoder.seek(5),
        Ok(SeekRequest {
            byte_offset: 5 * frame_size,
            skip_frames: 0,
        })
    );
    assert_eq!(
        decoder.seek(header.frames() as u64),
        Ok(SeekRequest {
            byte_offset: header.data_size as u64,
            skip_frames: 0,
        })
    );
}
