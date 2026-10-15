use nanoaudio_core::Configuration;
use nanoaudio_core::Format;
use nanoaudio_core::codec::Decoder;
use nanoaudio_mp3::{MP3_DEFAULT_MINIMUM_INPUT, Mp3Decoder, Mp3Sample};

const FIXTURES: [&[u8]; 6] = [
    include_bytes!("data/mono_440.mp3"),
    include_bytes!("data/stereo_440_1000.mp3"),
    include_bytes!("data/seek_cbr_info.mp3"),
    include_bytes!("data/seek_cbr_low.mp3"),
    include_bytes!("data/seek_vbr_xing.mp3"),
    include_bytes!("data/seek_vbr_vbri.mp3"),
];

const WINDOWS: [usize; 4] = [1_445, 1_800, 2_048, MP3_DEFAULT_MINIMUM_INPUT];

fn decode<S: nanoaudio_core::Sample + Mp3Sample>(
    file: &[u8],
    window: usize,
) -> (Configuration, Vec<S>) {
    decode_with(Mp3Decoder::new(), file, window)
}

fn decode_with<S: nanoaudio_core::Sample + Mp3Sample>(
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
            .decode(&file[..MP3_DEFAULT_MINIMUM_INPUT - 1], &mut output)
            .unwrap()
            .is_stalled()
    );
    assert!(decoder.decode(file, &mut output[..100]).is_err());

    decoder.finish();
    assert!(
        !decoder
            .decode(&file[..MP3_DEFAULT_MINIMUM_INPUT - 1], &mut output)
            .unwrap()
            .is_stalled()
    );
}

fn stream(window: usize, file: &[u8], chunk: usize) -> Vec<i16> {
    let mut decoder = Mp3Decoder::<i16>::with_minimum_input(window);
    let mut output = vec![0; decoder.minimum_output()];
    let mut decoded = Vec::new();
    let mut position = 0;
    let mut end = chunk.min(file.len());

    loop {
        if end == file.len() {
            decoder.finish();
        }

        let progress = decoder.decode(&file[position..end], &mut output).unwrap();

        assert!(end - position <= window.max(chunk) + chunk);
        decoded.extend_from_slice(&output[..progress.produced]);
        position += progress.consumed;

        if progress.is_stalled() {
            if end == file.len() {
                return decoded;
            }

            end = (end + chunk).min(file.len());
        }
    }
}

#[test]
fn small_windows_match_a_single_pass() {
    for file in FIXTURES {
        let (_, reference) = decode::<i16>(file, file.len());

        assert!(!reference.is_empty());

        for window in WINDOWS {
            for chunk in [1, 7, 333, window] {
                assert!(
                    stream(window, file, chunk) == reference,
                    "window {window}, chunk {chunk}"
                );
            }
        }
    }
}

#[test]
fn small_windows_skip_junk_without_losing_frames() {
    let audio = include_bytes!("data/stereo_440_1000.mp3");

    for length in [100, 1_500, 3_000, 3_900, 4_000, 5_000, 9_000] {
        for byte in [0x00, 0x55, 0xFF] {
            let mut file = vec![byte; length];

            file.extend_from_slice(audio);

            let (_, reference) = decode::<i16>(&file, file.len());

            assert!(reference.len() >= 44_100);

            for window in WINDOWS {
                for chunk in [7, 512] {
                    assert!(
                        stream(window, &file, chunk) == reference,
                        "{length} bytes of {byte:#x}, window {window}, chunk {chunk}"
                    );
                }
            }
        }
    }
}

#[test]
fn skips_headers_with_a_reserved_layer() {
    let audio = include_bytes!("data/stereo_440_1000.mp3");
    let (_, reference) = decode::<i16>(audio, audio.len());
    let mut file = [0xFF, 0xF1, 0x90, 0x44].repeat(300);

    file.extend_from_slice(audio);

    for window in WINDOWS {
        assert!(stream(window, &file, 512) == reference, "window {window}");
    }
}
