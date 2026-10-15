use nanoaudio::Format;
use nanoaudio::codec::{
    Decoder, Encoder, G722_RTP_CLOCK_RATE, G722_SAMPLE_RATE, G722BitRate, G722Decoder, G722Encoder,
};

const LENGTH: usize = 32_000;
const BLOCK: usize = 1_600;
const BIT_RATES: [G722BitRate; 3] = [
    G722BitRate::Rate64000,
    G722BitRate::Rate56000,
    G722BitRate::Rate48000,
];

fn wave(phase: u32) -> i32 {
    let position = ((phase >> 16) & 0xFFFF) as i32 - 32_768;

    ((position * (32_768 - position.abs())) >> 13).min(32_767)
}

fn signal() -> Vec<i16> {
    let mut state = 12_345u32;
    let mut tone = 0u32;
    let mut chirp = 0u32;
    let mut increment = 0u32;

    (0..LENGTH)
        .map(|index| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345) & 0x7FFF_FFFF;
            let noise = (state >> 15) as i32 - 32_768;
            tone = tone.wrapping_add(117_964_800);
            increment = increment.wrapping_add(2_000);
            chirp = chirp.wrapping_add(increment);

            let value = match index {
                0..4_000 => wave(tone) >> 1,
                4_000..8_000 => 0,
                8_000..14_000 => (wave(chirp) * 19_661) >> 15,
                14_000..20_000 => (noise * 9_830) >> 15,
                20_000..24_000 => wave(tone) * 2,
                24_000..28_000 => (wave(tone) >> 7) + (noise >> 9),
                _ if (index / 40) % 2 == 0 => 24_000,
                _ => -24_000,
            };

            value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        })
        .collect()
}

fn random_bytes(count: usize) -> Vec<u8> {
    let mut state = 987_654_321u32;

    (0..count)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345) & 0x7FFF_FFFF;
            (state >> 23) as u8
        })
        .collect()
}

fn digests(samples: &[i16]) -> Vec<u64> {
    samples
        .chunks(BLOCK)
        .map(|chunk| {
            chunk
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .fold(0xCBF2_9CE4_8422_2325, |digest, byte| {
                    (digest ^ byte as u64).wrapping_mul(0x0100_0000_01B3)
                })
        })
        .collect()
}

fn reference_digests() -> Vec<Vec<u64>> {
    let bytes = include_bytes!("data/g722_decoded.bin");
    let digests: Vec<u64> = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| u64::from_le_bytes(*chunk))
        .collect();
    let encoded = LENGTH / BLOCK;
    let noise = 8_000usize.div_ceil(BLOCK);

    let (encoded_digests, noise_digests) = digests.split_at(encoded * 3);

    encoded_digests
        .chunks(encoded)
        .chain(noise_digests.chunks(noise))
        .map(<[u64]>::to_vec)
        .collect()
}

fn decode(bit_rate: G722BitRate, input: &[u8]) -> Vec<i16> {
    let mut decoder = G722Decoder::new(bit_rate);
    let mut output = vec![0i16; input.len() * 2];
    let progress = decoder.decode(input, &mut output).unwrap();

    assert_eq!(
        (progress.consumed, progress.produced),
        (input.len(), output.len())
    );

    output
}

#[test]
fn encoder_matches_reference() {
    let input = signal();
    let mut encoder = G722Encoder::new();
    let mut output = vec![0u8; LENGTH / 2];
    let progress = encoder.encode(&input, &mut output).unwrap();

    assert_eq!((progress.consumed, progress.produced), (LENGTH, LENGTH / 2));

    let reference = include_bytes!("data/g722_encoded.bin");

    assert_eq!(output.len(), reference.len());

    for (index, (code, expected)) in output.iter().zip(reference).enumerate() {
        assert_eq!(code, expected, "byte {index}");
    }
}

#[test]
fn decoder_matches_reference() {
    let reference = reference_digests();
    let encoded = include_bytes!("data/g722_encoded.bin");
    let noise = random_bytes(4_000);
    let inputs = [encoded.as_slice(), noise.as_slice()];

    for (index, (input, bit_rate)) in inputs
        .iter()
        .flat_map(|input| BIT_RATES.map(|bit_rate| (*input, bit_rate)))
        .enumerate()
    {
        let digests = digests(&decode(bit_rate, input));

        assert_eq!(digests.len(), reference[index].len());

        for (block, (digest, expected)) in digests.iter().zip(&reference[index]).enumerate() {
            assert_eq!(digest, expected, "{bit_rate:?} input {index} block {block}");
        }
    }
}

