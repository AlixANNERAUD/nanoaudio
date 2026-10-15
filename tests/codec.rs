use nanoaudio::codec::{
    Decoder, Encoder, G711, ImaAdpcm, ImaAdpcmState, Law, WavDecoder, decode_a_law, decode_mu_law,
    encode_a_law, encode_mu_law,
};

fn samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

#[test]
fn a_law_matches_reference() {
    let encoded = include_bytes!("data/a_law_encoded.bin");
    let decoded = samples(include_bytes!("data/a_law_decoded.bin"));

    for (sample, expected) in (i16::MIN..=i16::MAX).zip(encoded) {
        assert_eq!(encode_a_law(sample), *expected, "sample {sample}");
    }

    for (code, expected) in (0..=u8::MAX).zip(&decoded) {
        assert_eq!(decode_a_law(code), *expected, "code {code}");
        assert_eq!(encode_a_law(decode_a_law(code)), code, "code {code}");
    }
}

#[test]
fn mu_law_matches_reference() {
    let encoded = include_bytes!("data/mu_law_encoded.bin");
    let decoded = samples(include_bytes!("data/mu_law_decoded.bin"));

    for (sample, expected) in (i16::MIN..=i16::MAX).zip(encoded) {
        assert_eq!(encode_mu_law(sample), *expected, "sample {sample}");
    }

    for (code, expected) in (0..=u8::MAX).zip(&decoded) {
        assert_eq!(decode_mu_law(code), *expected, "code {code}");

        if code != 0x7F {
            assert_eq!(encode_mu_law(decode_mu_law(code)), code, "code {code}");
        }
    }
}

#[test]
fn g711_codec_processes_whole_frames() {
    let mut codec = G711::new(Law::Mu, 8_000, 2).unwrap();
    let input = [100i16, -100, 2000, -2000, 5];
    let mut encoded = [0u8; 8];

    let progress = codec.encode(&input, &mut encoded).unwrap();
    assert_eq!((progress.consumed, progress.produced), (4, 4));

    let mut decoded = [0i16; 8];
    let progress = codec.decode(&encoded[..4], &mut decoded).unwrap();
    assert_eq!((progress.consumed, progress.produced), (4, 4));

    for (original, decoded) in input[..4].iter().zip(&decoded) {
        assert!((original - decoded).abs() <= original.abs() / 16 + 4);
    }
}

#[test]
fn ima_adpcm_matches_reference() {
    let input = samples(include_bytes!("data/adpcm_input.bin"));
    let nibbles = include_bytes!("data/adpcm_nibbles.bin");
    let decoded = samples(include_bytes!("data/adpcm_decoded.bin"));

    let mut encoder = ImaAdpcmState::default();
    for (index, (sample, expected)) in input.iter().zip(nibbles).enumerate() {
        assert_eq!(encoder.encode(*sample), *expected, "sample {index}");
    }

    let mut decoder = ImaAdpcmState::default();
    for (index, (nibble, expected)) in nibbles.iter().zip(&decoded).enumerate() {
        assert_eq!(decoder.decode(*nibble), *expected, "sample {index}");
    }
}

#[test]
fn ima_adpcm_block_layout() {
    let mut block = [0u8; 12];
    block[..2].copy_from_slice(&100i16.to_le_bytes());
    block[2] = 10;
    block[4..8].copy_from_slice(&[0x21, 0x43, 0x65, 0x87]);
    block[8..12].copy_from_slice(&[0x9A, 0xBC, 0xDE, 0xF0]);

    let mut codec = ImaAdpcm::new(8_000, 1, 12).unwrap();
    assert_eq!(codec.samples_per_block(), 17);

    let mut output = [0i16; 17];
    let progress = codec.decode(&block, &mut output).unwrap();
    assert_eq!((progress.consumed, progress.produced), (12, 17));

    let mut state = ImaAdpcmState::new(100, 10);
    let mut expected = vec![100];
    for byte in &block[4..] {
        expected.push(state.decode(byte & 0x0F));
        expected.push(state.decode(byte >> 4));
    }

    assert_eq!(output.as_slice(), expected.as_slice());
}

fn expected_blocks(input: &[i16], channels: usize, samples_per_block: usize) -> Vec<u8> {
    let mut states = vec![ImaAdpcmState::default(); channels];
    let mut output = Vec::new();

    for block in input.chunks_exact(samples_per_block * channels) {
        for (channel, state) in states.iter_mut().enumerate() {
            *state = ImaAdpcmState::new(block[channel], state.index());
            output.extend_from_slice(&block[channel].to_le_bytes());
            output.extend_from_slice(&[state.index(), 0]);
        }

        let mut nibbles = vec![Vec::new(); channels];
        for frame in block.chunks_exact(channels).skip(1) {
            for (channel, sample) in frame.iter().enumerate() {
                nibbles[channel].push(states[channel].encode(*sample));
            }
        }

        for group in 0..(samples_per_block - 1) / 8 {
            for channel_nibbles in &nibbles {
                for pair in channel_nibbles[group * 8..group * 8 + 8].as_chunks::<2>().0 {
                    output.push(pair[0] | (pair[1] << 4));
                }
            }
        }
    }

    output
}

