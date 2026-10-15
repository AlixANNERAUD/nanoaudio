#![cfg(feature = "mp3")]

use nanoaudio::Configuration;
use nanoaudio::Format;
use nanoaudio::codec::{Decoder, Mp3Decoder, Mp3Sample};

fn decode<S: nanoaudio::Sample + Mp3Sample>(file: &[u8], window: usize) -> (Configuration, Vec<S>) {
    decode_with(Mp3Decoder::new(), file, window)
}

fn decode_with<S: nanoaudio::Sample + Mp3Sample>(
    mut decoder: Mp3Decoder<S>,
    file: &[u8],
    window: usize,
) -> (Configuration, Vec<S>) {
    let mut output = vec![S::SILENCE; decoder.minimum_output()];
    let mut decoded = Vec::new();
    let mut position = 0;

    loop {
        let end = (position + window).min(file.len());

        if end == file.len() {
            decoder.finish();
        }

        let progress = decoder.decode(&file[position..end], &mut output).unwrap();
        decoded.extend_from_slice(&output[..progress.produced]);
        position += progress.consumed;

        if progress.is_stalled() && end == file.len() {
            break;
        }
    }

    (decoder.configuration().unwrap(), decoded)
}

fn power(samples: &[f64], sample_rate: f64, frequency: f64) -> f64 {
    let coefficient = 2.0 * (core::f64::consts::TAU * frequency / sample_rate).cos();
    let (mut previous, mut before) = (0.0, 0.0);

    for sample in samples {
        let current = sample + coefficient * previous - before;
        before = previous;
        previous = current;
    }

    previous * previous + before * before - coefficient * previous * before
}

fn channel(samples: &[i16], channels: usize, channel: usize) -> Vec<f64> {
    samples
        .chunks_exact(channels)
        .map(|frame| frame[channel] as f64)
        .collect()
}

#[test]
fn mono() {
    let (configuration, samples) = decode::<i16>(include_bytes!("data/mono_440.mp3"), 16 * 1024);

    assert_eq!(configuration, Configuration::new(16_000, 1, Format::I16));
    assert!(samples.len() >= 8_000, "{}", samples.len());

    let middle = channel(&samples[2_000..6_000], 1, 0);
    assert!(power(&middle, 16_000.0, 440.0) > 100.0 * power(&middle, 16_000.0, 1_000.0));
}

#[test]
fn stereo_in_small_windows() {
    let file = include_bytes!("data/stereo_440_1000.mp3");
    let (reference_configuration, reference) = decode::<i16>(file, file.len());

    let decoder = Mp3Decoder::<i16>::with_minimum_input(2_048);
    assert_eq!(decoder.minimum_input(), 2_048);

    let (configuration, samples) = decode_with(decoder, file, 2_048);

    assert_eq!(configuration, reference_configuration);
    assert_eq!(configuration, Configuration::new(44_100, 2, Format::I16));
    assert_eq!(samples, reference);
    assert!(samples.len() >= 44_100);

    let frames = 4_000..20_000;
    let left = channel(&samples[frames.start * 2..frames.end * 2], 2, 0);
    let right = channel(&samples[frames.start * 2..frames.end * 2], 2, 1);

    assert!(power(&left, 44_100.0, 440.0) > 100.0 * power(&left, 44_100.0, 1_000.0));
    assert!(power(&right, 44_100.0, 1_000.0) > 100.0 * power(&right, 44_100.0, 440.0));
}

#[test]
fn float_output_matches_integer_output() {
    let file = include_bytes!("data/mono_440.mp3");
    let (_, integers) = decode::<i16>(file, file.len());
    let (configuration, floats) = decode::<f32>(file, file.len());

    assert_eq!(configuration.format, Format::F32);
    assert_eq!(floats.len(), integers.len());

    for (float, integer) in floats.iter().zip(&integers) {
        assert!((float * 32_768.0 - *integer as f32).abs() <= 1.0);
    }
}

#[test]
fn waits_for_enough_input() {
    let file = include_bytes!("data/stereo_440_1000.mp3");
    let mut decoder = Mp3Decoder::<i16>::new();
    let mut output = vec![0i16; decoder.minimum_output()];

    assert!(
        decoder
            .decode(&file[..4_096], &mut output)
            .unwrap()
            .is_stalled()
    );
    assert!(decoder.decode(file, &mut output[..100]).is_err());

    decoder.finish();
    assert!(
        !decoder
            .decode(&file[..4_096], &mut output)
            .unwrap()
            .is_stalled()
    );
}
