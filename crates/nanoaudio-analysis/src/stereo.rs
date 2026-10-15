use core::f32::consts::{LN_2, LOG10_E, SQRT_2};
use core::fmt::Debug;

use nanoaudio_dsp::{Analyzer, DefaultRepresentation, Fixed, Float};

use crate::Measure;

const LEFT: usize = 0;
const RIGHT: usize = 1;
const PRODUCT: usize = 2;

mod sealed {
    pub trait Sealed {}

    impl Sealed for nanoaudio_dsp::Fixed {}
    impl Sealed for nanoaudio_dsp::Float {}
}

/// A [`Measure`] that a [`StereoMeter`] can accumulate, implemented for [`Fixed`] and [`Float`].
///
/// [`Fixed`] sums the products of the top 16 bits of each value in `i64`, with no floating point. [`Float`] sums in `f32`.
pub trait Correlate: Measure + Debug + PartialEq + sealed::Sealed {
    /// The type of the sums of squares and products.
    type Sum: Copy + Debug + PartialEq;

    #[doc(hidden)]
    const ZERO: Self::Sum;

    #[doc(hidden)]
    fn accumulate(sums: &mut [Self::Sum; 3], left: Self::Value, right: Self::Value);

    #[doc(hidden)]
    fn energy(sums: &[Self::Sum; 3], weights: [i8; 3], divisor: u64) -> Self::Value;

    #[doc(hidden)]
    fn balance(sums: &[Self::Sum; 3]) -> Self::Value;

    #[doc(hidden)]
    fn correlation(sums: &[Self::Sum; 3]) -> Self::Value;

    #[doc(hidden)]
    fn to_f32(sum: Self::Sum) -> f32;
}

impl Correlate for Fixed {
    type Sum = i64;

    const ZERO: i64 = 0;

    #[inline]
    fn accumulate(sums: &mut [i64; 3], left: i32, right: i32) {
        let left = left >> 16;
        let right = right >> 16;

        sums[LEFT] += (left * left) as i64;
        sums[RIGHT] += (right * right) as i64;
        sums[PRODUCT] += (left * right) as i64;
    }

    fn energy(sums: &[i64; 3], weights: [i8; 3], divisor: u64) -> i32 {
        if divisor == 0 {
            return 0;
        }

        let total: i128 = sums
            .iter()
            .zip(weights)
            .map(|(sum, weight)| *sum as i128 * weight as i128)
            .sum();

        (total.max(0) * 2 / divisor as i128).min(i32::MAX as i128) as i32
    }

    fn balance(sums: &[i64; 3]) -> i32 {
        let left = sums[LEFT] as i128;
        let right = sums[RIGHT] as i128;

        if left + right == 0 {
            return 0;
        }

        saturate(((right - left) << 31) / (left + right))
    }

    fn correlation(sums: &[i64; 3]) -> i32 {
        if sums[LEFT] == 0 || sums[RIGHT] == 0 {
            return 0;
        }

        let norm = (sums[LEFT] as u128 * sums[RIGHT] as u128).isqrt() as i128;

        saturate(((sums[PRODUCT] as i128) << 31) / norm)
    }

    fn to_f32(sum: i64) -> f32 {
        sum as f32
    }
}

impl Correlate for Float {
    type Sum = f32;

    const ZERO: f32 = 0.0;

    #[inline]
    fn accumulate(sums: &mut [f32; 3], left: f32, right: f32) {
        sums[LEFT] += left * left;
        sums[RIGHT] += right * right;
        sums[PRODUCT] += left * right;
    }

    fn energy(sums: &[f32; 3], weights: [i8; 3], divisor: u64) -> f32 {
        if divisor == 0 {
            return 0.0;
        }

        let total: f32 = sums
            .iter()
            .zip(weights)
            .filter(|(_, weight)| *weight != 0)
            .map(|(sum, weight)| sum * weight as f32)
            .sum();

        if total < 0.0 {
            0.0
        } else {
            total / divisor as f32
        }
    }

    fn balance(sums: &[f32; 3]) -> f32 {
        let total = sums[LEFT] + sums[RIGHT];

        if total == 0.0 {
            return 0.0;
        }

        ((sums[RIGHT] - sums[LEFT]) / total).clamp(-1.0, 1.0)
    }

