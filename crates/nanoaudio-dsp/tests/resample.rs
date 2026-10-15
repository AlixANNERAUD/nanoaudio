use nanoaudio_core::{Error, Format};
use nanoaudio_dsp::{
    Fixed, Float, Interpolation, Linear, Representation, Resampler, Sinc, Stream, samples_using,
    source_using,
};

const RATIOS: [(u32, u32); 6] = [
    (44_100, 48_000),
    (48_000, 44_100),
    (8_000, 48_000),
    (48_000, 8_000),
    (16_000, 44_100),
    (48_000, 16_000),
];

fn tone(rate: u32, frequency: f64, amplitude: f64, frames: usize, channels: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|index| {
            let phase = core::f64::consts::TAU * frequency * index as f64 / rate as f64;
            (0..channels).map(move |channel| (amplitude * (phase + channel as f64).sin()) as f32)
        })
        .collect()
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit as u64) as usize
    }

    fn signal(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

fn run<K, R, const CHANNELS: usize>(
    resampler: &mut Resampler<K, R, CHANNELS>,
    input: &[f32],
    seed: u64,
) -> Vec<f32>
where
    R: Representation,
    K: Interpolation<R, CHANNELS>,
{
    let channels = resampler.channels() as usize;
    let mut random = Random(seed);
    let mut output = Vec::new();
    let mut buffer = vec![0.0f32; 4096];
    let mut position = 0;

    while position < input.len() {
        let end = (position + channels * (1 + random.below(700))).min(input.len());
        let mut chunk = &input[position..end];

        loop {
            let room = channels * (1 + random.below(900));
            let progress = resampler.process(chunk, &mut buffer[..room]);
            output.extend_from_slice(&buffer[..progress.produced]);
            chunk = &chunk[progress.consumed..];

            if progress.consumed == 0 && progress.produced < room {
                break;
            }
        }

        assert!(chunk.is_empty());
        position = end;
    }

    output
}

fn fit(signal: &[f32], channels: usize, channel: usize, rate: u32, frequency: f64) -> (f64, f64) {
    let omega = core::f64::consts::TAU * frequency / rate as f64;
    let mut matrix = [[0.0f64; 3]; 3];
    let mut vector = [0.0f64; 3];
    let values: Vec<f64> = signal
        .iter()
        .skip(channel)
        .step_by(channels)
        .map(|value| *value as f64)
        .collect();
    let basis = |index: usize| {
        let angle = omega * index as f64;
        [angle.sin(), angle.cos(), 1.0]
    };

    for (index, value) in values.iter().enumerate() {
        let row = basis(index);
        for i in 0..3 {
            vector[i] += row[i] * value;
            for j in 0..3 {
                matrix[i][j] += row[i] * row[j];
            }
        }
    }

    let determinant = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let whole = determinant(matrix);
    let solution: [f64; 3] = core::array::from_fn(|unknown| {
        let mut replaced = matrix;
        for (row, value) in replaced.iter_mut().zip(vector) {
            row[unknown] = value;
        }
        determinant(replaced) / whole
    });

    let residual = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let row = basis(index);
            let error = value - (0..3).map(|i| row[i] * solution[i]).sum::<f64>();
            error * error
        })
        .sum::<f64>()
        / values.len() as f64;

    (solution[0].hypot(solution[1]), residual.sqrt())
}

fn decibels(value: f64) -> f64 {
    20.0 * value.log10()
}

fn sines<K, R>(name: &str, frequencies: &[f64], residual_limit: f64)
where
    R: Representation,
    K: Interpolation<R, 2>,
{
    for (input_rate, output_rate) in RATIOS {
        let lower = input_rate.min(output_rate) as f64;

        for frequency in frequencies.iter().map(|frequency| lower * frequency) {
            let amplitude = 0.5;
            let input = tone(input_rate, frequency, amplitude, input_rate as usize / 5, 2);
            let mut resampler = Resampler::<K, R>::new(2, input_rate, output_rate).unwrap();
            let output = run(&mut resampler, &input, 7);
            let skip =
                2 * ((2 * resampler.delay() + 2) * output_rate as usize / input_rate as usize + 80);

            for channel in 0..2 {
                let (fitted, residual) = fit(&output[skip..], 2, channel, output_rate, frequency);
                let gain = decibels(fitted / amplitude);
                let residual = decibels(residual / (amplitude / 2f64.sqrt()));

                assert!(
                    gain.abs() < 0.25,
                    "{name} {input_rate}->{output_rate} {frequency} Hz: gain {gain} dB"
                );
                assert!(
                    residual < residual_limit,
                    "{name} {input_rate}->{output_rate} {frequency} Hz: residual {residual} dB"
                );
            }
        }
    }
}

