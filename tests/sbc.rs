use nanoaudio::Error;
use nanoaudio::codec::{
    Decoder, Encoder, SBC_MAXIMUM_FRAME_LENGTH, SBC_MAXIMUM_FRAME_SAMPLES, SbcAllocation,
    SbcChannelMode, SbcDecoder, SbcEncoder, SbcParameters,
};

struct Fixture {
    stream: &'static [u8],
    reference: &'static [u8],
    parameters: SbcParameters,
}

const fn parameters(
    sample_rate: u32,
    channel_mode: SbcChannelMode,
    allocation: SbcAllocation,
    blocks: u8,
    subbands: u8,
    bitpool: u8,
) -> SbcParameters {
    SbcParameters {
        sample_rate,
        channel_mode,
        allocation,
        blocks,
        subbands,
        bitpool,
    }
}

const FIXTURES: [Fixture; 6] = [
    Fixture {
        stream: include_bytes!("data/sbc_44100_joint_stereo.sbc"),
        reference: include_bytes!("data/sbc_44100_joint_stereo.pcm"),
        parameters: SbcParameters::HIGH_QUALITY,
    },
    Fixture {
        stream: include_bytes!("data/sbc_48000_stereo.sbc"),
        reference: include_bytes!("data/sbc_48000_stereo.pcm"),
        parameters: parameters(
            48_000,
            SbcChannelMode::Stereo,
            SbcAllocation::Loudness,
            16,
            8,
            51,
        ),
    },
    Fixture {
        stream: include_bytes!("data/sbc_16000_mono.sbc"),
        reference: include_bytes!("data/sbc_16000_mono.pcm"),
        parameters: parameters(
            16_000,
            SbcChannelMode::Mono,
            SbcAllocation::Loudness,
            12,
            4,
            20,
        ),
    },
    Fixture {
        stream: include_bytes!("data/sbc_32000_snr.sbc"),
        reference: include_bytes!("data/sbc_32000_snr.pcm"),
        parameters: parameters(
            32_000,
            SbcChannelMode::JointStereo,
            SbcAllocation::Snr,
            8,
            4,
            35,
        ),
    },
    Fixture {
        stream: include_bytes!("data/sbc_44100_dual_channel.sbc"),
        reference: include_bytes!("data/sbc_44100_dual_channel.pcm"),
        parameters: parameters(
            44_100,
            SbcChannelMode::DualChannel,
            SbcAllocation::Loudness,
            4,
            8,
            32,
        ),
    },
    Fixture {
        stream: include_bytes!("data/msbc.sbc"),
        reference: include_bytes!("data/msbc.pcm"),
        parameters: SbcParameters::MSBC,
    },
];

fn samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes(*chunk))
        .collect()
}

fn decode(stream: &[u8]) -> Vec<i16> {
    let mut decoder = SbcDecoder::new();
    let mut output = vec![0i16; 1 << 16];
    let progress = decoder.decode(stream, &mut output).unwrap();

    assert_eq!(progress.consumed, stream.len());
    output.truncate(progress.produced);
    output
}

fn signal_to_noise(signal: &[i16], decoded: &[i16]) -> f64 {
    let power: f64 = signal.iter().map(|sample| (*sample as f64).powi(2)).sum();
    let noise: f64 = signal
        .iter()
        .zip(decoded)
        .map(|(signal, decoded)| (*signal as f64 - *decoded as f64).powi(2))
        .sum();

    10.0 * (power / noise).log10()
}

#[test]
fn decoder_matches_reference() {
    for fixture in &FIXTURES {
        let decoded = decode(fixture.stream);
        let reference = samples(fixture.reference);
        let maximum_error = decoded
            .iter()
            .zip(&reference)
            .map(|(decoded, reference)| (*decoded as i32 - *reference as i32).abs())
            .max()
            .unwrap();
        let ratio = signal_to_noise(&reference, &decoded);

        assert_eq!(decoded.len(), reference.len(), "{:?}", fixture.parameters);
        assert!(
            maximum_error <= 8,
            "{:?} {maximum_error}",
            fixture.parameters
        );
        assert!(ratio > 78.0, "{:?} {ratio}", fixture.parameters);
    }
}