    fn correlation(sums: &[f32; 3]) -> f32 {
        let [left, right, product] = *sums;

        if left == 0.0 || right == 0.0 {
            return 0.0;
        }

        let largest = if left > right { left } else { right };
        let norm = (left / largest) * (right / largest);

        if norm < f32::MIN_POSITIVE {
            return 0.0;
        }

        (product / largest / sqrt(norm)).clamp(-1.0, 1.0)
    }

    fn to_f32(sum: f32) -> f32 {
        sum
    }
}

fn saturate(value: i128) -> i32 {
    value.clamp(-(i32::MAX as i128), i32::MAX as i128) as i32
}

fn sqrt(value: f32) -> f32 {
    let mut root = f32::from_bits((value.to_bits() >> 1) + 0x1fbd_1df5);

    for _ in 0..3 {
        root = 0.5 * (root + value / root);
    }

    root
}

fn log10(value: f32) -> f32 {
    if value.is_nan() || value < 0.0 {
        return f32::NAN;
    }

    if value == 0.0 {
        return f32::NEG_INFINITY;
    }

    if value == f32::INFINITY {
        return value;
    }

    let (value, offset) = if value < f32::MIN_POSITIVE {
        (value * 8_388_608.0, -23)
    } else {
        (value, 0)
    };
    let bits = value.to_bits();
    let mut exponent = (bits >> 23) as i32 - 127 + offset;
    let mut mantissa = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);

    if mantissa > SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }

    let ratio = (mantissa - 1.0) / (mantissa + 1.0);
    let square = ratio * ratio;
    let logarithm =
        2.0 * ratio * (1.0 + square * (1.0 / 3.0 + square * (1.0 / 5.0 + square * (1.0 / 7.0))));

    (exponent as f32 * LN_2 + logarithm) * LOG10_E
}

/// A stereo meter that measures the balance and the phase correlation of a pair of channels.
///
/// It sums the energy of each channel and their product over consecutive windows of frames, without an exponential time constant. Results cover the last complete window and change once per window, or cover the frames seen so far until the first window completes. They are computed when read, so the per-frame cost is three multiplies and three additions.
///
/// A channel missing from a frame takes the value of the last channel, so a mono stream reads as centered and fully correlated.
///
/// In [`Fixed`], every method except [`balance_db`](Self::balance_db) uses integers only, and the top 16 bits of each value are measured. In [`Float`], sums are `f32`, so windows of more than a few seconds lose precision, and a NaN or infinite value makes the results of its window NaN or infinite. The next window starts clean.
///
/// ```
/// use nanoaudio_analysis::{StereoMeter, analyze_using};
/// use nanoaudio_dsp::Float;
///
/// let mut meter = StereoMeter::<Float>::new(48_000);
///
/// analyze_using::<Float, _, _>(&[1000i16, -1000, 2000, -2000], 2, &mut meter).unwrap();
///
/// assert_eq!(meter.correlation(), -1.0);
/// assert_eq!(meter.balance(), 0.0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoMeter<R: Correlate = DefaultRepresentation> {
    left: u8,
    right: u8,
    window: u32,
    frames: u32,
    current: [R::Sum; 3],
    last: [R::Sum; 3],
    last_frames: u32,
}

impl<R: Correlate> StereoMeter<R> {
    /// Creates a meter on channels 0 and 1 with a window of 100 ms at `sample_rate` Hz.
    pub const fn new(sample_rate: u32) -> Self {
        Self::with_window(sample_rate, 100)
    }

    /// Creates a meter on channels 0 and 1 with a window of `milliseconds` at `sample_rate` Hz, at least one frame long.
    pub const fn with_window(sample_rate: u32, milliseconds: u32) -> Self {
        let frames = sample_rate as u64 * milliseconds as u64 / 1000;
        let window = if frames == 0 {
            1
        } else if frames > u32::MAX as u64 {
            u32::MAX
        } else {
            frames as u32
        };

        Self {
            left: 0,
            right: 1,
            window,
            frames: 0,
            current: [R::ZERO; 3],
            last: [R::ZERO; 3],
            last_frames: 0,
        }
    }

    /// Measures the channels `left` and `right` instead of 0 and 1.
    pub const fn with_channels(mut self, left: u8, right: u8) -> Self {
        self.left = left;
        self.right = right;
        self
    }

    /// Returns the length of the window, in frames.
    pub const fn window(&self) -> u32 {
        self.window
    }

