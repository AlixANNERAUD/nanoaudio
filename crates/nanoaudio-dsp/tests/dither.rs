use nanoaudio_core::{Format, I24, Sample};
use nanoaudio_dsp::{
    Fixed, Float, Representation, Stream, convert, convert_bytes_dithered, convert_dithered,
};

const RATE: usize = 48_000;
const PERIOD: usize = 48;
const FRAMES: usize = 96_000;

struct Tone<R: Representation> {
    table: [f32; PERIOD],
    channels: u8,
    position: usize,
    remaining: usize,
    representation: core::marker::PhantomData<R>,
}

impl<R: Representation> Tone<R> {
    fn new(amplitude: f32, channels: u8, frames: usize) -> Self {
        Self {
            table: core::array::from_fn(|index| {
                amplitude * (core::f32::consts::TAU * index as f32 / PERIOD as f32).sin()
            }),
            channels,
            position: 0,
            remaining: frames,
            representation: core::marker::PhantomData,
        }
    }
}

impl<R: Representation> Stream for Tone<R> {
    type Representation = R;

    fn channels(&self) -> u8 {
        self.channels
    }

    fn next_frame(&mut self, frame: &mut [R::Value]) -> bool {
        if self.remaining == 0 {
            return false;
        }

        frame.fill(R::from_sample(self.table[self.position]));
        self.position = (self.position + 1) % PERIOD;
        self.remaining -= 1;
        true
    }
}

fn collect<St: Stream, S: Sample>(mut stream: St) -> Vec<S> {
    let mut output = Vec::new();
    let mut buffer = [S::SILENCE; 256];

    loop {
        let channels = stream.channels() as usize;
        let frames = stream.write_samples(&mut buffer);

        if frames == 0 {
            return output;
        }

        output.extend_from_slice(&buffer[..frames * channels]);
    }
}

fn least_significant_bit(format: Format) -> f64 {
    match format {
        Format::U8 => 2f64.powi(-7),
        Format::I16 => 2f64.powi(-15),
        Format::I24 => 2f64.powi(-23),
        Format::I32 | Format::F32 => 2f64.powi(-31),
    }
}

fn noise<R: Representation>(target: Format, seed: u32) -> Vec<f64> {
    let lsb = least_significant_bit(target);

    collect::<_, f32>(Tone::<R>::new(0.0, 2, FRAMES).dither(target, seed))
        .into_iter()
        .map(|value| value as f64 / lsb)
        .collect()
}

fn check_statistics(noise: &[f64]) {
    let count = noise.len() as f64;
    let mean = noise.iter().sum::<f64>() / count;
    let variance = noise
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / count;
    let tolerance = 0.02;

    assert!(mean.abs() < tolerance, "mean {mean}");
    assert!(
        (variance * 6.0 - 1.0).abs() < tolerance * 3.0,
        "variance {variance}"
    );
    assert!(noise.iter().all(|value| value.abs() < 1.0));
    assert!(noise.iter().any(|value| value.abs() > 0.9));

    let near_zero = noise.iter().filter(|value| value.abs() < 0.5).count() as f64;
    assert!((near_zero / count - 0.75).abs() < tolerance, "{near_zero}");
}

#[test]
fn noise_is_triangular_with_one_lsb_amplitude() {
    for target in [Format::U8, Format::I16] {
        check_statistics(&noise::<Fixed>(target, 1));
        check_statistics(&noise::<Float>(target, 1));
    }

    check_statistics(&noise::<Fixed>(Format::I24, 7));
}

#[test]
fn full_resolution_targets_are_left_alone() {
    for target in [Format::I32, Format::F32] {
        assert!(noise::<Fixed>(target, 1).iter().all(|value| *value == 0.0));
        assert!(noise::<Float>(target, 1).iter().all(|value| *value == 0.0));
    }
}

fn power(samples: impl Iterator<Item = f64>, frequency: f64) -> f64 {
    let coefficient = 2.0 * (core::f64::consts::TAU * frequency / RATE as f64).cos();
    let (mut previous, mut before) = (0.0, 0.0);

    for sample in samples {
        let current = sample + coefficient * previous - before;
        before = previous;
        previous = current;
    }

    previous * previous + before * before - coefficient * previous * before
}

fn harmonics(samples: &[i16]) -> (f64, f64) {
    let fundamental = (RATE / PERIOD) as f64;
    let values = || samples.iter().map(|sample| *sample as f64);

    (
        power(values(), fundamental),
        power(values(), fundamental * 3.0),
    )
}