#[test]
fn sines_keep_their_frequency_and_amplitude() {
    sines::<Sinc, Float>("float sinc", &[0.0625, 0.37], -95.0);
    sines::<Sinc, Fixed>("fixed sinc", &[0.0625, 0.37], -75.0);
    sines::<Linear, Float>("float linear", &[0.0625], -40.0);
    sines::<Linear, Fixed>("fixed linear", &[0.0625], -40.0);
}

fn aliasing<R: Representation>(name: &str, limit: f64)
where
    Sinc: Interpolation<R, 1>,
{
    for (input_rate, output_rate) in [(48_000, 44_100), (48_000, 16_000), (44_100, 8_000)] {
        let nyquist = output_rate as f64 / 2.0;
        let top = input_rate as f64 / 2.0;
        let frames = input_rate as usize / 2;
        let start = nyquist * 1.02;
        let input: Vec<f32> = (0..frames)
            .map(|index| {
                let time = index as f64 / input_rate as f64;
                let duration = frames as f64 / input_rate as f64;
                let rate = (top * 0.99 - start) / duration;
                let phase = core::f64::consts::TAU * (start * time + rate * time * time / 2.0);
                (0.9 * phase.sin()) as f32
            })
            .collect();
        let mut resampler = Resampler::<Sinc, R, 1>::new(1, input_rate, output_rate).unwrap();
        let output = run(&mut resampler, &input, 3);
        let settled =
            &output[(2 * resampler.delay() + 2) * output_rate as usize / input_rate as usize..];
        let power = settled
            .iter()
            .map(|value| (*value as f64).powi(2))
            .sum::<f64>()
            / settled.len() as f64;
        let level = 10.0 * (power / (0.9f64 * 0.9 / 2.0)).log10();

        assert!(
            level < limit,
            "{name} {input_rate}->{output_rate}: aliasing at {level} dB"
        );
    }
}

#[test]
fn downsampling_rejects_everything_above_the_new_nyquist_frequency() {
    aliasing::<Float>("float", -95.0);
    aliasing::<Fixed>("fixed", -75.0);
}

fn direct_current<K, R>(name: &str, tolerance: f32)
where
    R: Representation,
    K: Interpolation<R, 2>,
{
    for (input_rate, output_rate) in RATIOS {
        let input = vec![0.25f32; 2 * input_rate as usize / 10];
        let mut resampler = Resampler::<K, R>::new(2, input_rate, output_rate).unwrap();
        let output = run(&mut resampler, &input, 11);
        let skip =
            2 * ((2 * resampler.delay() + 2) * output_rate as usize / input_rate as usize + 2);

        for value in &output[skip..] {
            assert!(
                (value - 0.25).abs() < tolerance,
                "{name} {input_rate}->{output_rate}: {value}"
            );
        }
    }
}

#[test]
fn direct_current_is_preserved() {
    direct_current::<Sinc, Float>("float sinc", 1e-4);
    direct_current::<Sinc, Fixed>("fixed sinc", 4e-4);
    direct_current::<Linear, Float>("float linear", 1e-6);
    direct_current::<Linear, Fixed>("fixed linear", 1e-4);
}

fn lengths<K, R>(frames: usize, extreme: bool)
where
    R: Representation,
    K: Interpolation<R, 2>,
{
    let extra: &[(u32, u32)] = if extreme {
        &[(1, 1), (3, 7), (999_983, 1), (1, 13)]
    } else {
        &[(1, 1), (3, 7), (7, 3)]
    };

    for &(input_rate, output_rate) in RATIOS.iter().chain(extra) {
        let mut random = Random(99);
        let input: Vec<f32> = (0..2 * frames).map(|_| random.signal() * 0.5).collect();
        let mut whole = Resampler::<K, R>::new(2, input_rate, output_rate).unwrap();
        let expected = whole.output_frames(frames);
        let mut buffer = vec![0.0f32; 2 * (expected + 1)];
        let progress = whole.process(&input, &mut buffer[..2 * expected]);
        let exact = (frames as u128 * output_rate as u128).div_ceil(input_rate as u128);

        assert_eq!(expected as u128, exact, "{input_rate}->{output_rate}");
        assert_eq!(progress.consumed, input.len());
        assert_eq!(progress.produced, 2 * expected);

        for seed in [1, 2] {
            let mut chunked = Resampler::<K, R>::new(2, input_rate, output_rate).unwrap();
            let output = run(&mut chunked, &input, seed);

            assert_eq!(output.len(), 2 * expected, "{input_rate}->{output_rate}");
            assert!(
                output == buffer[..2 * expected],
                "{input_rate}->{output_rate}"
            );
        }
    }
}