#[test]
fn parameters_match_headers() {
    for fixture in &FIXTURES {
        let parameters = SbcParameters::parse(fixture.stream).unwrap();
        let length = parameters.frame_length();

        assert_eq!(parameters, fixture.parameters);
        assert_eq!(fixture.stream.len() % length, 0);
        assert_eq!(
            fixture.stream.len() / length * parameters.frame_samples(),
            fixture.reference.len() / 2
        );

        let mut decoder = SbcDecoder::default();
        assert_eq!(decoder.configuration(), None);

        decoder
            .decode(
                &fixture.stream[..length],
                &mut [0; SBC_MAXIMUM_FRAME_SAMPLES],
            )
            .unwrap();
        assert_eq!(decoder.parameters(), Some(parameters));
        assert_eq!(decoder.configuration(), Some(parameters.configuration()));
    }

    assert_eq!(SbcParameters::HIGH_QUALITY.frame_length(), 119);
    assert_eq!(SbcParameters::MSBC.frame_length(), 57);
    assert_eq!(SbcParameters::default(), SbcParameters::HIGH_QUALITY);
}

#[test]
fn decoder_streams_whole_frames() {
    for fixture in &FIXTURES {
        let expected = decode(fixture.stream);
        let mut decoder = SbcDecoder::new();
        let mut output = vec![0.0f32; decoder.minimum_output()];
        let mut decoded = Vec::new();
        let mut position = 0;
        let mut available = 0;

        loop {
            available = (available + 37).min(fixture.stream.len());

            if available == fixture.stream.len() {
                decoder.finish();
            }

            let progress = decoder
                .decode_into(&fixture.stream[position..available], &mut output)
                .unwrap();

            decoded.extend_from_slice(&output[..progress.produced]);
            position += progress.consumed;

            if progress.is_stalled() && available == fixture.stream.len() {
                break;
            }
        }

        assert_eq!(position, fixture.stream.len());
        assert_eq!(decoded.len(), expected.len());

        for (decoded, expected) in decoded.iter().zip(&expected) {
            assert_eq!(*decoded, *expected as f32 / 32_768.0);
        }
    }
}

#[test]
fn decoder_reports_errors() {
    let stream = FIXTURES[0].stream;
    let length = SbcParameters::HIGH_QUALITY.frame_length();
    let mut output = [0i16; SBC_MAXIMUM_FRAME_SAMPLES];
    let mut decoder = SbcDecoder::new();

    assert!(
        decoder
            .decode(&stream[..3], &mut output)
            .unwrap()
            .is_stalled()
    );
    assert!(
        decoder
            .decode(&stream[..length - 1], &mut output)
            .unwrap()
            .is_stalled()
    );
    assert_eq!(
        decoder.decode(stream, &mut output[..255]),
        Err(Error::BufferTooSmall)
    );

    let progress = decoder.decode(stream, &mut output).unwrap();
    assert_eq!((progress.consumed, progress.produced), (length, 256));

    let mut corrupted = stream[..length].to_vec();
    corrupted[5] ^= 0x10;
    assert_eq!(
        decoder.decode(&corrupted, &mut output),
        Err(Error::InvalidData)
    );

    corrupted[0] = 0x9D;
    assert_eq!(
        decoder.decode(&corrupted, &mut output),
        Err(Error::InvalidHeader)
    );
    assert_eq!(
        decoder.decode(&[0x9C, 0xBD, 1, 0], &mut output),
        Err(Error::InvalidHeader)
    );
    assert_eq!(
        decoder.decode(&[0x9C, 0x31, 129, 0], &mut output),
        Err(Error::InvalidHeader)
    );

    decoder.finish();
    assert!(decoder.decode(&[], &mut output).unwrap().is_stalled());
    assert_eq!(
        decoder.decode(&stream[..length + 10], &mut output),
        Ok(nanoaudio::codec::Progress::new(length, 256))
    );
    assert_eq!(
        decoder.decode(&stream[..10], &mut output),
        Err(Error::Incomplete)
    );
    assert_eq!(
        decoder.decode(&stream[..2], &mut output),
        Err(Error::Incomplete)
    );

    decoder.reset();
    assert_eq!(decoder.parameters(), None);
    assert!(
        decoder
            .decode(&stream[..2], &mut output)
            .unwrap()
            .is_stalled()
    );
}

