use std::f64::consts::TAU;

use nanoaudio_synth::{Oscillator, Waveform, frequency_increment};

const SAMPLE_RATE: u32 = 44_100;
const LENGTH: usize = 1 << 14;

fn fft(real: &mut [f64], imaginary: &mut [f64]) {
    let length = real.len();
    let mut target = 0;

    for index in 0..length {
        if index < target {
            real.swap(index, target);
            imaginary.swap(index, target);
        }

        let mut bit = length >> 1;
        while target & bit != 0 {
            target ^= bit;
            bit >>= 1;
        }
        target |= bit;
    }

    let mut size = 2;
    while size <= length {
        let angle = -TAU / size as f64;

        for start in (0..length).step_by(size) {
            for offset in 0..size / 2 {
                let (sine, cosine) = (angle * offset as f64).sin_cos();
                let even = start + offset;
                let odd = even + size / 2;
                let odd_real = real[odd] * cosine - imaginary[odd] * sine;
                let odd_imaginary = real[odd] * sine + imaginary[odd] * cosine;
                real[odd] = real[even] - odd_real;
                imaginary[odd] = imaginary[even] - odd_imaginary;
                real[even] += odd_real;
                imaginary[even] += odd_imaginary;
            }
        }

        size <<= 1;
    }
}

fn alias_ratio(samples: &[f64], frequency: f64) -> f64 {
    let mut real: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let position = TAU * index as f64 / LENGTH as f64;
            let window = 0.35875 - 0.48829 * position.cos() + 0.14128 * (2.0 * position).cos()
                - 0.01168 * (3.0 * position).cos();
            sample * window
        })
        .collect();
    let mut imaginary = vec![0.0; LENGTH];
    fft(&mut real, &mut imaginary);

    let bin_width = SAMPLE_RATE as f64 / LENGTH as f64;
    let harmonics = (SAMPLE_RATE as f64 / 2.0 / frequency) as usize;
    let mut signal = 0.0;
    let mut alias = 0.0;

    for bin in 1..LENGTH / 2 {
        let power = real[bin] * real[bin] + imaginary[bin] * imaginary[bin];
        let bin_frequency = bin as f64 * bin_width;
        let harmonic = (1..=harmonics)
            .any(|index| (bin_frequency - index as f64 * frequency).abs() <= 4.0 * bin_width);

        if harmonic {
            signal += power;
        } else {
            alias += power;
        }
    }

    10.0 * (alias / signal).log10()
}

fn band_limited(waveform: Waveform, frequency: f32) -> Vec<f64> {
    let mut oscillator = Oscillator::new(waveform);
    oscillator.set_frequency(frequency, SAMPLE_RATE);

    (0..LENGTH)
        .map(|_| oscillator.next_sample() as f64 / 32_768.0)
        .collect()
}

fn naive(waveform: Waveform, frequency: f32) -> Vec<f64> {
    let increment = frequency_increment(frequency, SAMPLE_RATE);
    let mut phase = 0u32;

    (0..LENGTH)
        .map(|_| {
            let value = match waveform {
                Waveform::Saw => phase as f64 / 2_147_483_648.0 - 1.0,
                _ => {
                    if phase < 1 << 31 {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };
            phase = phase.wrapping_add(increment);
            value
        })
        .collect()
}

#[test]
fn polyblep_lowers_aliasing() {
    for (waveform, frequency, gain) in [
        (Waveform::Saw, 440.0, 15.0),
        (Waveform::Saw, 1_760.0, 15.0),
        (Waveform::Saw, 4_186.0, 10.0),
        (Waveform::Square, 440.0, 15.0),
        (Waveform::Square, 1_760.0, 15.0),
        (Waveform::Square, 4_186.0, 10.0),
    ] {
        let reference = alias_ratio(&naive(waveform, frequency), frequency as f64);
        let actual = alias_ratio(&band_limited(waveform, frequency), frequency as f64);

        println!("{waveform:?} at {frequency} Hz: naive {reference:.1} dB, PolyBLEP {actual:.1} dB");
        assert!(actual < reference - gain, "{waveform:?} {frequency}");
    }
}

#[test]
fn triangle_and_sine_alias_little() {
    for (waveform, frequency, limit) in [
        (Waveform::Triangle, 440.0, -50.0),
        (Waveform::Triangle, 1_760.0, -38.0),
        (Waveform::Sine, 4_186.0, -80.0),
    ] {
        let actual = alias_ratio(&band_limited(waveform, frequency), frequency as f64);

        println!("{waveform:?} at {frequency} Hz: {actual:.1} dB");
        assert!(actual < limit, "{waveform:?} {actual}");
    }
}