#[test]
fn lengths_are_exact_whatever_the_chunk_sizes() {
    lengths::<Linear, Float>(30_011, true);
    lengths::<Linear, Fixed>(30_011, true);
    lengths::<Sinc, Float>(8_009, false);
    lengths::<Sinc, Fixed>(8_009, false);
}

fn requests<K, R>()
where
    R: Representation,
    K: Interpolation<R, 2>,
{
    for (input_rate, output_rate) in RATIOS {
        let mut resampler = Resampler::<K, R>::new(2, input_rate, output_rate).unwrap();
        let mut random = Random(5);
        let input = vec![0.1f32; 2 * 20_000];
        let mut output = vec![0.0f32; 2 * 20_000];

        for _ in 0..200 {
            let wanted = random.below(600);
            let needed = resampler.input_frames(wanted);

            assert!(resampler.output_frames(needed) >= wanted);

            let progress = resampler.process(&input[..2 * needed], &mut output[..2 * wanted]);

            assert_eq!(progress.consumed, 2 * needed, "{input_rate}->{output_rate}");
            assert_eq!(progress.produced, 2 * wanted, "{input_rate}->{output_rate}");
        }
    }
}

#[test]
fn input_frames_give_exactly_the_requested_output() {
    requests::<Linear, Fixed>();
    requests::<Sinc, Float>();
}

fn identity<K, R>(delay: usize)
where
    R: Representation,
    K: Interpolation<R, 2>,
{
    let mut random = Random(17);
    let input: Vec<i16> = (0..2 * 5_000).map(|_| random.next() as i16).collect();
    let mut resampler = Resampler::<K, R>::new(2, 48_000, 48_000).unwrap();
    let mut output = vec![0i16; input.len()];

    assert_eq!(resampler.delay(), delay);

    let progress = resampler.process(&input, &mut output);

    assert_eq!(progress.consumed, input.len());
    assert_eq!(progress.produced, input.len());
    assert!(output[..2 * delay].iter().all(|sample| *sample == 0));
    assert_eq!(output[2 * delay..], input[..input.len() - 2 * delay]);
}

#[test]
fn equal_rates_delay_the_input_unchanged() {
    identity::<Linear, Fixed>(1);
    identity::<Linear, Float>(1);
    identity::<Sinc, Fixed>(34);
    identity::<Sinc, Float>(34);
}

fn drift<K, R>(name: &str, limit: f64)
where
    R: Representation,
    K: Interpolation<R, 1>,
{
    let frequency = 1_000.0;
    let input = tone(44_100, frequency, 0.5, 44_100, 1);
    let mut resampler = Resampler::<K, R, 1>::new(1, 44_100, 48_000).unwrap();
    let mut output = Vec::new();
    let mut buffer = [0.0f32; 512];
    let rates = [
        (4_410_000, 4_800_480),
        (4_410_000, 4_799_520),
        (44_100, 48_000),
    ];

    for (index, chunk) in input.chunks(441).enumerate() {
        if index % 20 == 19 {
            resampler
                .set_rates(rates[index / 20 % 3].0, rates[index / 20 % 3].1)
                .unwrap();
        }

        let progress = resampler.process(chunk, &mut buffer);
        assert_eq!(progress.consumed, chunk.len());
        output.extend_from_slice(&buffer[..progress.produced]);
    }

    let coefficient = 2.0 * (core::f64::consts::TAU * frequency / 48_000.0).cos();
    let worst = output[200..]
        .windows(3)
        .map(|window| (window[2] as f64 - coefficient * window[1] as f64 + window[0] as f64).abs())
        .fold(0.0, f64::max);

    let worst = decibels(worst / 0.5);

    assert!(worst < limit, "{name}: {worst} dB");
}

#[test]
fn small_ratio_changes_do_not_click() {
    drift::<Sinc, Float>("float sinc", -85.0);
    drift::<Sinc, Fixed>("fixed sinc", -55.0);
    drift::<Linear, Float>("float linear", -50.0);
}