#[test]
fn ima_adpcm_stereo_round_trip() {
    let mut encoder = ImaAdpcm::with_samples_per_block(16_000, 2, 505).unwrap();
    let mut decoder = ImaAdpcm::new(16_000, 2, encoder.block_align()).unwrap();
    assert_eq!(encoder.block_align(), 512);

    let frames = 505 * 3;
    let input: Vec<i16> = (0..frames)
        .flat_map(|index| {
            let phase = index as f32 / 16_000.0 * core::f32::consts::TAU;
            [
                ((phase * 440.0).sin() * 12_000.0) as i16,
                ((phase * 660.0).sin() * 8_000.0) as i16,
            ]
        })
        .collect();

    let mut encoded = vec![0u8; 512 * 3];
    let progress = encoder.encode(&input, &mut encoded).unwrap();
    assert_eq!(
        (progress.consumed, progress.produced),
        (input.len(), encoded.len())
    );

    let mut output = vec![0i16; input.len()];
    let progress = decoder.decode(&encoded, &mut output).unwrap();
    assert_eq!(
        (progress.consumed, progress.produced),
        (encoded.len(), input.len())
    );

    let signal_to_noise = |range: core::ops::Range<usize>| {
        let signal: f64 = input[range.clone()]
            .iter()
            .map(|sample| (*sample as f64).powi(2))
            .sum();
        let noise: f64 = input[range.clone()]
            .iter()
            .zip(&output[range])
            .map(|(input, output)| (*input as f64 - *output as f64).powi(2))
            .sum();

        10.0 * (signal / noise).log10()
    };

    assert!(signal_to_noise(0..1010) > 15.0);
    assert!(signal_to_noise(1010..3030) > 30.0);
    assert_eq!(encoded, expected_blocks(&input, 2, 505));
    assert_eq!(output[..2], input[..2]);
}

#[test]
fn ima_adpcm_waits_for_whole_blocks() {
    let mut codec = ImaAdpcm::new(8_000, 1, 12).unwrap();
    let mut output = [0i16; 17];

    assert!(codec.decode(&[0; 11], &mut output).unwrap().is_stalled());
    assert!(codec.decode(&[0; 12], &mut output[..16]).is_err());
    assert!(ImaAdpcm::new(8_000, 2, 12).is_err());
    assert!(ImaAdpcm::with_samples_per_block(8_000, 1, 10).is_err());
}

fn wav_file(
    tag: u16,
    channels: u16,
    bits: u16,
    block_align: u16,
    extra: &[u8],
    data: &[u8],
) -> Vec<u8> {
    let mut format = Vec::new();
    format.extend_from_slice(&tag.to_le_bytes());
    format.extend_from_slice(&channels.to_le_bytes());
    format.extend_from_slice(&8_000u32.to_le_bytes());
    format.extend_from_slice(&0u32.to_le_bytes());
    format.extend_from_slice(&block_align.to_le_bytes());
    format.extend_from_slice(&bits.to_le_bytes());
    format.extend_from_slice(extra);

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(b"WAVE");
    file.extend_from_slice(b"fmt ");
    file.extend_from_slice(&(format.len() as u32).to_le_bytes());
    file.extend_from_slice(&format);
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(data);
    file
}

fn decode_file<S: nanoaudio::Sample>(file: &[u8]) -> (nanoaudio::WavHeader, Vec<S>) {
    let header = nanoaudio::WavHeader::parse(file).unwrap();
    let mut decoder = WavDecoder::<S>::new(&header).unwrap();
    let mut input = &file[header.data_offset..];
    let mut output = vec![S::SILENCE; header.frames() * header.configuration.channels as usize];
    let mut produced = 0;

    while !input.is_empty() {
        let progress = decoder.decode(input, &mut output[produced..]).unwrap();

        if progress.is_stalled() {
            break;
        }

        input = &input[progress.consumed..];
        produced += progress.produced;
    }

    assert_eq!(produced, output.len());
    (header, output)
}

#[test]
fn wav_decoder_g711() {
    let data = [0x00u8, 0x7F, 0x80, 0xFF, 0x55, 0xD5];
    let file = wav_file(7, 2, 8, 2, &[0, 0], &data);

    let (header, decoded) = decode_file::<i16>(&file);
    assert_eq!(header.frames(), 3);
    assert_eq!(decoded, data.map(decode_mu_law));

    let (_, decoded) = decode_file::<f32>(&wav_file(6, 1, 8, 1, &[0, 0], &data));
    let expected: Vec<f32> = data
        .iter()
        .map(|code| decode_a_law(*code) as f32 / 32_768.0)
        .collect();
    assert_eq!(decoded, expected);
}

#[test]
fn wav_decoder_ima_adpcm() {
    let mut encoder = ImaAdpcm::with_samples_per_block(8_000, 1, 505).unwrap();
    let input: Vec<i16> = (0..1010)
        .map(|index| ((index as f32 * 0.3).sin() * 9_000.0) as i16)
        .collect();
    let mut data = vec![0u8; 512];
    encoder.encode(&input, &mut data).unwrap();

    let file = wav_file(0x11, 1, 4, 256, &[2, 0, 0xF9, 0x01], &data);
    let (header, decoded) = decode_file::<i16>(&file);

    let mut reference = vec![0i16; 1010];
    ImaAdpcm::new(8_000, 1, 256)
        .unwrap()
        .decode(&data, &mut reference)
        .unwrap();

    assert_eq!(header.frames(), 1010);
    assert_eq!(decoded, reference);
}

#[test]
fn wav_decoder_pcm_into_pipeline() {
    use nanoaudio::{Format, Stream, Volume, samples};

    let file = wav_file(1, 1, 8, 1, &[], &[0x00, 0x80, 0xFF]);
    let (_, decoded) = decode_file::<i16>(&file);
    assert_eq!(decoded, [i16::MIN, 0, 0x7F00]);

    let mut output = [0u8; 12];
    let frames = samples(&decoded, 1)
        .unwrap()
        .to_channels(2)
        .unwrap()
        .volume(Volume::from_percent(50))
        .write_bytes(&mut output, Format::I16);

    assert_eq!(frames, 3);
    assert_eq!(i16::from_le_bytes([output[0], output[1]]), -16_384);
    assert_eq!(i16::from_le_bytes([output[10], output[11]]), 0x3F80);
}