    /// Returns the phase correlation, from -[`FULL_SCALE`](Measure::FULL_SCALE) out of phase to [`FULL_SCALE`](Measure::FULL_SCALE) in phase.
    ///
    /// It is the sum of the products of the two channels over the square root of the product of their energies. It is 0 for unrelated channels, and when either channel is silent.
    pub fn correlation(&self) -> R::Value {
        R::correlation(self.sums().0)
    }

    /// Returns the balance between the channels, from -[`FULL_SCALE`](Measure::FULL_SCALE) for the left channel only to [`FULL_SCALE`](Measure::FULL_SCALE) for the right channel only.
    ///
    /// It is the difference of the energies of the right and left channels over their sum, and 0 when both are silent.
    pub fn balance(&self) -> R::Value {
        R::balance(self.sums().0)
    }

    /// Returns the energy of the right channel relative to the left one, in dB.
    ///
    /// It is positive when the right channel is louder, 0 when both are silent, and infinite when only one is silent.
    pub fn balance_db(&self) -> f32 {
        let sums = self.sums().0;
        let left = R::to_f32(sums[LEFT]);
        let right = R::to_f32(sums[RIGHT]);

        if left == 0.0 && right == 0.0 {
            return 0.0;
        }

        10.0 * log10(right / left)
    }

    /// Returns the mean square of the left channel, [`FULL_SCALE`](Measure::FULL_SCALE) for a full-scale square wave.
    pub fn left_energy(&self) -> R::Value {
        self.energy([1, 0, 0], 1)
    }

    /// Returns the mean square of the right channel, [`FULL_SCALE`](Measure::FULL_SCALE) for a full-scale square wave.
    pub fn right_energy(&self) -> R::Value {
        self.energy([0, 1, 0], 1)
    }

    /// Returns the mean square of the mid signal, the average of the two channels.
    pub fn mid_energy(&self) -> R::Value {
        self.energy([1, 1, 2], 4)
    }

    /// Returns the mean square of the side signal, half the difference of the two channels.
    pub fn side_energy(&self) -> R::Value {
        self.energy([1, 1, -2], 4)
    }

    /// Clears the measurements and starts a new window.
    pub fn reset(&mut self) {
        self.frames = 0;
        self.current = [R::ZERO; 3];
        self.last = [R::ZERO; 3];
        self.last_frames = 0;
    }

    fn sums(&self) -> (&[R::Sum; 3], u32) {
        if self.last_frames > 0 {
            (&self.last, self.last_frames)
        } else {
            (&self.current, self.frames)
        }
    }

    fn energy(&self, weights: [i8; 3], scale: u64) -> R::Value {
        let (sums, frames) = self.sums();

        R::energy(sums, weights, frames as u64 * scale)
    }
}

impl<R: Correlate> Analyzer<R> for StereoMeter<R> {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        let (Some(left), Some(right)) = (
            frame.get(self.left as usize).or(frame.last()),
            frame.get(self.right as usize).or(frame.last()),
        ) else {
            return;
        };

        R::accumulate(&mut self.current, *left, *right);
        self.frames += 1;

        if self.frames >= self.window {
            self.last = self.current;
            self.last_frames = self.frames;
            self.current = [R::ZERO; 3];
            self.frames = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::f64::consts::PI;

    use nanoaudio_core::Format;
    use nanoaudio_dsp::{Stream, analyze_bytes_using, analyze_using, samples_using};

    use super::*;

    const RATE: u32 = 8000;
    const FRAMES: usize = if cfg!(miri) { 800 } else { 4000 };
    const MILLISECONDS: u32 = (FRAMES * 1000) as u32 / RATE;

    fn value<R: Correlate>(value: R::Value) -> f64 {
        R::to_sample::<f32>(value) as f64
    }

    fn noise(state: &mut u32) -> i16 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        (*state >> 16) as i16
    }

    fn sine(frame: usize) -> f64 {
        20_000.0 * (2.0 * PI * 440.0 * frame as f64 / RATE as f64).sin()
    }

    fn stereo(signal: impl Fn(usize) -> (i16, i16)) -> [i16; 2 * FRAMES] {
        let mut samples = [0i16; 2 * FRAMES];

        for frame in 0..FRAMES {
            let (left, right) = signal(frame);
            samples[2 * frame] = left;
            samples[2 * frame + 1] = right;
        }

        samples
    }