#[test]
fn large_ratio_changes_lengthen_the_delay() {
    let mut resampler = Resampler::<Sinc, Float, 1>::new(1, 44_100, 48_000).unwrap();
    let input = vec![0.5f32; 10_000];
    let mut output = vec![0.0f32; 20_000];

    resampler.process(&input, &mut output);
    assert_eq!(resampler.delay(), 34);

    resampler.set_rates(48_000, 16_000).unwrap();
    assert_eq!(resampler.delay(), 98);
    assert_eq!(resampler.rates(), (3, 1));

    let progress = resampler.process(&input, &mut output);

    assert_eq!(progress.consumed, input.len());
    assert!(
        output[..progress.produced]
            .iter()
            .all(|value| (value - 0.5).abs() < 1e-3)
    );

    assert_eq!(
        resampler.set_rates(48_000, 1_000),
        Err(Error::InvalidParameter)
    );
    assert_eq!(resampler.rates(), (3, 1));
}

#[test]
fn full_scale_input_does_not_overflow() {
    for (input_rate, output_rate) in RATIOS.into_iter().chain([(96_000, 22_050)]) {
        let mut random = Random(23);
        let input: Vec<i16> = (0..4_000)
            .map(|index| match index % 3 {
                0 => i16::MAX,
                1 => i16::MIN,
                _ => random.next() as i16,
            })
            .collect();
        let mut fixed = Resampler::<Sinc, Fixed, 1>::new(1, input_rate, output_rate).unwrap();
        let mut float = Resampler::<Sinc, Float, 1>::new(1, input_rate, output_rate).unwrap();
        let mut fixed_output = vec![0i16; 30_000];
        let mut float_output = vec![0i16; 30_000];

        let fixed_progress = fixed.process(&input, &mut fixed_output);
        let float_progress = float.process(&input, &mut float_output);

        assert_eq!(fixed_progress, float_progress);

        for (fixed, float) in fixed_output.iter().zip(&float_output) {
            assert!(
                (*fixed as i32 - *float as i32).abs() < 64,
                "{fixed} {float}"
            );
        }
    }
}

#[test]
fn stream_stage_matches_the_slice_function() {
    let input = tone(44_100, 3_000.0, 0.7, 4_410, 2);
    let bytes: Vec<u8> = input
        .iter()
        .flat_map(|sample| ((sample * 32_767.0) as i16).to_le_bytes())
        .collect();
    let samples: Vec<i16> = input
        .iter()
        .map(|sample| (sample * 32_767.0) as i16)
        .collect();

    let mut slice = Resampler::<Sinc, Float>::new(2, 44_100, 48_000).unwrap();
    let mut expected = vec![0i16; 2 * 4_800];
    let progress = slice.process(&samples, &mut expected);
    expected.truncate(progress.produced);

    let mut staged = Resampler::<Sinc, Float>::new(2, 44_100, 48_000).unwrap();
    let mut actual = Vec::new();

    for chunk in bytes.chunks(4 * 97) {
        let mut buffer = [0i16; 2 * 128];

        loop {
            let frames = source_using::<Float>(chunk, Format::I16, 2)
                .unwrap()
                .resample(&mut staged)
                .unwrap()
                .write_samples(&mut buffer);

            actual.extend_from_slice(&buffer[..2 * frames]);

            if frames < 128 {
                break;
            }
        }
    }

    assert_eq!(actual.len(), expected.len());
    assert_eq!(actual, expected);
}

#[test]
fn rejects_invalid_parameters() {
    assert_eq!(
        Resampler::<Linear, Float>::new(0, 1, 1).err(),
        Some(Error::TooManyChannels)
    );
    assert_eq!(
        Resampler::<Linear, Float>::new(3, 1, 1).err(),
        Some(Error::TooManyChannels)
    );
    assert_eq!(
        Resampler::<Linear, Float>::new(1, 0, 1).err(),
        Some(Error::InvalidParameter)
    );
    assert_eq!(
        Resampler::<Sinc<70>, Float>::new(1, 48_000, 44_100).err(),
        Some(Error::InvalidParameter)
    );
    assert!(Resampler::<Sinc<70>, Fixed>::new(1, 44_100, 48_000).is_ok());
    assert!(Resampler::<Sinc, Fixed>::new(2, 48_000, 6_100).is_ok());
    assert!(Resampler::<Sinc, Fixed>::new(2, 48_000, 6_000).is_err());

    let mut resampler = Resampler::<Linear, Fixed>::new(1, 8_000, 16_000).unwrap();
    assert!(
        samples_using::<Fixed, i16>(&[0; 4], 2)
            .unwrap()
            .resample(&mut resampler)
            .is_err()
    );
}

#[test]
fn reset_starts_over() {
    let input = tone(16_000, 440.0, 0.5, 1_600, 2);
    let mut resampler = Resampler::<Sinc, Float>::new(2, 16_000, 44_100).unwrap();
    let first = run(&mut resampler, &input, 1);

    resampler.reset();

    assert_eq!(run(&mut resampler, &input, 2), first);
}
