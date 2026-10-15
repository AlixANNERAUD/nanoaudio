use nanoaudio_aiff::{AiffDecoder, AiffEncoding, AiffHeader};
use nanoaudio_core::codec::Decoder;
use nanoaudio_core::{Configuration, Format, Sample};

fn values<T, const SIZE: usize>(bytes: &[u8], read: fn([u8; SIZE]) -> T) -> Vec<T> {
    bytes
        .as_chunks::<SIZE>()
        .0
        .iter()
        .map(|chunk| read(*chunk))
        .collect()
}

fn decode<S: Sample>(file: &[u8], window: usize) -> (AiffHeader, Vec<S>) {
    let header = AiffHeader::parse(file).unwrap();
    let mut decoder = AiffDecoder::<S>::new(&header).unwrap();
    let start = header.data_offset;
    let mut input = &file[start..start + header.data_size as usize];
    let mut output = vec![S::SILENCE; decoder.minimum_output() * 37];
    let mut decoded = Vec::new();

    loop {
        let length = window.min(input.len());
        let progress = decoder.decode(&input[..length], &mut output).unwrap();

        if progress.is_stalled() {
            break;
        }

        decoded.extend_from_slice(&output[..progress.produced]);
        input = &input[progress.consumed..];
    }

    assert!(input.is_empty());
    assert_eq!(
        decoder.configuration(),
        Some(Configuration::new(
            header.configuration.sample_rate,
            header.configuration.channels,
            S::FORMAT
        ))
    );
    assert_eq!(
        decoded.len(),
        header.frames() * header.configuration.channels as usize
    );
    (header, decoded)
}

fn check_integer(
    file: &[u8],
    decoded: &[u8],
    configuration: Configuration,
    encoding: AiffEncoding,
    frames: usize,
) {
    let expected = values(decoded, i32::from_le_bytes);

    for window in [8, 13, 100, usize::MAX] {
        let (header, samples) = decode::<i32>(file, window);

        assert_eq!(header.configuration, configuration);
        assert_eq!(header.encoding, encoding);
        assert_eq!(header.frames(), frames);
        assert_eq!(samples, expected);
    }
}

#[test]
fn signed_8_bit_pcm_matches_libsndfile() {
    check_integer(
        include_bytes!("data/aiff_pcm_s8.aiff"),
        include_bytes!("data/aiff_pcm_s8_decoded.bin"),
        Configuration::new(8_000, 1, Format::U8),
        AiffEncoding::BigEndian,
        302,
    );

    let (_, samples) = decode::<u8>(include_bytes!("data/aiff_pcm_s8.aiff"), 64);
    let expected: Vec<u8> = values(
        include_bytes!("data/aiff_pcm_s8_decoded.bin"),
        i32::from_le_bytes,
    )
    .into_iter()
    .map(|value| ((value >> 24) as u8) ^ 0x80)
    .collect();
    assert_eq!(samples, expected);
}

#[test]
fn big_endian_pcm_matches_libsndfile() {
    check_integer(
        include_bytes!("data/aiff_pcm_16.aiff"),
        include_bytes!("data/aiff_pcm_16_decoded.bin"),
        Configuration::new(44_100, 2, Format::I16),
        AiffEncoding::BigEndian,
        300,
    );
    check_integer(
        include_bytes!("data/aiff_pcm_24.aiff"),
        include_bytes!("data/aiff_pcm_24_decoded.bin"),
        Configuration::new(48_000, 2, Format::I24),
        AiffEncoding::BigEndian,
        300,
    );
    check_integer(
        include_bytes!("data/aiff_pcm_32.aiff"),
        include_bytes!("data/aiff_pcm_32_decoded.bin"),
        Configuration::new(96_000, 2, Format::I32),
        AiffEncoding::BigEndian,
        300,
    );
    check_integer(
        include_bytes!("data/aiff_none.aifc"),
        include_bytes!("data/aiff_none_decoded.bin"),
        Configuration::new(44_100, 2, Format::I16),
        AiffEncoding::BigEndian,
        300,
    );

    let (_, samples) = decode::<i16>(include_bytes!("data/aiff_pcm_16.aiff"), 1000);
    let expected: Vec<i16> = values(
        include_bytes!("data/aiff_pcm_16_decoded.bin"),
        i32::from_le_bytes,
    )
    .into_iter()
    .map(|value| (value >> 16) as i16)
    .collect();
    assert_eq!(samples, expected);
}

#[test]
fn little_endian_pcm_matches_libsndfile() {
    check_integer(
        include_bytes!("data/aiff_sowt_16.aifc"),
        include_bytes!("data/aiff_sowt_16_decoded.bin"),
        Configuration::new(22_050, 2, Format::I16),
        AiffEncoding::LittleEndian,
        300,
    );
    check_integer(
        include_bytes!("data/aiff_sowt_32.aifc"),
        include_bytes!("data/aiff_sowt_32_decoded.bin"),
        Configuration::new(11_025, 1, Format::I32),
        AiffEncoding::LittleEndian,
        300,
    );
}

#[test]
fn float_matches_libsndfile() {
    let file = include_bytes!("data/aiff_float.aifc");
    let reference = values(
        include_bytes!("data/aiff_float_decoded.bin"),
        f32::from_le_bytes,
    );

    let header = AiffHeader::parse(file).unwrap();
    let start = header.data_offset;
    let raw = values(
        &file[start..start + header.data_size as usize],
        f32::from_be_bytes,
    );
    assert_eq!(raw, reference);

    let expected: Vec<f32> = reference.iter().map(|value| value.convert()).collect();
    for window in [8, 13, 100, usize::MAX] {
        let (header, samples) = decode::<f32>(file, window);

        assert_eq!(
            header.configuration,
            Configuration::new(48_000, 2, Format::F32)
        );
        assert_eq!(header.encoding, AiffEncoding::BigEndian);
        assert_eq!(samples, expected);
    }

    let expected: Vec<i32> = reference.iter().map(|value| value.to_i32()).collect();
    assert_eq!(decode::<i32>(file, 64).1, expected);
}

fn check_g711(file: &[u8], decoded: &[u8], configuration: Configuration, encoding: AiffEncoding) {
    let expected = values(decoded, i16::from_le_bytes);

    for window in [2, 3, 100, usize::MAX] {
        let (header, samples) = decode::<i16>(file, window);

        assert_eq!(header.configuration, configuration);
        assert_eq!(header.encoding, encoding);
        assert_eq!(samples, expected);
    }
}

#[test]
fn g711_matches_libsndfile() {
    check_g711(
        include_bytes!("data/aiff_alaw.aifc"),
        include_bytes!("data/aiff_alaw_decoded.bin"),
        Configuration::new(8_000, 1, Format::I16),
        AiffEncoding::ALaw,
    );
    check_g711(
        include_bytes!("data/aiff_ulaw.aifc"),
        include_bytes!("data/aiff_ulaw_decoded.bin"),
        Configuration::new(8_000, 2, Format::I16),
        AiffEncoding::MuLaw,
    );
}

#[test]
fn headers_parse_from_partial_files() {
    for file in [
        &include_bytes!("data/aiff_pcm_16.aiff")[..],
        &include_bytes!("data/aiff_float.aifc")[..],
        &include_bytes!("data/aiff_alaw.aifc")[..],
    ] {
        let header = AiffHeader::parse(file).unwrap();

        for length in 0..header.data_offset {
            assert_eq!(
                AiffHeader::parse(&file[..length]),
                Err(nanoaudio_core::Error::Incomplete)
            );
        }

        assert_eq!(AiffHeader::parse(&file[..header.data_offset]), Ok(header));
    }
}
