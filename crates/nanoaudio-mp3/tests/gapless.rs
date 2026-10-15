use nanoaudio_core::Error;
use nanoaudio_core::codec::{Decoder, Seek};
use nanoaudio_mp3::{MP3_DEFAULT_MINIMUM_INPUT, Mp3Decoder};

struct Fixture {
    file: &'static [u8],
    sample_rate: u32,
    channels: usize,
    frames: usize,
    block: usize,
    blocks: usize,
    constant: bool,
}

const FIXTURES: [Fixture; 3] = [
    Fixture {
        file: include_bytes!("data/gapless_cbr.mp3"),
        sample_rate: 44_100,
        channels: 2,
        frames: 30_000,
        block: 1152,
        blocks: 28,
        constant: true,
    },
    Fixture {
        file: include_bytes!("data/gapless_low.mp3"),
        sample_rate: 22_050,
        channels: 1,
        frames: 20_000,
        block: 576,
        blocks: 37,
        constant: true,
    },
    Fixture {
        file: include_bytes!("data/gapless_vbr.mp3"),
        sample_rate: 44_100,
        channels: 2,
        frames: 40_000,
        block: 1152,
        blocks: 36,
        constant: false,
    },
];

const ENCODER_DELAY: usize = 576;
const DECODER_DELAY: usize = 529;

fn signal(fixture: &Fixture) -> Vec<i16> {
    let mut state: u64 = 1789;
    let mut samples = Vec::new();

    for index in 0..fixture.frames {
        let time = index as f64 / f64::from(fixture.sample_rate);

        for channel in 0..fixture.channels {
            state = (state * 1_103_515_245 + 12_345) % (1 << 31);

            let noise = ((state >> 16) % 4001) as f64 - 2000.0;
            let frequency = 200.0 + 1800.0 * time + 300.0 * channel as f64;
            let tone = 6000.0 * (core::f64::consts::TAU * frequency * time).sin();

            samples.push((tone + noise) as i16);
        }
    }

    samples
}

