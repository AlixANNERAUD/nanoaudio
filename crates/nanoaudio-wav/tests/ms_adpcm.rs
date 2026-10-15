use nanoaudio_adpcm::{MsAdpcm, MsAdpcmCoefficients, MsAdpcmState};
use nanoaudio_core::codec::{Decoder, Encoder};
use nanoaudio_core::{Configuration, Error, Format, Sample};
use nanoaudio_wav::{Encoding, WavDecoder, WavHeader};

fn samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes(*chunk))
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
    let mut output = vec![S::SILENCE; decoder.minimum_output() * 2];
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
        decoded.len(),
        header.frames() * header.configuration.channels as usize
    );
    decoded
}

fn wav_file(codec: &MsAdpcm, channels: u16, data: &[u8]) -> Vec<u8> {
    let coefficients = codec.coefficients().as_slice();
    let block_align = codec.block_align();
    let bytes_per_second = 8_000 * block_align as u32 / codec.samples_per_block() as u32;

    let mut format = Vec::new();
    format.extend_from_slice(&2u16.to_le_bytes());
    format.extend_from_slice(&channels.to_le_bytes());
    format.extend_from_slice(&8_000u32.to_le_bytes());
    format.extend_from_slice(&bytes_per_second.to_le_bytes());
    format.extend_from_slice(&block_align.to_le_bytes());
    format.extend_from_slice(&4u16.to_le_bytes());
    format.extend_from_slice(&(4 + 4 * coefficients.len() as u16).to_le_bytes());
    format.extend_from_slice(&(codec.samples_per_block() as u16).to_le_bytes());
    format.extend_from_slice(&(coefficients.len() as u16).to_le_bytes());
    for pair in coefficients {
        format.extend_from_slice(&pair[0].to_le_bytes());
        format.extend_from_slice(&pair[1].to_le_bytes());
    }

    let frames = data.len() / block_align as usize * codec.samples_per_block();
    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(
        &(4 + 8 + format.len() as u32 + 12 + 8 + data.len() as u32).to_le_bytes(),
    );
    file.extend_from_slice(b"WAVE");
    file.extend_from_slice(b"fmt ");
    file.extend_from_slice(&(format.len() as u32).to_le_bytes());
    file.extend_from_slice(&format);
    file.extend_from_slice(b"fact");
    file.extend_from_slice(&4u32.to_le_bytes());
    file.extend_from_slice(&(frames as u32).to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(data);
    file
}

#[test]
fn header_matches_reference() {
    let (header, bytes) = data(include_bytes!("data/ms_adpcm_stereo.wav"));

    assert_eq!(
        header.configuration,
        Configuration::new(8_000, 2, Format::I16)
    );
    assert_eq!(
        header.encoding,
        Encoding::MsAdpcm {
            block_align: 512,
            samples_per_block: 500,
            coefficients: MsAdpcmCoefficients::STANDARD,
        }
    );
    assert_eq!(bytes.len(), 3 * 512);
    assert_eq!(header.frames(), 1500);

    let (header, _) = data(include_bytes!("data/ms_adpcm_permuted.wav"));
    let Encoding::MsAdpcm { coefficients, .. } = header.encoding else {
        panic!("unexpected encoding {:?}", header.encoding);
    };
    let mut reversed = MsAdpcmCoefficients::STANDARD.as_slice().to_vec();
    reversed.reverse();

    assert_eq!(coefficients.as_slice(), reversed.as_slice());
}

#[test]
fn decoder_matches_libsndfile() {
    let cases: [(&[u8], &[u8]); 4] = [
        (
            include_bytes!("data/ms_adpcm_mono.wav"),
            include_bytes!("data/ms_adpcm_mono_decoded.bin"),
        ),
        (
            include_bytes!("data/ms_adpcm_stereo.wav"),
            include_bytes!("data/ms_adpcm_stereo_decoded.bin"),
        ),
        (
            include_bytes!("data/ms_adpcm_permuted.wav"),
            include_bytes!("data/ms_adpcm_stereo_decoded.bin"),
        ),
        (
            include_bytes!("data/ms_adpcm_corrupt.wav"),
            include_bytes!("data/ms_adpcm_corrupt_decoded.bin"),
        ),
    ];

    for (file, decoded) in cases {
        let expected = samples(decoded);

        for window in [512, 700, 1500, usize::MAX] {
            assert_eq!(decode::<i16>(file, window), expected);
        }

        let floats: Vec<f32> = expected
            .iter()
            .map(|sample| *sample as f32 / 32_768.0)
            .collect();
        assert_eq!(decode::<f32>(file, 1024), floats);
    }
}

#[test]
fn decoder_rejects_unknown_predictor() {
    let file = include_bytes!("data/ms_adpcm_mono.wav");
    let (header, bytes) = data(file);
    let mut bytes = bytes.to_vec();
    bytes[256] = 7;

    let mut decoder = WavDecoder::<i16>::new(&header).unwrap();
    let mut output = vec![0i16; 1500];

    let progress = decoder.decode(&bytes, &mut output).unwrap();
    assert_eq!((progress.consumed, progress.produced), (256, 500));
    assert_eq!(
        decoder.decode(&bytes[256..], &mut output),
        Err(Error::InvalidData)
    );

    let custom = MsAdpcmCoefficients::new(&[[256, 0]]).unwrap();
    let mut codec = MsAdpcm::with_coefficients(8_000, 1, 256, 500, custom).unwrap();
    bytes[0] = 0;
    assert_eq!(
        codec.decode(&bytes[..256], &mut output),
        Ok(nanoaudio_core::codec::Progress::new(256, 500))
    );
    bytes[0] = 1;
    assert_eq!(
        codec.decode(&bytes[..256], &mut output),
        Err(Error::InvalidData)
    );
}

#[test]
fn block_layout() {
    let mut block = [0u8; 7 * 2 + 3];
    block[0] = 1;
    block[1] = 3;
    block[2..4].copy_from_slice(&20i16.to_le_bytes());
    block[4..6].copy_from_slice(&300i16.to_le_bytes());
    block[6..8].copy_from_slice(&1000i16.to_le_bytes());
    block[8..10].copy_from_slice(&(-1000i16).to_le_bytes());
    block[10..12].copy_from_slice(&900i16.to_le_bytes());
    block[12..14].copy_from_slice(&(-900i16).to_le_bytes());
    block[14..].copy_from_slice(&[0x7A, 0x18, 0xF3]);

    let mut codec = MsAdpcm::new(8_000, 2, 17).unwrap();
    assert_eq!(codec.samples_per_block(), 5);

    let mut output = [0i16; 10];
    let progress = codec.decode(&block, &mut output).unwrap();
    assert_eq!((progress.consumed, progress.produced), (17, 10));

    let standard = MsAdpcmCoefficients::STANDARD;
    let mut left = MsAdpcmState::new(standard.as_slice()[1], 20, 1000, 900);
    let mut right = MsAdpcmState::new(standard.as_slice()[3], 300, -1000, -900);
    let mut expected = vec![900, -900, 1000, -1000];
    for byte in [0x7A, 0x18, 0xF3] {
        expected.push(left.decode(byte >> 4));
        expected.push(right.decode(byte & 0x0F));
    }

    assert_eq!(output.as_slice(), expected.as_slice());
}

#[test]
fn state_adapts_like_the_reference() {
    let mut state = MsAdpcmState::new([256, 0], 16, 0, 0);

    assert_eq!(state.decode(0x7), 112);
    assert_eq!(state.delta(), 38);
    assert_eq!(state.decode(0x8), 112 - 8 * 38);
    assert_eq!(state.delta(), 114);
    assert_eq!((state.previous(), state.before_previous()), (-192, 112));

    let mut state = MsAdpcmState::new([0, 0], 20_000, 0, 0);
    state.decode(0x8);
    assert_eq!(state.delta(), 16);
    assert_eq!(state.previous(), i16::MIN);
}

#[test]
fn rejects_invalid_parameters() {
    assert_eq!(MsAdpcm::new(8_000, 3, 512), Err(Error::TooManyChannels));
    assert_eq!(MsAdpcm::new(8_000, 0, 512), Err(Error::TooManyChannels));
    assert_eq!(MsAdpcm::new(8_000, 2, 14), Err(Error::InvalidParameter));
    assert_eq!(MsAdpcm::new(0, 1, 256), Err(Error::InvalidParameter));
    assert_eq!(
        MsAdpcm::with_coefficients(8_000, 1, 256, 501, MsAdpcmCoefficients::STANDARD),
        Err(Error::InvalidParameter)
    );
    assert_eq!(
        MsAdpcmCoefficients::new(&[[0, 0]; 8]),
        Err(Error::InvalidParameter)
    );
    assert_eq!(MsAdpcmCoefficients::new(&[]), Err(Error::InvalidParameter));

    let codec = MsAdpcm::with_samples_per_block(8_000, 1, 500).unwrap();
    assert_eq!(codec.block_align(), 256);
    assert_eq!(MsAdpcm::new(8_000, 1, 256).unwrap(), codec);
    assert_eq!(
        MsAdpcm::with_samples_per_block(8_000, 2, 500)
            .unwrap()
            .block_align(),
        512
    );
}

#[test]
fn rejects_invalid_headers() {
    let mut file = include_bytes!("data/ms_adpcm_stereo.wav").to_vec();
    let format = 12 + 8;

    file[format + 20] = 8;
    assert_eq!(WavHeader::parse(&file), Err(Error::UnsupportedFormat));

    file[format + 20] = 0;
    assert_eq!(WavHeader::parse(&file), Err(Error::InvalidHeader));

    file[format + 20] = 7;
    file[format + 18..format + 20].copy_from_slice(&600u16.to_le_bytes());
    assert_eq!(WavHeader::parse(&file), Err(Error::InvalidHeader));

    let file = include_bytes!("data/ms_adpcm_stereo.wav");
    let (header, _) = data(file);
    for length in 0..header.data_offset {
        assert_eq!(WavHeader::parse(&file[..length]), Err(Error::Incomplete));
    }
}

fn signal_to_noise(input: &[i16], output: &[i16]) -> f64 {
    let signal: f64 = input.iter().map(|sample| (*sample as f64).powi(2)).sum();
    let noise: f64 = input
        .iter()
        .zip(output)
        .map(|(input, output)| (*input as f64 - *output as f64).powi(2))
        .sum();

    10.0 * (signal / noise).log10()
}

fn encoder_matches_libsndfile(channels: u8, file: &[u8], decoded: &[u8]) {
    let stereo = samples(include_bytes!("data/ms_adpcm_input.bin"));
    let input: Vec<i16> = stereo
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|frame| frame[..channels as usize].to_vec())
        .collect();

    let mut encoder = MsAdpcm::with_samples_per_block(8_000, channels, 500).unwrap();
    let mut encoded = vec![0u8; 2 * encoder.block_align() as usize];
    let progress = encoder.encode(&input, &mut encoded).unwrap();
    assert_eq!(
        (progress.consumed, progress.produced),
        (input.len(), encoded.len())
    );

    let name = if channels == 1 { "mono" } else { "stereo" };
    let generated = wav_file(&encoder, channels as u16, &encoded);
    if let Some(directory) = std::env::var_os("NANOAUDIO_GENERATED") {
        let path = std::path::Path::new(&directory).join(format!("ms_adpcm_encoded_{name}.wav"));

        std::fs::write(path, &generated).unwrap();
    }
    assert_eq!(generated, file);

    let expected = samples(decoded);
    assert_eq!(decode::<i16>(file, usize::MAX), expected);

    let mut output = vec![0i16; input.len()];
    let mut decoder = MsAdpcm::new(8_000, channels, encoder.block_align()).unwrap();
    decoder.decode(&encoded, &mut output).unwrap();
    assert_eq!(output, expected);

    assert_eq!(
        output[..2 * channels as usize],
        input[..2 * channels as usize]
    );
    assert!(signal_to_noise(&input, &output) > 19.0);
}

#[test]
fn mono_encoder_matches_libsndfile() {
    encoder_matches_libsndfile(
        1,
        include_bytes!("data/ms_adpcm_encoded_mono.wav"),
        include_bytes!("data/ms_adpcm_encoded_mono_decoded.bin"),
    );
}

#[test]
fn stereo_encoder_matches_libsndfile() {
    encoder_matches_libsndfile(
        2,
        include_bytes!("data/ms_adpcm_encoded_stereo.wav"),
        include_bytes!("data/ms_adpcm_encoded_stereo_decoded.bin"),
    );
}

#[test]
fn encoder_waits_for_whole_blocks() {
    let mut encoder = MsAdpcm::with_samples_per_block(8_000, 1, 500).unwrap();
    let mut output = [0u8; 256];

    assert!(encoder.encode(&[0; 499], &mut output).unwrap().is_stalled());
    assert_eq!(
        encoder.encode(&[0; 500], &mut output[..255]),
        Err(Error::BufferTooSmall)
    );
}
