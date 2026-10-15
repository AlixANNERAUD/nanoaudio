use nanoaudio_core::codec::Decoder;
use nanoaudio_core::{Configuration, Format, I24, Sample};
use nanoaudio_dsp::{
    Fixed, Float, Representation, Stream, Volume, convert_bytes, fill_silence, mix_bytes,
    source_using,
};
use nanoaudio_wav::{WavDecoder, WavHeader};

const FILE: &[u8] = include_bytes!("data/pcm_24.wav");
const EXTENSIBLE_FILE: &[u8] = include_bytes!("data/pcm_24_extensible.wav");
const DECODED: &[u8] = include_bytes!("data/pcm_24_decoded.bin");

fn expected() -> Vec<i32> {
    DECODED
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| i32::from_le_bytes(*chunk))
        .collect()
}

fn data(file: &[u8]) -> (WavHeader, &[u8]) {
    let header = WavHeader::parse(file).unwrap();
    let start = header.data_offset;

    (header, &file[start..start + header.data_size as usize])
}

fn decode<S: Sample>(file: &[u8], window: usize) -> Vec<S> {
    let (header, mut input) = data(file);
    let mut decoder = WavDecoder::<S>::new(&header).unwrap();
    let mut output = vec![S::SILENCE; decoder.minimum_output() * 7];
    let mut decoded = Vec::new();

    while !input.is_empty() {
        let end = window.min(input.len());
        let progress = decoder.decode(&input[..end], &mut output).unwrap();

        if progress.is_stalled() {
            break;
        }

        decoded.extend_from_slice(&output[..progress.produced]);
        input = &input[progress.consumed..];
    }

    assert!(input.is_empty());
    assert_eq!(
        decoded.len(),
        header.frames() * header.configuration.channels as usize
    );
    decoded
}

#[test]
fn header_matches_reference() {
    let (header, bytes) = data(FILE);

    assert_eq!(
        header.configuration,
        Configuration::new(48_000, 2, Format::I24)
    );
    assert_eq!(header.frames(), 401);
    assert_eq!(bytes.len(), 401 * 6);

    let (header, bytes) = data(EXTENSIBLE_FILE);

    assert_eq!(
        header.configuration,
        Configuration::new(48_000, 1, Format::I24)
    );
    assert_eq!(header.frames(), 401);
    assert_eq!(bytes.len(), 401 * 3);
}

#[test]
fn wav_decoder_matches_reference() {
    let expected = expected();

    for window in [6, 7, 100, FILE.len()] {
        assert_eq!(decode::<i32>(FILE, window), expected);
    }

    let samples: Vec<i32> = decode::<I24>(FILE, 1000)
        .into_iter()
        .map(i32::from)
        .collect();
    let values: Vec<i32> = expected.iter().map(|value| value >> 8).collect();
    assert_eq!(samples, values);

    let mono: Vec<i32> = expected.iter().step_by(2).copied().collect();
    assert_eq!(decode::<i32>(EXTENSIBLE_FILE, 31), mono);
}

fn stream_matches_reference<R: Representation>() {
    let (_, bytes) = data(FILE);
    let expected = expected();
    let mut output = vec![0i32; expected.len()];

    let frames = source_using::<R>(bytes, Format::I24, 2)
        .unwrap()
        .write_samples(&mut output);

    assert_eq!(frames, 401);
    assert_eq!(output, expected);

    let mut copy = vec![0u8; bytes.len()];
    let frames = source_using::<R>(bytes, Format::I24, 2)
        .unwrap()
        .write_bytes(&mut copy, Format::I24);

    assert_eq!(frames, 401);
    assert_eq!(copy, bytes);
}

#[test]
fn stream_matches_reference_fixed() {
    stream_matches_reference::<Fixed>();
}

#[test]
fn stream_matches_reference_float() {
    stream_matches_reference::<Float>();
}

#[test]
fn bytes_round_trip() {
    let (_, bytes) = data(FILE);
    let count = bytes.len() / 3;

    let mut wide = vec![0u8; count * 4];
    assert_eq!(
        convert_bytes(bytes, Format::I24, &mut wide, Format::I32),
        count
    );
    assert_eq!(wide, DECODED);

    let mut narrow = vec![0u8; bytes.len()];
    assert_eq!(
        convert_bytes(&wide, Format::I32, &mut narrow, Format::I24),
        count
    );
    assert_eq!(narrow, bytes);

    let mut float = vec![0u8; count * 4];
    convert_bytes(bytes, Format::I24, &mut float, Format::F32);
    narrow.fill(0);
    convert_bytes(&float, Format::F32, &mut narrow, Format::I24);
    assert_eq!(narrow, bytes);
}

#[test]
fn volume_and_mix_bytes() {
    let mut output = [0xAAu8; 6];
    fill_silence(&mut output, Format::I24);
    assert_eq!(output, [0; 6]);

    let mut input = [0u8; 6];
    I24::new(0x40_0000).write_le(&mut input[..3]);
    I24::new(-1000).write_le(&mut input[3..]);

    assert_eq!(
        mix_bytes(&mut output, &input, Format::I24, Volume::UNITY),
        2
    );
    assert_eq!(
        mix_bytes(&mut output, &input, Format::I24, Volume::UNITY),
        2
    );
    assert_eq!(I24::read_le(&output[..3]), I24::MAX);
    assert_eq!(I24::read_le(&output[3..]).value(), -2000);

    Volume::from_percent(50).apply_to_bytes(&mut output, Format::I24);
    assert_eq!(I24::read_le(&output[3..]).value(), -1000);
}