    fn signals() -> [[i16; 2 * FRAMES]; 7] {
        let mut first = 1u32;
        let mut second = 0x1234_5678u32;
        let mut noises = [(0i16, 0i16); FRAMES];

        for pair in noises.iter_mut() {
            *pair = (noise(&mut first), noise(&mut second));
        }

        let angle = PI / 6.0;

        [
            stereo(|frame| {
                let sample = sine(frame) as i16;
                (sample, sample)
            }),
            stereo(|frame| {
                let sample = sine(frame) as i16;
                (sample, -sample)
            }),
            stereo(|frame| noises[frame]),
            stereo(|frame| (sine(frame) as i16, 0)),
            stereo(|frame| (0, noises[frame].1)),
            stereo(|frame| {
                let sample = sine(frame);
                (
                    (sample * angle.cos()).round() as i16,
                    (sample * angle.sin()).round() as i16,
                )
            }),
            [0; 2 * FRAMES],
        ]
    }

    struct Expected {
        correlation: f64,
        balance: f64,
        balance_db: f64,
        left: f64,
        right: f64,
        mid: f64,
        side: f64,
    }

    fn expected(samples: &[i16]) -> Expected {
        let (mut left, mut right, mut product, mut mid, mut side) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let frames = samples.len() as f64 / 2.0;

        for frame in samples.as_chunks::<2>().0 {
            let l = frame[0] as f64 / 32_768.0;
            let r = frame[1] as f64 / 32_768.0;
            left += l * l;
            right += r * r;
            product += l * r;
            mid += (l + r) * (l + r) / 4.0;
            side += (l - r) * (l - r) / 4.0;
        }

        Expected {
            correlation: if left == 0.0 || right == 0.0 {
                0.0
            } else {
                product / (left * right).sqrt()
            },
            balance: if left + right == 0.0 {
                0.0
            } else {
                (right - left) / (right + left)
            },
            balance_db: if left + right == 0.0 {
                0.0
            } else {
                10.0 * (right / left).log10()
            },
            left: left / frames,
            right: right / frames,
            mid: mid / frames,
            side: side / frames,
        }
    }