fn feed(decoder: &mut Mp3Decoder, input: &[u8], chunk: usize) -> Vec<i16> {
    let mut output = vec![0; decoder.minimum_output()];
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

fn decode(file: &[u8], gapless: bool) -> Vec<i16> {
    let mut decoder = Mp3Decoder::new();

    decoder.set_gapless(gapless);

    feed(&mut decoder, file, 512)
}

fn correlation(decoded: &[i16], original: &[i16]) -> f64 {
    let (mut product, mut energy) = (0.0, 0.0);

    for (decoded, original) in decoded.iter().zip(original) {
        product += f64::from(*decoded) * f64::from(*original);
        energy += f64::from(*decoded) * f64::from(*decoded);
    }

    product / energy.sqrt()
}

fn best_lag(full: &[i16], original: &[i16], position: usize, channels: usize) -> isize {
    let span = 1152 * channels;
    let original = &original[position * channels..][..span];

    (-8..=8)
        .max_by(|first: &isize, second: &isize| {
            let at = |lag: isize| {
                let start = (position as isize + lag) as usize * channels;

                correlation(&full[start..start + span], original)
            };

            at(*first).total_cmp(&at(*second))
        })
        .unwrap()
}

#[test]
fn output_is_the_encoded_audio() {
    for fixture in &FIXTURES {
        let original = signal(fixture);
        let channels = fixture.channels;
        let trimmed = decode(fixture.file, true);
        let full = decode(fixture.file, false);
        let lead = fixture.block + ENCODER_DELAY + DECODER_DELAY;

        assert_eq!(full.len(), (fixture.blocks + 1) * fixture.block * channels);
        assert_eq!(trimmed.len(), fixture.frames * channels);
        assert_eq!(trimmed, full[lead * channels..][..trimmed.len()]);

        let shifted = &full[lead * channels..];

        assert_eq!(best_lag(shifted, &original, 8, channels), 0);
        assert_eq!(
            best_lag(shifted, &original, fixture.frames - 1152, channels),
            0
        );

        let (mut noise, mut power) = (0.0, 0.0);

        for (decoded, original) in trimmed.iter().zip(&original) {
            noise += (f64::from(*decoded) - f64::from(*original)).powi(2);
            power += f64::from(*original).powi(2);
        }

        assert!(10.0 * (power / noise).log10() > 10.0);
    }
}

#[test]
fn reports_the_trimmed_length() {
    for fixture in &FIXTURES {
        for gapless in [true, false] {
            let mut decoder = Mp3Decoder::new();

            decoder.set_gapless(gapless);
            assert_eq!(decoder.is_gapless(), gapless);
            assert_eq!(decoder.frames(), None);

            let decoded = feed(&mut decoder, fixture.file, 512);

            assert_eq!(
                decoder.frames(),
                Some((decoded.len() / fixture.channels) as u64)
            );
        }
    }
}

fn prime(decoder: &mut Mp3Decoder, input: &[u8]) {
    let mut output = vec![0; decoder.minimum_output()];

    decoder.decode(input, &mut output).unwrap();
}

#[test]
fn seeks_to_exact_positions_in_constant_bitrate_streams() {
    for (fixture, gapless) in FIXTURES
        .iter()
        .filter(|fixture| fixture.constant)
        .flat_map(|fixture| [(fixture, true), (fixture, false)])
    {
        let trimmed = decode(fixture.file, gapless);
        let (frames, block) = (trimmed.len() / fixture.channels, fixture.block);
        let mut targets = vec![0, 1, 2, frames / 3, frames / 2, frames - 1, frames];

        for multiple in 1..=4 {
            targets.extend([multiple * block - 1, multiple * block, multiple * block + 1]);
        }

        for offset in [2, 529, 530, 1105, 1106, 1152, 1153, 2000, 3000] {
            targets.push(frames - offset);
        }

        for input_window in [MP3_DEFAULT_MINIMUM_INPUT, 1_445] {
            for (index, target) in targets.iter().enumerate() {
                let chunk = [7, 512, usize::MAX][index % 3];
                let mut decoder = Mp3Decoder::with_minimum_input(input_window);

                decoder.set_gapless(gapless);
                prime(&mut decoder, fixture.file);

                let request = decoder.seek(*target as u64).unwrap();
                let decoded = feed(
                    &mut decoder,
                    &fixture.file[(request.byte_offset as usize).min(fixture.file.len())..],
                    chunk,
                );

                assert!(
                    decoded == trimmed[target * fixture.channels..],
                    "frame {target}, chunk {chunk}, gapless {gapless}: {} samples instead of {}",
                    decoded.len(),
                    trimmed.len() - target * fixture.channels
                );
            }

            let mut decoder = Mp3Decoder::with_minimum_input(input_window);

            decoder.set_gapless(gapless);
            prime(&mut decoder, fixture.file);

            assert_eq!(
                decoder.seek(frames as u64 + 1),
                Err(Error::InvalidParameter)
            );
        }
    }
}

#[test]
fn seeks_near_the_target_in_variable_bitrate_streams() {
    let fixture = &FIXTURES[2];
    let trimmed = decode(fixture.file, true);
    let channels = fixture.channels;

    for target in [0, 1, 1_000, 10_000, 20_000, 30_000, 35_000] {
        let mut decoder = Mp3Decoder::new();

        prime(&mut decoder, fixture.file);

        let request = decoder.seek(target as u64).unwrap();
        let decoded = feed(
            &mut decoder,
            &fixture.file[(request.byte_offset as usize).min(fixture.file.len())..],
            512,
        );
        let start = (-2..=2)
            .map(|frames: isize| target as isize + frames * fixture.block as isize)
            .filter(|start| (0..=fixture.frames as isize).contains(start))
            .find(|start| {
                let expected = &trimmed[*start as usize * channels..];
                let length = decoded
                    .len()
                    .min(expected.len())
                    .saturating_sub(1152 * channels);

                decoded[..length] == expected[..length]
            });

        assert!(start.is_some(), "frame {target}");
        assert_eq!(decoded.len(), (fixture.frames - target) * channels);
    }
}