fn removes_harmonic_distortion<R: Representation>() {
    let amplitude = 1.3 / 32_768.0;
    let plain = collect::<_, i16>(Tone::<R>::new(amplitude, 1, FRAMES));
    let dithered = collect::<_, i16>(Tone::<R>::new(amplitude, 1, FRAMES).dither(Format::I16, 42));

    let (plain_fundamental, plain_third) = harmonics(&plain);
    let (dithered_fundamental, dithered_third) = harmonics(&dithered);

    let ideal = (FRAMES as f64 * 1.3 / 2.0).powi(2);

    assert!(plain_third > plain_fundamental / 1_000.0);
    assert!(dithered_third < plain_third / 100.0);
    assert!((dithered_fundamental / ideal - 1.0).abs() < 0.05);
}

#[test]
fn removes_harmonic_distortion_from_quiet_tones() {
    removes_harmonic_distortion::<Fixed>();
    removes_harmonic_distortion::<Float>();
}

#[test]
fn is_deterministic_for_a_seed() {
    let first = collect::<_, i16>(Tone::<Fixed>::new(0.01, 2, 1_000).dither(Format::I16, 5));
    let second = collect::<_, i16>(Tone::<Fixed>::new(0.01, 2, 1_000).dither(Format::I16, 5));
    let other = collect::<_, i16>(Tone::<Fixed>::new(0.01, 2, 1_000).dither(Format::I16, 6));

    assert_eq!(first, second);
    assert_ne!(first, other);
    assert!(
        first
            .as_chunks::<2>()
            .0
            .iter()
            .any(|[left, right]| left != right)
    );
}

#[test]
fn matches_across_representations() {
    for target in [Format::U8, Format::I16, Format::I24] {
        let fixed = noise::<Fixed>(target, 9);
        let float = noise::<Float>(target, 9);

        assert!(
            fixed
                .iter()
                .zip(&float)
                .all(|(fixed, float)| (fixed - float).abs() < 1e-3)
        );
    }

    let fixed = collect::<_, i16>(Tone::<Fixed>::new(0.001, 1, 10_000).dither(Format::I16, 3));
    let float = collect::<_, i16>(Tone::<Float>::new(0.001, 1, 10_000).dither(Format::I16, 3));
    let matching = fixed
        .iter()
        .zip(&float)
        .filter(|(fixed, float)| fixed == float)
        .count();

    assert!(
        fixed
            .iter()
            .zip(&float)
            .all(|(fixed, float)| fixed.abs_diff(*float) <= 1)
    );
    assert!(matching > 9_900);
}

#[test]
fn slices_continue_the_noise_between_calls() {
    let input: Vec<f32> = (0..2_000)
        .map(|index| (index as f32 * 0.01).sin() * 0.001)
        .collect();
    let mut whole = vec![0i16; 2_000];
    let mut split = vec![0i16; 2_000];
    let mut state = 11;

    assert_eq!(convert_dithered(&input, &mut whole, &mut 11), 2_000);
    assert_eq!(
        convert_dithered(&input[..700], &mut split[..700], &mut state),
        700
    );
    assert_eq!(
        convert_dithered(&input[700..], &mut split[700..], &mut state),
        1_300
    );
    assert_eq!(whole, split);

    let mut plain = vec![0i16; 2_000];
    convert(&input, &mut plain);
    assert_ne!(whole, plain);
    assert!(
        whole
            .iter()
            .zip(&plain)
            .all(|(dithered, plain)| dithered.abs_diff(*plain) <= 1)
    );
}

#[test]
fn bytes_match_typed_slices() {
    let input: Vec<i32> = (0..500).map(|index| index * 40_000 - 10_000_000).collect();
    let bytes: Vec<u8> = input
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    let mut typed = vec![I24::SILENCE; 500];
    let mut output = vec![0u8; 1_500];

    assert_eq!(convert_dithered(&input, &mut typed, &mut 3), 500);
    assert_eq!(
        convert_bytes_dithered(&bytes, Format::I32, &mut output, Format::I24, &mut 3),
        500
    );
    assert!(
        typed
            .iter()
            .zip(output.as_chunks::<3>().0)
            .all(|(typed, bytes)| *typed == I24::read_le(bytes.as_slice()))
    );
}