fn signal(sample_rate: u32, channels: usize, frames: usize) -> Vec<i16> {
    (0..frames)
        .flat_map(|index| {
            let time = index as f64 / sample_rate as f64;
            let left = (core::f64::consts::TAU * 440.0 * time).sin() * 9_000.0
                + (core::f64::consts::TAU * 2_500.0 * time).sin() * 4_000.0;
            let right = (core::f64::consts::TAU * 700.0 * time).sin() * 7_000.0
                + (core::f64::consts::TAU * 440.0 * time).sin() * 5_000.0;

            [left as i16, right as i16].into_iter().take(channels)
        })
        .collect()
}

fn round_trip(parameters: SbcParameters, minimum: f64) {
    let channels = parameters.channels() as usize;
    let frames = 40;
    let input = signal(
        parameters.sample_rate,
        channels,
        frames * parameters.samples_per_channel(),
    );
    let mut encoder = SbcEncoder::new(parameters).unwrap();
    let mut encoded = vec![0u8; frames * parameters.frame_length()];

    assert_eq!(encoder.minimum_input(), parameters.frame_samples());
    assert_eq!(encoder.minimum_output(), parameters.frame_length());
    assert_eq!(encoder.configuration(), parameters.configuration());

    let progress = encoder.encode(&input, &mut encoded).unwrap();
    assert_eq!(
        (progress.consumed, progress.produced),
        (input.len(), encoded.len())
    );

    for frame in encoded.chunks_exact(parameters.frame_length()) {
        assert_eq!(SbcParameters::parse(frame), Ok(parameters));
    }

    let decoded = decode(&encoded);
    let delay = (10 * parameters.subbands as usize - parameters.subbands as usize + 1) * channels;
    let skip = 4 * parameters.samples_per_channel() * channels;
    let ratio = signal_to_noise(&input[skip..input.len() - delay], &decoded[skip + delay..]);

    assert!(ratio > minimum, "{parameters:?} {ratio}");
}

#[test]
fn encoder_round_trip() {
    round_trip(SbcParameters::HIGH_QUALITY, 60.0);
    round_trip(SbcParameters::MSBC, 50.0);

    for (channel_mode, allocation, blocks, subbands, bitpool, minimum) in [
        (
            SbcChannelMode::Stereo,
            SbcAllocation::Loudness,
            16,
            8,
            51,
            55.0,
        ),
        (
            SbcChannelMode::Mono,
            SbcAllocation::Loudness,
            12,
            4,
            20,
            55.0,
        ),
        (
            SbcChannelMode::JointStereo,
            SbcAllocation::Snr,
            8,
            4,
            35,
            60.0,
        ),
        (
            SbcChannelMode::DualChannel,
            SbcAllocation::Snr,
            4,
            8,
            32,
            60.0,
        ),
        (
            SbcChannelMode::Stereo,
            SbcAllocation::Loudness,
            16,
            8,
            250,
            60.0,
        ),
        (SbcChannelMode::Mono, SbcAllocation::Snr, 16, 8, 2, 3.0),
    ] {
        for sample_rate in [16_000, 32_000, 44_100, 48_000] {
            round_trip(
                parameters(
                    sample_rate,
                    channel_mode,
                    allocation,
                    blocks,
                    subbands,
                    bitpool,
                ),
                minimum,
            );
        }
    }
}