#[test]
fn round_trip_quality() {
    let input = signal();
    let mut encoded = vec![0u8; LENGTH / 2];
    G722Encoder::new().encode(&input, &mut encoded).unwrap();

    for (bit_rate, minimum) in BIT_RATES.into_iter().zip([30.0, 29.0, 27.0]) {
        let output = decode(bit_rate, &encoded);
        let delay = 22;
        let range = 200..3_800;
        let signal: f64 = input[range.clone()]
            .iter()
            .map(|sample| (*sample as f64).powi(2))
            .sum();
        let noise: f64 = range
            .map(|index| (input[index] as f64 - output[index + delay] as f64).powi(2))
            .sum();
        let ratio = 10.0 * (signal / noise).log10();

        assert!(ratio > minimum, "{bit_rate:?} {ratio}");
    }
}

#[test]
fn streams_in_arbitrary_chunks() {
    let input = signal();
    let mut encoder = G722Encoder::new();
    let mut encoded = Vec::new();
    let mut position = 0;
    let mut buffer = [0u8; 7];

    for size in [1, 2, 3, 5, 8, 13, 21, 34].iter().cycle() {
        let end = (position + size).min(input.len());
        let progress = encoder.encode(&input[position..end], &mut buffer).unwrap();

        assert_eq!(progress.consumed, progress.produced * 2);
        encoded.extend_from_slice(&buffer[..progress.produced]);
        position += progress.consumed;

        if position == input.len() {
            break;
        }
    }

    assert_eq!(encoded.as_slice(), include_bytes!("data/g722_encoded.bin"));

    let mut decoder = G722Decoder::default();
    let mut output = [0.0f32; 9];
    let mut decoded = Vec::new();
    let mut remaining = encoded.as_slice();

    while !remaining.is_empty() {
        let progress = decoder.decode_into(remaining, &mut output).unwrap();

        assert_eq!(progress.produced, progress.consumed * 2);
        decoded.extend_from_slice(&output[..progress.produced]);
        remaining = &remaining[progress.consumed..];
    }

    let expected = decode(G722BitRate::Rate64000, &encoded);

    assert_eq!(decoded.len(), expected.len());

    for (decoded, expected) in decoded.iter().zip(&expected) {
        assert_eq!(*decoded, *expected as f32 / 32_768.0);
    }
}

#[test]
fn stalls_on_partial_input() {
    let mut encoder = G722Encoder::new();
    let mut decoder = G722Decoder::new(G722BitRate::Rate56000);

    assert!(encoder.encode(&[1], &mut [0; 4]).unwrap().is_stalled());
    assert!(encoder.encode(&[1, 2], &mut []).unwrap().is_stalled());
    assert!(decoder.decode(&[0x55], &mut [0; 1]).unwrap().is_stalled());
    assert!(decoder.decode(&[], &mut [0; 2]).unwrap().is_stalled());
}

#[test]
fn reset_restores_initial_state() {
    let input = signal();
    let mut encoder = G722Encoder::new();
    let mut first = [0u8; 100];
    let mut second = [0u8; 100];

    encoder.encode(&input[..200], &mut first).unwrap();
    encoder.reset();
    encoder.encode(&input[..200], &mut second).unwrap();
    assert_eq!(first, second);

    let mut decoder = G722Decoder::new(G722BitRate::Rate48000);
    let mut first = [0i16; 200];
    let mut second = [0i16; 200];

    decoder.decode(&random_bytes(100), &mut first).unwrap();
    decoder.reset();
    decoder.decode(&random_bytes(100), &mut second).unwrap();
    assert_eq!(first, second);
    assert_eq!(decoder.bit_rate(), G722BitRate::Rate48000);
}

#[test]
fn configuration() {
    let decoder = G722Decoder::default();
    let encoder = G722Encoder::default();
    let configuration = decoder.configuration().unwrap();

    assert_eq!(configuration, encoder.configuration());
    assert_eq!(configuration.sample_rate, G722_SAMPLE_RATE);
    assert_eq!(configuration.sample_rate, G722_RTP_CLOCK_RATE * 2);
    assert_eq!(configuration.channels, 1);
    assert_eq!(configuration.format, Format::I16);
    assert_eq!((decoder.minimum_input(), decoder.minimum_output()), (1, 2));
    assert_eq!((encoder.minimum_input(), encoder.minimum_output()), (2, 1));

    for (bit_rate, (bits_per_second, mode)) in
        BIT_RATES
            .into_iter()
            .zip([(64_000, 1), (56_000, 2), (48_000, 3)])
    {
        assert_eq!(bit_rate.bits_per_second(), bits_per_second);
        assert_eq!(bit_rate.mode(), mode);
    }
}