    fn close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} is not {expected}"
        );
    }

    fn measures<R: Correlate>(tolerance: f64) {
        for samples in signals() {
            let mut tapped = StereoMeter::<R>::with_window(RATE, MILLISECONDS);
            let mut output = [0i16; 2 * FRAMES];

            let frames = samples_using::<R, _>(&samples, 2)
                .unwrap()
                .tap(&mut tapped)
                .write_samples(&mut output);

            assert_eq!(frames, FRAMES);
            assert_eq!(output, samples);
            assert_eq!(tapped.window(), FRAMES as u32);

            let mut sliced = StereoMeter::<R>::with_window(RATE, MILLISECONDS);
            let mut bytes = [0u8; 4 * FRAMES];

            for (sample, chunk) in samples.iter().zip(bytes.as_chunks_mut::<2>().0) {
                chunk.copy_from_slice(&sample.to_le_bytes());
            }

            assert_eq!(
                analyze_using::<R, _, _>(&samples, 2, &mut sliced),
                Ok(FRAMES)
            );
            assert_eq!(sliced, tapped);

            let mut bytewise = StereoMeter::<R>::with_window(RATE, MILLISECONDS);

            assert_eq!(
                analyze_bytes_using::<R, _>(&bytes, Format::I16, 2, &mut bytewise),
                Ok(FRAMES)
            );
            assert_eq!(bytewise, tapped);

            let expected = expected(&samples);

            close(
                value::<R>(tapped.correlation()),
                expected.correlation,
                tolerance,
            );
            close(value::<R>(tapped.balance()), expected.balance, tolerance);
            close(value::<R>(tapped.left_energy()), expected.left, tolerance);
            close(value::<R>(tapped.right_energy()), expected.right, tolerance);
            close(value::<R>(tapped.mid_energy()), expected.mid, tolerance);
            close(value::<R>(tapped.side_energy()), expected.side, tolerance);

            if expected.balance_db.is_finite() {
                close(tapped.balance_db() as f64, expected.balance_db, 1e-3);
            } else {
                assert_eq!(tapped.balance_db() as f64, expected.balance_db);
            }
        }
    }

    fn closed_forms<R: Correlate>() {
        let [
            in_phase,
            inverted,
            uncorrelated,
            left,
            right,
            panned,
            silence,
        ] = signals();
        let full = R::FULL_SCALE;
        let measure = |samples: &[i16]| {
            let mut meter = StereoMeter::<R>::with_window(RATE, MILLISECONDS);
            analyze_using::<R, _, _>(samples, 2, &mut meter).unwrap();
            meter
        };

        let meter = measure(&in_phase);
        assert!(meter.correlation() == full);
        assert!(meter.balance() == R::SILENCE);
        assert_eq!(meter.balance_db(), 0.0);
        assert!(meter.side_energy() == R::SILENCE);
        assert!(meter.mid_energy() == meter.left_energy());

        let meter = measure(&inverted);
        assert_eq!(value::<R>(meter.correlation()), -value::<R>(full));
        assert!(meter.mid_energy() == R::SILENCE);
        assert!(meter.side_energy() == meter.left_energy());

        let meter = measure(&uncorrelated);
        assert!(value::<R>(meter.correlation()).abs() < 0.05);
        assert!(value::<R>(meter.balance()).abs() < 0.05);

        let meter = measure(&left);
        assert!(meter.correlation() == R::SILENCE);
        assert_eq!(value::<R>(meter.balance()), -value::<R>(full));
        assert_eq!(meter.balance_db(), f32::NEG_INFINITY);

        let meter = measure(&right);
        assert!(meter.correlation() == R::SILENCE);
        assert!(meter.balance() == full);
        assert_eq!(meter.balance_db(), f32::INFINITY);

        let meter = measure(&panned);
        assert!(value::<R>(meter.correlation()) > 0.999_99);
        close(value::<R>(meter.balance()), -0.5, 1e-4);
        close(
            meter.balance_db() as f64,
            20.0 * (PI / 6.0).tan().log10(),
            1e-3,
        );

        let meter = measure(&silence);
        assert!(meter.correlation() == R::SILENCE);
        assert!(meter.balance() == R::SILENCE);
        assert_eq!(meter.balance_db(), 0.0);
        assert!(meter.left_energy() == R::SILENCE);
        assert!(meter.mid_energy() == R::SILENCE);
    }

    fn mono<R: Correlate>() {
        let mut meter = StereoMeter::<R>::new(RATE);
        analyze_using::<R, _, _>(&[1000i16, -3000, 2000], 1, &mut meter).unwrap();

        assert!(meter.correlation() == R::FULL_SCALE);
        assert!(meter.balance() == R::SILENCE);
        assert_eq!(meter.balance_db(), 0.0);
        assert!(meter.left_energy() == meter.right_energy());
        assert!(meter.left_energy() != R::SILENCE);
    }

    fn chooses_channels<R: Correlate>() {
        let mut meter = StereoMeter::<R>::new(RATE).with_channels(3, 1);
        let samples = [5000i16, 1000, 7000, -1000, -6000, 2000, 300, -2000];

        assert_eq!(analyze_using::<R, _, _>(&samples, 4, &mut meter), Ok(2));
        assert_eq!(value::<R>(meter.correlation()), -value::<R>(R::FULL_SCALE));

        let mut meter = StereoMeter::<R>::new(RATE).with_channels(7, 3);
        analyze_using::<R, _, _>(&samples, 4, &mut meter).unwrap();

        assert!(meter.correlation() == R::FULL_SCALE);
        assert!(meter.balance() == R::SILENCE);
    }

    fn windows<R: Correlate>() {
        let mut meter = StereoMeter::<R>::with_window(1000, 4);
        let frame = |left: i16, right: i16| [R::from_sample(left), R::from_sample(right)];

        assert_eq!(meter.window(), 4);
        assert!(meter.correlation() == R::SILENCE);

        for _ in 0..2 {
            meter.analyze(&frame(1000, 1000));
        }

        assert!(meter.correlation() == R::FULL_SCALE);

        for _ in 0..2 {
            meter.analyze(&frame(1000, 1000));
        }

        for _ in 0..4 {
            meter.analyze(&frame(1000, -1000));
        }

        for _ in 0..3 {
            meter.analyze(&frame(2000, 2000));
        }

        assert_eq!(value::<R>(meter.correlation()), -value::<R>(R::FULL_SCALE));
        close(
            value::<R>(meter.left_energy()),
            (1000.0f64 / 32768.0).powi(2),
            1e-9,
        );

        meter.analyze(&frame(2000, 2000));

        assert!(meter.correlation() == R::FULL_SCALE);
        close(
            value::<R>(meter.left_energy()),
            (2000.0f64 / 32768.0).powi(2),
            1e-9,
        );

        meter.analyze(&[]);
        meter.reset();

        assert_eq!(meter, StereoMeter::with_window(1000, 4));
        assert_eq!(StereoMeter::<R>::with_window(0, 100).window(), 1);
        assert_eq!(
            StereoMeter::<R>::with_window(u32::MAX, u32::MAX).window(),
            u32::MAX
        );
        assert_eq!(StereoMeter::<R>::new(48_000).window(), 4800);
    }

    #[test]
    fn fixed() {
        measures::<Fixed>(1e-6);
        closed_forms::<Fixed>();
        mono::<Fixed>();
        chooses_channels::<Fixed>();
        windows::<Fixed>();
    }

    #[test]
    fn float() {
        measures::<Float>(1e-5);
        closed_forms::<Float>();
        mono::<Float>();
        chooses_channels::<Float>();
        windows::<Float>();
    }

    #[test]
    fn fixed_extremes() {
        let mut meter = StereoMeter::<Fixed>::with_window(1000, 2);

        meter.analyze(&[i32::MIN, i32::MIN]);
        meter.analyze(&[i32::MIN, i32::MIN]);

        assert_eq!(meter.correlation(), i32::MAX);
        assert_eq!(meter.left_energy(), i32::MAX);
        assert_eq!(meter.mid_energy(), i32::MAX);
        assert_eq!(meter.side_energy(), 0);

        meter.analyze(&[i32::MIN, i32::MAX]);
        meter.analyze(&[i32::MIN, i32::MAX]);

        assert_eq!(meter.correlation(), -i32::MAX);
        assert_eq!(meter.side_energy(), (65_535i64 * 65_535 / 2) as i32);
        assert_eq!(meter.mid_energy(), 0);

        let mut meter = StereoMeter::<Fixed>::with_window(u32::MAX, u32::MAX);
        let largest = (1i64 << 30) * (u32::MAX as i64 - 1);

        meter.current = [largest, largest, -largest];
        meter.frames = u32::MAX - 1;
        meter.analyze(&[i32::MIN, i32::MAX]);

        assert_eq!(meter.last_frames, u32::MAX);
        assert_eq!(meter.frames, 0);
        assert_eq!(meter.correlation(), -i32::MAX);
        assert_eq!(meter.balance(), 0);
        assert_eq!(meter.left_energy(), i32::MAX);
        assert_eq!(meter.side_energy(), i32::MAX);
        assert_eq!(meter.mid_energy(), 0);
        assert_eq!(meter.balance_db(), 0.0);

        meter.last = [largest, 0, 0];

        assert_eq!(meter.balance(), -i32::MAX);
        assert_eq!(meter.right_energy(), 0);
    }

    #[test]
    fn float_non_finite() {
        let mut meter = StereoMeter::<Float>::with_window(1000, 2);

        meter.analyze(&[f32::NAN, 0.5]);
        meter.analyze(&[0.5, 0.5]);

        assert!(meter.correlation().is_nan());
        assert!(meter.balance().is_nan());
        assert!(meter.balance_db().is_nan());

        meter.analyze(&[f32::INFINITY, 0.5]);
        meter.analyze(&[0.5, 0.5]);

        assert!(meter.correlation().is_nan());
        assert!(meter.balance().is_nan());
        assert_eq!(meter.left_energy(), f32::INFINITY);

        meter.analyze(&[0.5, 0.5]);
        meter.analyze(&[0.25, 0.25]);

        assert_eq!(meter.correlation(), 1.0);
        assert_eq!(meter.balance(), 0.0);

        meter.analyze(&[2.0, -2.0]);
        meter.analyze(&[3.0, -3.0]);

        assert_eq!(meter.correlation(), -1.0);
        assert_eq!(meter.left_energy(), 6.5);
    }

    #[test]
    fn logarithms_and_roots() {
        let mut value = f32::from_bits(7);

        while value < 1e38 {
            close(log10(value) as f64, (value as f64).log10(), 2e-5);
            value *= 1.37;
        }

        let mut value = f32::MIN_POSITIVE;

        while value < 2.0 {
            let root = sqrt(value) as f64;
            close(root / (value as f64).sqrt(), 1.0, 2e-7);
            value *= 1.21;
        }

        assert_eq!(log10(0.0), f32::NEG_INFINITY);
        assert_eq!(log10(f32::INFINITY), f32::INFINITY);
        assert_eq!(log10(1.0), 0.0);
        assert!(log10(-1.0).is_nan());
        assert!(log10(f32::NAN).is_nan());
    }
}
