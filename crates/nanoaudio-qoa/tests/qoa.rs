use nanoaudio_core::codec::{Decoder, Encoder};
use nanoaudio_core::{Configuration, Error, Format, Sample};
use nanoaudio_qoa::{QoaDecoder, QoaEncoder};

const MONO: &[u8] = include_bytes!("data/qoa_mono.qoa");
const STEREO: &[u8] = include_bytes!("data/qoa_stereo.qoa");

struct Fixture {
    encoded: &'static [u8],
    input: Vec<i16>,
    decoded: Vec<i16>,
    sample_rate: u32,
    channels: u8,
}

fn samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn fixtures() -> [Fixture; 2] {
    [
        Fixture {
            encoded: MONO,
            input: samples(include_bytes!("data/qoa_mono_input.bin")),
            decoded: samples(include_bytes!("data/qoa_mono_decoded.bin")),
            sample_rate: 16_000,
            channels: 1,
        },
        Fixture {
            encoded: STEREO,
            input: samples(include_bytes!("data/qoa_stereo_input.bin")),
            decoded: samples(include_bytes!("data/qoa_stereo_decoded.bin")),
            sample_rate: 44_100,
            channels: 2,
        },
    ]
}

fn decode<S: Sample>(
    decoder: &mut QoaDecoder<S>,
    file: &[u8],
    window: usize,
    output_size: usize,
) -> Result<Vec<S>, (Error, Vec<S>)> {
    let mut output = vec![S::SILENCE; output_size];
    let mut decoded = Vec::new();
    let mut position = 0;
    let mut end = window.min(file.len());

    loop {
        if end == file.len() {
            decoder.finish();
        }

        let progress = match decoder.decode(&file[position..end], &mut output) {
            Ok(progress) => progress,
            Err(error) => return Err((error, decoded)),
        };
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

fn encode(
    encoder: &mut QoaEncoder<i16>,
    input: &[i16],
    window: usize,
    output_size: usize,
) -> Vec<u8> {
    let mut output = vec![0; output_size];
    let mut encoded = Vec::new();
    let mut position = 0;
    let mut end = window.min(input.len());

    loop {
        if end == input.len() {
            encoder.flush();
        }

        let progress = encoder.encode(&input[position..end], &mut output).unwrap();
        encoded.extend_from_slice(&output[..progress.produced]);
        position += progress.consumed;

        if !progress.is_stalled() {
            end = end.max((position + window).min(input.len()));
        } else if end == input.len() {
            return encoded;
        } else {
            end = (end + window).min(input.len());
        }
    }
}

fn streaming(file: &[u8]) -> Vec<u8> {
    let mut file = file.to_vec();
    file[4..8].fill(0);
    file
}

#[test]
fn decoder_matches_reference() {
    for fixture in fixtures() {
        let mut decoder = QoaDecoder::<i16>::new();
        assert_eq!(decoder.configuration(), None);

        let decoded = decode(
            &mut decoder,
            fixture.encoded,
            fixture.encoded.len(),
            1 << 16,
        )
        .unwrap();

        assert_eq!(decoded, fixture.decoded);
        assert_eq!(
            decoder.configuration(),
            Some(Configuration::new(
                fixture.sample_rate,
                fixture.channels,
                Format::I16
            ))
        );
        assert_eq!(
            decoder.samples(),
            Some((fixture.input.len() / fixture.channels as usize) as u32)
        );
        assert!(!decoder.is_streaming());
    }
}

#[test]
fn decoder_streams_small_chunks() {
    for fixture in fixtures() {
        for window in [1, 3, 8, 13, 100, 1_000] {
            let mut decoder = QoaDecoder::<i16>::new();
            let minimum = decoder.minimum_output();
            let decoded = decode(&mut decoder, fixture.encoded, window, minimum).unwrap();

            assert_eq!(decoded, fixture.decoded, "window {window}");
        }
    }
}

#[test]
fn decoder_handles_streaming_files() {
    for fixture in fixtures() {
        let file = streaming(fixture.encoded);
        let mut decoder = QoaDecoder::<i16>::new();
        let decoded = decode(&mut decoder, &file, 37, 1_000).unwrap();

        assert_eq!(decoded, fixture.decoded);
        assert_eq!(decoder.samples(), Some(0));
        assert!(decoder.is_streaming());
    }
}

#[test]
fn decoder_converts_to_f32() {
    for fixture in fixtures() {
        let mut decoder = QoaDecoder::<f32>::new();
        let decoded = decode(&mut decoder, fixture.encoded, 512, 4_096).unwrap();
        let expected: Vec<f32> = fixture
            .decoded
            .iter()
            .map(|sample| *sample as f32 / 32_768.0)
            .collect();

        assert_eq!(decoded, expected);
        assert_eq!(decoder.configuration().unwrap().format, Format::F32);
    }
}

#[test]
fn encoder_matches_reference() {
    for fixture in fixtures() {
        let frames = (fixture.input.len() / fixture.channels as usize) as u32;
        let mut encoder =
            QoaEncoder::<i16>::new(fixture.sample_rate, fixture.channels, frames).unwrap();
        let encoded = encode(&mut encoder, &fixture.input, fixture.input.len(), 1 << 16);

        assert_eq!(encoded, fixture.encoded);
        assert!(encoder.is_finished());
    }
}

#[test]
fn encoder_streams_small_chunks() {
    for fixture in fixtures() {
        let frames = (fixture.input.len() / fixture.channels as usize) as u32;

        for window in [2, 40, 333, 7_000] {
            let mut encoder =
                QoaEncoder::<i16>::new(fixture.sample_rate, fixture.channels, frames).unwrap();
            let minimum = encoder.minimum_output();
            let encoded = encode(&mut encoder, &fixture.input, window, minimum);

            assert_eq!(encoded, fixture.encoded, "window {window}");
        }
    }
}

#[test]
fn encoder_writes_streaming_files() {
    for fixture in fixtures() {
        let mut encoder = QoaEncoder::<i16>::new(fixture.sample_rate, fixture.channels, 0).unwrap();
        assert!(encoder.is_streaming());
        assert_eq!(encoder.minimum_input(), 5_120 * fixture.channels as usize);

        let window = encoder.minimum_input();
        let encoded = encode(&mut encoder, &fixture.input, window, 1_000);

        assert_eq!(encoded, streaming(fixture.encoded));
    }
}

#[test]
fn encoder_accepts_f32() {
    let fixture = &fixtures()[1];
    let input: Vec<f32> = fixture
        .input
        .iter()
        .map(|sample| sample.convert())
        .collect();
    let frames = (input.len() / 2) as u32;
    let mut encoder = QoaEncoder::<f32>::new(fixture.sample_rate, 2, frames).unwrap();
    let mut output = vec![0; 1 << 16];

    let progress = encoder.encode(&input, &mut output).unwrap();

    assert_eq!(progress.consumed, input.len());
    assert_eq!(&output[..progress.produced], fixture.encoded);
}

#[test]
fn encoder_rejects_invalid_parameters() {
    assert_eq!(
        QoaEncoder::<i16>::new(44_100, 0, 1).unwrap_err(),
        Error::TooManyChannels
    );
    assert_eq!(
        QoaEncoder::<i16>::new(44_100, 9, 1).unwrap_err(),
        Error::TooManyChannels
    );
    assert_eq!(
        QoaEncoder::<i16>::new(0, 1, 1).unwrap_err(),
        Error::InvalidParameter
    );
    assert_eq!(
        QoaEncoder::<i16>::new(0x100_0000, 1, 1).unwrap_err(),
        Error::InvalidParameter
    );

    let mut encoder = QoaEncoder::<i16>::new(44_100, 2, 1).unwrap();
    let mut output = [0; 8];
    assert_eq!(
        encoder.encode(&[0; 2], &mut output).unwrap_err(),
        Error::BufferTooSmall
    );
}

#[test]
fn decoder_rejects_invalid_headers() {
    let mut output = [0i16; 160];

    let mut decoder = QoaDecoder::<i16>::new();
    assert_eq!(
        decoder.decode(b"qoag\0\0\0\x01", &mut output).unwrap_err(),
        Error::InvalidHeader
    );

    let mut file = MONO.to_vec();
    file[8] = 9;
    let mut decoder = QoaDecoder::<i16>::new();
    assert_eq!(
        decoder.decode(&file, &mut output).unwrap_err(),
        Error::TooManyChannels
    );

    let mut file = MONO.to_vec();
    file[15] ^= 1;
    let mut decoder = QoaDecoder::<i16>::new();
    assert_eq!(
        decoder.decode(&file, &mut output).unwrap_err(),
        Error::InvalidData
    );

    let mut decoder = QoaDecoder::<i16>::new();
    assert_eq!(
        decoder.decode(MONO, &mut output[..159]).unwrap_err(),
        Error::BufferTooSmall
    );
}

#[test]
fn decoder_reports_truncated_input() {
    for fixture in fixtures() {
        let length = fixture.encoded.len();

        for end in (0..length).step_by(7).chain(length - 9..length) {
            let mut decoder = QoaDecoder::<i16>::new();
            let (error, decoded) =
                decode(&mut decoder, &fixture.encoded[..end], 64, 160).unwrap_err();

            assert_eq!(error, Error::Incomplete, "end {end}");
            assert_eq!(decoded, fixture.decoded[..decoded.len()], "end {end}");
        }
    }
}

#[test]
fn decoder_survives_corrupt_input() {
    for fixture in fixtures() {
        for position in (0..fixture.encoded.len()).step_by(11) {
            for mask in [0x01, 0x80, 0xFF] {
                let mut file = fixture.encoded.to_vec();
                file[position] ^= mask;

                let mut decoder = QoaDecoder::<i16>::new();
                let _ = decode(&mut decoder, &file, 29, 160);
            }
        }

        let mut file = streaming(fixture.encoded);
        file.truncate(1_000);
        file.extend_from_slice(&[0xFF; 200]);
        let mut decoder = QoaDecoder::<i16>::new();
        assert!(decode(&mut decoder, &file, 1 << 12, 1 << 12).is_err());
    }
}

#[test]
fn decoder_ignores_trailing_bytes() {
    let fixture = &fixtures()[0];
    let mut file = fixture.encoded.to_vec();
    file.extend_from_slice(&[0xAB; 64]);

    let mut decoder = QoaDecoder::<i16>::new();
    let decoded = decode(&mut decoder, &file, 256, 160).unwrap();

    assert_eq!(decoded, fixture.decoded);
}