#[test]
fn encoder_waits_for_whole_frames() {
    let parameters = SbcParameters::HIGH_QUALITY;
    let mut encoder = SbcEncoder::new(parameters).unwrap();
    let input = signal(44_100, 2, 300);
    let mut output = [0u8; SBC_MAXIMUM_FRAME_LENGTH];

    assert!(
        encoder
            .encode(&input[..255], &mut output)
            .unwrap()
            .is_stalled()
    );
    assert_eq!(
        encoder.encode(&input, &mut output[..118]),
        Err(Error::BufferTooSmall)
    );

    let progress = encoder.encode(&input, &mut output).unwrap();
    assert_eq!((progress.consumed, progress.produced), (512, 238));

    let first = output;
    encoder.reset();
    encoder.encode(&input, &mut output).unwrap();
    assert_eq!(first, output);
    assert_eq!(encoder.parameters(), parameters);
}

#[test]
fn invalid_parameters() {
    let valid = SbcParameters::HIGH_QUALITY;

    for invalid in [
        SbcParameters {
            sample_rate: 22_050,
            ..valid
        },
        SbcParameters { blocks: 6, ..valid },
        SbcParameters {
            subbands: 6,
            ..valid
        },
        SbcParameters {
            bitpool: 1,
            ..valid
        },
        SbcParameters {
            channel_mode: SbcChannelMode::Mono,
            bitpool: 129,
            ..valid
        },
        SbcParameters {
            blocks: 15,
            ..valid
        },
    ] {
        assert_eq!(invalid.validate(), Err(Error::InvalidParameter));
        assert!(SbcEncoder::new(invalid).is_err());
    }

    assert_eq!(valid.validate(), Ok(()));
    assert_eq!(SbcParameters::MSBC.validate(), Ok(()));
    assert_eq!(
        SbcParameters {
            bitpool: 255,
            ..valid
        }
        .validate(),
        Ok(())
    );
}

fn checksum(frame: &[u8], bits: usize) -> u8 {
    (8..24)
        .chain(32..32 + bits)
        .fold(0x0F, |crc: u8, position| {
            let bit = (frame[position / 8] >> (7 - position % 8)) & 1;
            let feedback = (crc >> 7) ^ bit;

            (crc << 1) ^ if feedback == 1 { 0x1D } else { 0 }
        })
}

#[test]
fn decoder_survives_arbitrary_frames() {
    let mut state = 2_463_534_242u32;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut decoder = SbcDecoder::new();
    let mut output = [0i16; SBC_MAXIMUM_FRAME_SAMPLES];

    for iteration in 0..4_000 {
        let settings = random() as u8;
        let mut header = [0x9C, settings, 0];
        let maximum = SbcParameters::parse(&[0x9C, settings, 2])
            .unwrap()
            .maximum_bitpool()
            .min(255);

        header[2] = (2 + random() as usize % (maximum - 1)) as u8;

        if iteration % 10 == 0 {
            header = [0xAD, 0, 0];
        }

        let parameters = SbcParameters::parse(&header).unwrap();
        let mut frame: Vec<u8> = (0..parameters.frame_length())
            .map(|_| random() as u8)
            .collect();
        let join = if parameters.channel_mode == SbcChannelMode::JointStereo {
            parameters.subbands as usize
        } else {
            0
        };
        let bits = join + 4 * parameters.subbands as usize * parameters.channels() as usize;

        frame[..3].copy_from_slice(&header);
        frame[3] = checksum(&frame, bits);

        let progress = decoder.decode(&frame, &mut output).unwrap();

        assert_eq!(
            (progress.consumed, progress.produced),
            (frame.len(), parameters.frame_samples())
        );
    }
}
