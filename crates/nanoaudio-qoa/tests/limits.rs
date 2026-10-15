use nanoaudio_core::codec::{Decoder, Encoder, Seek};
use nanoaudio_core::{Error, Sample};
use nanoaudio_qoa::{QOA_SLICE_LENGTH, QoaDecoder, QoaEncoder};

const MONO: &[u8] = include_bytes!("data/qoa_mono.qoa");
const STEREO: &[u8] = include_bytes!("data/qoa_stereo.qoa");

fn samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn decode<D: Decoder>(
    decoder: &mut D,
    file: &[u8],
    window: usize,
) -> Result<Vec<D::Sample>, Error> {
    let mut output = vec![D::Sample::SILENCE; decoder.minimum_output()];
    let mut decoded = Vec::new();
    let mut position = 0;
    let mut end = window.min(file.len());

    loop {
        if end == file.len() {
            decoder.finish();
        }

        let progress = decoder.decode(&file[position..end], &mut output)?;
        decoded.extend_from_slice(&output[..progress.produced]);
        position += progress.consumed;

        if !progress.is_stalled() {
            end = end.max((position + window).min(file.len()));
        } else if end == file.len() {
            return Ok(decoded);
        } else {
            end = (end + window).min(file.len());
        }
    }
}

fn encode<E: Encoder<Sample = i16>>(encoder: &mut E, input: &[i16]) -> Vec<u8> {
    let mut output = vec![0; input.len() * 2 + 1_024];
    let progress = encoder.encode(input, &mut output).unwrap();

    assert_eq!(progress.consumed, input.len());
    output.truncate(progress.produced);
    output
}

#[test]
fn limited_decoders_match_the_default() {
    let mono = decode(&mut QoaDecoder::<i16>::new(), MONO, 100).unwrap();
    let stereo = decode(&mut QoaDecoder::<i16>::new(), STEREO, 100).unwrap();

    assert_eq!(
        decode(&mut QoaDecoder::<i16, 1>::new_limited(), MONO, 100).unwrap(),
        mono
    );
    assert_eq!(
        decode(&mut QoaDecoder::<i16, 2>::new_limited(), MONO, 100).unwrap(),
        mono
    );
    assert_eq!(
        decode(&mut QoaDecoder::<i16, 2>::new_limited(), STEREO, 100).unwrap(),
        stereo
    );
    assert_eq!(
        decode(&mut QoaDecoder::<i16, 3>::new_limited(), STEREO, 7).unwrap(),
        stereo
    );
}

#[test]
fn limited_decoders_reject_more_channels() {
    assert_eq!(
        decode(&mut QoaDecoder::<i16, 1>::new_limited(), STEREO, 100),
        Err(Error::TooManyChannels)
    );
}

#[test]
fn limited_decoders_need_a_smaller_output() {
    assert_eq!(
        QoaDecoder::<i16>::new().minimum_output(),
        QOA_SLICE_LENGTH * 8
    );
    assert_eq!(
        QoaDecoder::<i16, 1>::new_limited().minimum_output(),
        QOA_SLICE_LENGTH
    );
    assert_eq!(
        QoaDecoder::<f32, 2>::new_limited().minimum_output(),
        QOA_SLICE_LENGTH * 2
    );
}

#[test]
fn limited_decoders_seek() {
    let mut decoder = QoaDecoder::<i16, 2>::new_limited();
    let mut output = [0i16; QOA_SLICE_LENGTH * 2];
    let expected = decode(&mut QoaDecoder::<i16>::new(), STEREO, 100).unwrap();

    decoder.decode(STEREO, &mut output).unwrap();

    let frame = 5_130;
    let request = decoder.seek(frame).unwrap();
    let decoded = decode(&mut decoder, &STEREO[request.byte_offset as usize..], 100).unwrap();

    assert_eq!(decoded, expected[frame as usize * 2..]);
}

#[test]
fn limited_encoders_match_the_default() {
    let mono = samples(include_bytes!("data/qoa_mono_input.bin"));
    let stereo = samples(include_bytes!("data/qoa_stereo_input.bin"));
    let mono_frames = mono.len() as u32;
    let stereo_frames = stereo.len() as u32 / 2;

    assert_eq!(
        encode(
            &mut QoaEncoder::<i16, 1>::new_limited(16_000, 1, mono_frames).unwrap(),
            &mono
        ),
        MONO
    );
    assert_eq!(
        encode(
            &mut QoaEncoder::<i16, 2>::new_limited(16_000, 1, mono_frames).unwrap(),
            &mono
        ),
        MONO
    );
    assert_eq!(
        encode(
            &mut QoaEncoder::<i16, 2>::new_limited(44_100, 2, stereo_frames).unwrap(),
            &stereo
        ),
        STEREO
    );
}

#[test]
fn limited_encoders_reject_more_channels() {
    assert_eq!(
        QoaEncoder::<i16, 1>::new_limited(16_000, 2, 0).unwrap_err(),
        Error::TooManyChannels
    );
    assert_eq!(
        QoaEncoder::<i16, 2>::new_limited(16_000, 3, 0).unwrap_err(),
        Error::TooManyChannels
    );
    assert_eq!(
        QoaEncoder::<i16, 2>::new_limited(16_000, 0, 0).unwrap_err(),
        Error::TooManyChannels
    );
}

#[test]
fn limited_codecs_are_smaller() {
    assert!(size_of::<QoaDecoder<i16, 2>>() < size_of::<QoaDecoder<i16>>());
    assert!(size_of::<QoaDecoder<i16, 1>>() < size_of::<QoaDecoder<i16, 2>>());
    assert!(size_of::<QoaEncoder<i16, 2>>() < size_of::<QoaEncoder<i16>>());
    assert!(size_of::<QoaEncoder<i16, 1>>() < size_of::<QoaEncoder<i16, 2>>());
}
