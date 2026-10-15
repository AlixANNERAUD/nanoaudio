use core::fmt::Debug;
use core::time::Duration;

use nanoaudio_core::MAXIMUM_CHANNELS;
use nanoaudio_dsp::{Analyzer, Fixed, Float};

use crate::Measure;

mod sealed {
    pub trait Sealed {}

    impl Sealed for nanoaudio_dsp::Fixed {}
    impl Sealed for nanoaudio_dsp::Float {}

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct FloatSum {
        pub(super) block: f32,
        pub(super) total: f32,
        pub(super) compensation: f32,
    }
}

use sealed::FloatSum;

const DECIBELS_PER_OCTAVE: f32 = 3.010_3;

/// A representation whose values a [`LevelMeter`] can square and sum, [`Fixed`] or [`Float`].
///
/// In [`Fixed`], values are rounded to 16 bits before squaring, so RMS levels below about -96 dBFS read as silence, and squares are summed in a `u64`, which cannot overflow for any window up to `u32::MAX` frames. In [`Float`], they are summed in `f32` blocks of 256 frames, then with compensated summation.
pub trait Power: Measure<Value: Debug + PartialEq> + Debug + PartialEq + sealed::Sealed {
    #[doc(hidden)]
    type Sum: Copy + Debug + PartialEq;
    #[doc(hidden)]
    const ZERO: Self::Sum;
    #[doc(hidden)]
    const BLOCK: u32;

    #[doc(hidden)]
    fn accumulate(sum: &mut Self::Sum, value: Self::Value);
    #[doc(hidden)]
    fn flush(sum: &mut Self::Sum);
    #[doc(hidden)]
    fn mean_square(sum: Self::Sum, frames: u32) -> Self::Value;
    #[doc(hidden)]
    fn root_mean_square(sum: Self::Sum, frames: u32) -> Self::Value;
    #[doc(hidden)]
    fn power_decibels(sum: Self::Sum, frames: u32) -> f32;
    #[doc(hidden)]
    fn amplitude_decibels(value: Self::Value) -> f32;
}

fn fixed_power(sum: u64, frames: u32) -> u64 {
    if frames == 0 {
        return 0;
    }

    let frames = frames as u64;

    ((sum / frames) << 32) + ((sum % frames) << 32) / frames
}

impl Power for Fixed {
    type Sum = u64;
    const ZERO: u64 = 0;
    const BLOCK: u32 = u32::MAX;

    #[inline(always)]
    fn accumulate(sum: &mut u64, value: i32) {
        let rounded = ((value >> 15) + 1) >> 1;
        *sum += (rounded * rounded) as u64;
    }

    #[inline(always)]
    fn flush(_: &mut u64) {}

    fn mean_square(sum: u64, frames: u32) -> i32 {
        ((fixed_power(sum, frames) + (1 << 30)) >> 31).min(i32::MAX as u64) as i32
    }

    fn root_mean_square(sum: u64, frames: u32) -> i32 {
        fixed_power(sum, frames).isqrt().min(i32::MAX as u64) as i32
    }

    fn power_decibels(sum: u64, frames: u32) -> f32 {
        match fixed_power(sum, frames) {
            0 => f32::NEG_INFINITY,
            power => DECIBELS_PER_OCTAVE * (log2(power as f32) - 62.0),
        }
    }

    fn amplitude_decibels(value: i32) -> f32 {
        match value.saturating_abs() {
            0 => f32::NEG_INFINITY,
            magnitude => 2.0 * DECIBELS_PER_OCTAVE * (log2(magnitude as f32) - 31.0),
        }
    }
}

impl FloatSum {
    fn value(self) -> f32 {
        let total = self.total + self.block;

        if total.is_finite() {
            total - self.compensation
        } else {
            total
        }
    }
}

impl Power for Float {
    type Sum = FloatSum;
    const ZERO: FloatSum = FloatSum {
        block: 0.0,
        total: 0.0,
        compensation: 0.0,
    };
    const BLOCK: u32 = 255;

    #[inline(always)]
    fn accumulate(sum: &mut FloatSum, value: f32) {
        sum.block += value * value;
    }

    #[inline]
    fn flush(sum: &mut FloatSum) {
        let block = sum.block - sum.compensation;
        let total = sum.total + block;

        sum.compensation = if total.is_finite() {
            (total - sum.total) - block
        } else {
            0.0
        };
        sum.total = total;
        sum.block = 0.0;
    }

    fn mean_square(sum: FloatSum, frames: u32) -> f32 {
        if frames == 0 {
            return 0.0;
        }

        sum.value() / frames as f32
    }

    fn root_mean_square(sum: FloatSum, frames: u32) -> f32 {
        sqrt(Self::mean_square(sum, frames))
    }

    fn power_decibels(sum: FloatSum, frames: u32) -> f32 {
        DECIBELS_PER_OCTAVE * log2(Self::mean_square(sum, frames))
    }

    fn amplitude_decibels(value: f32) -> f32 {
        2.0 * DECIBELS_PER_OCTAVE * log2(value.abs())
    }
}

fn log2(value: f32) -> f32 {
    if value.is_nan() || value < 0.0 {
        return f32::NAN;
    }

    if value == 0.0 {
        return f32::NEG_INFINITY;
    }

    if value.is_infinite() {
        return value;
    }

    let (value, offset) = if value < f32::MIN_POSITIVE {
        (value * (1u32 << 31) as f32, -31.0)
    } else {
        (value, 0.0)
    };
    let bits = value.to_bits();
    let mut exponent = ((bits >> 23) & 0xff) as i32 - 127;
    let mut mantissa = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);

    if mantissa > core::f32::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }

    let ratio = (mantissa - 1.0) / (mantissa + 1.0);
    let square = ratio * ratio;
    let logarithm = 2.0
        * ratio
        * (1.0 + square * (1.0 / 3.0 + square * (1.0 / 5.0 + square * (1.0 / 7.0 + square / 9.0))));

    exponent as f32 + offset + logarithm * core::f32::consts::LOG2_E
}

fn sqrt(value: f32) -> f32 {
    if value.is_nan() || value <= 0.0 || value.is_infinite() {
        return value;
    }

    if value < f32::MIN_POSITIVE {
        return sqrt(value * (1u64 << 48) as f32) / (1u32 << 24) as f32;
    }

    let mut root = f32::from_bits((value.to_bits() >> 1) + 0x1fbd_1df5);

    for _ in 0..3 {
        root = 0.5 * (root + value / root);
    }

    root
}

/// The peak and mean square of one channel over one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level<R: Power> {
    peak: R::Value,
    sum: R::Sum,
    frames: u32,
}

impl<R: Power> Level<R> {
    /// The level of a channel before its first window, silence over 0 frames.
    pub const SILENCE: Self = Self {
        peak: R::SILENCE,
        sum: R::ZERO,
        frames: 0,
    };

    /// Returns the number of frames measured.
    pub const fn frames(&self) -> u32 {
        self.frames
    }

    /// Returns the largest magnitude, between silence and [`FULL_SCALE`](Measure::FULL_SCALE) in fixed point.
    pub const fn peak(&self) -> R::Value {
        self.peak
    }

    /// Returns the mean of the squared values, relative to full scale: a full-scale square wave gives full scale.
    ///
    /// It is Q31 in [`Fixed`], so levels below about -93 dBFS read as 0 there. [`rms`](Self::rms) and [`rms_decibels`](Self::rms_decibels) keep more precision.
    pub fn mean_square(&self) -> R::Value {
        R::mean_square(self.sum, self.frames)
    }

    /// Returns the root mean square, the square root of [`mean_square`](Self::mean_square).
    ///
    /// A full-scale sine gives about 0.707 of full scale.
    pub fn rms(&self) -> R::Value {
        R::root_mean_square(self.sum, self.frames)
    }

    /// Returns the peak in dBFS, 0 at full scale and negative infinity for silence.
    pub fn peak_decibels(&self) -> f32 {
        R::amplitude_decibels(self.peak)
    }

    /// Returns the root mean square in dBFS, 0 for a full-scale square wave, about -3.01 for a full-scale sine and negative infinity for silence.
    ///
    /// A NaN sample in [`Float`] makes it NaN for its window.
    pub fn rms_decibels(&self) -> f32 {
        R::power_decibels(self.sum, self.frames)
    }
}

impl<R: Power> Default for Level<R> {
    fn default() -> Self {
        Self::SILENCE
    }
}

/// A meter that measures the peak and RMS level of each channel over consecutive windows.
///
/// Each window covers a fixed number of frames, and its [`Level`]s are read once it is complete. The meter also holds the largest peak since the last reset. It keeps up to `CHANNELS` channels, and ignores the others.
///
/// Samples in [`Fixed`] are processed without floating point: peaks keep the full precision, and squares are taken at 16-bit precision. Only the decibel methods use floating point.
///
/// ```
/// use core::time::Duration;
///
/// use nanoaudio_analysis::{LevelMeter, analyze_using};
/// use nanoaudio_dsp::Fixed;
///
/// let mut meter = LevelMeter::<Fixed>::with_duration(Duration::from_millis(1), 48_000);
/// let square: [i16; 96] = core::array::from_fn(|index| if index % 4 < 2 { 16384 } else { -16384 });
///
/// analyze_using::<Fixed, _, _>(&square, 2, &mut meter).unwrap();
///
/// let level = meter.level(0);
///
/// assert_eq!(meter.window(), 48);
/// assert_eq!(level.peak(), 1 << 30);
/// assert_eq!(level.rms(), 1 << 30);
/// assert!((level.rms_decibels() + 6.02).abs() < 0.01);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct LevelMeter<R: Power, const CHANNELS: usize = MAXIMUM_CHANNELS> {
    window: u32,
    remaining: u32,
    channels: u8,
    windows: u64,
    peaks: [R::Value; CHANNELS],
    sums: [R::Sum; CHANNELS],
    levels: [Level<R>; CHANNELS],
    held: [R::Value; CHANNELS],
}

impl<R: Power, const CHANNELS: usize> LevelMeter<R, CHANNELS> {
    /// Creates a meter with windows of `window` frames, at least 1.
    pub const fn new(window: u32) -> Self {
        let window = if window == 0 { 1 } else { window };

        Self {
            window,
            remaining: window,
            channels: 0,
            windows: 0,
            peaks: [R::SILENCE; CHANNELS],
            sums: [R::ZERO; CHANNELS],
            levels: [Level::SILENCE; CHANNELS],
            held: [R::SILENCE; CHANNELS],
        }
    }

    /// Creates a meter with windows of `duration` at `sample_rate` Hz, rounded to the nearest frame and at least 1.
    pub const fn with_duration(duration: Duration, sample_rate: u32) -> Self {
        let frames = (duration.as_nanos() * sample_rate as u128 + 500_000_000) / 1_000_000_000;

        Self::new(if frames > u32::MAX as u128 {
            u32::MAX
        } else {
            frames as u32
        })
    }

    /// Returns the number of frames in a window.
    pub const fn window(&self) -> u32 {
        self.window
    }

    /// Returns the number of channels of the last frame, up to `CHANNELS`, or 0 before the first one.
    pub const fn channels(&self) -> u8 {
        self.channels
    }

    /// Returns the number of windows completed since the last reset.
    pub const fn windows(&self) -> u64 {
        self.windows
    }

    /// Returns the level of `channel` over the last complete window, or [`Level::SILENCE`] before the first one or for a channel out of range.
    pub fn level(&self, channel: usize) -> Level<R> {
        match self.levels.get(channel) {
            Some(level) => *level,
            None => Level::SILENCE,
        }
    }

    /// Returns the largest magnitude of `channel` since the last reset, including the window in progress.
    pub fn peak_hold(&self, channel: usize) -> R::Value {
        match (self.held.get(channel), self.peaks.get(channel)) {
            (Some(held), Some(peak)) if *peak > *held => *peak,
            (Some(held), _) => *held,
            _ => R::SILENCE,
        }
    }

    /// Returns [`peak_hold`](Self::peak_hold) in dBFS, negative infinity for silence.
    pub fn peak_hold_decibels(&self, channel: usize) -> f32 {
        R::amplitude_decibels(self.peak_hold(channel))
    }

    /// Measures one interleaved frame, and returns `true` when it completes a window.
    ///
    /// It is what [`Analyzer::analyze`] does, for analyzers built on top of this meter that act on each window.
    #[inline]
    pub fn measure(&mut self, frame: &[R::Value]) -> bool {
        self.channels = frame.len().min(CHANNELS) as u8;

        for ((value, peak), sum) in frame
            .iter()
            .zip(self.peaks.iter_mut())
            .zip(self.sums.iter_mut())
        {
            let magnitude = R::magnitude(*value);

            if magnitude > *peak {
                *peak = magnitude;
            }

            R::accumulate(sum, *value);
        }

        self.remaining -= 1;

        if self.remaining & R::BLOCK != 0 {
            return false;
        }

        self.end_block()
    }

    #[cold]
    fn end_block(&mut self) -> bool {
        for sum in self.sums.iter_mut() {
            R::flush(sum);
        }

        if self.remaining != 0 {
            return false;
        }

        self.complete();
        true
    }

    fn complete(&mut self) {
        for (((peak, sum), level), held) in self
            .peaks
            .iter_mut()
            .zip(self.sums.iter_mut())
            .zip(self.levels.iter_mut())
            .zip(self.held.iter_mut())
        {
            *level = Level {
                peak: *peak,
                sum: *sum,
                frames: self.window,
            };

            if *peak > *held {
                *held = *peak;
            }

            *peak = R::SILENCE;
            *sum = R::ZERO;
        }

        self.remaining = self.window;
        self.windows += 1;
    }

    /// Clears every level, the peak hold and the window in progress, and keeps the window length.
    pub fn reset(&mut self) {
        *self = Self::new(self.window);
    }

    /// Clears the peak hold only, so that it starts again from the window in progress.
    pub fn reset_peak_hold(&mut self) {
        self.held = [R::SILENCE; CHANNELS];
    }
}

impl<R: Power, const CHANNELS: usize> Analyzer<R> for LevelMeter<R, CHANNELS> {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        self.measure(frame);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use nanoaudio_core::{Format, I24, Sample};
    use nanoaudio_dsp::{
        Representation, Stream, analyze_bytes_using, analyze_using, samples_using,
    };

    use super::*;

    const SCALE: f64 = 2_147_483_648.0;

    fn linear<R: Power>(value: R::Value) -> f64 {
        R::to_sample::<i32>(value) as f64 / SCALE
    }

    fn decibels(power: f64) -> f64 {
        if power == 0.0 {
            f64::NEG_INFINITY
        } else {
            10.0 * power.log10()
        }
    }

    fn close(actual: f32, expected: f64) {
        if expected.is_infinite() {
            assert_eq!(actual as f64, expected);
        } else {
            assert!((actual as f64 - expected).abs() < 1e-3);
        }
    }

    struct Reference {
        peak: f64,
        power: f64,
    }

    fn reference(samples: &[i16], channels: usize, window: usize) -> Vec<Vec<Reference>> {
        samples
            .chunks_exact(channels * window)
            .map(|block| {
                (0..channels)
                    .map(|channel| {
                        let values = block
                            .iter()
                            .skip(channel)
                            .step_by(channels)
                            .map(|sample| *sample as f64 / 32_768.0);
                        Reference {
                            peak: values
                                .clone()
                                .fold(0.0, |peak, value| value.abs().max(peak)),
                            power: values.map(|value| value * value).sum::<f64>() / window as f64,
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn signal(channels: usize, frames: usize) -> Vec<i16> {
        (0..frames * channels)
            .map(|index| {
                let (frame, channel) = (index / channels, index % channels);
                let phase = frame as f64 * (channel as f64 + 1.0) * 0.0123;
                let amplitude = [0.9, 0.25, 0.001, 1.0, 0.5, 0.03, 0.7, 0.1][channel];
                (amplitude * 32_767.0 * phase.sin()).round() as i16
            })
            .collect()
    }

    fn check<R: Power>(meter: &LevelMeter<R>, expected: &[Reference], tolerance: f64) {
        for (channel, expected) in expected.iter().enumerate() {
            let level = meter.level(channel);
            let rms = expected.power.sqrt();
            assert!((linear::<R>(level.peak()) - expected.peak).abs() < 1e-6);
            assert!((linear::<R>(level.mean_square()) - expected.power).abs() < 1e-6);
            assert!((linear::<R>(level.rms()) - rms).abs() < 1e-6 + rms * tolerance);
            close(level.rms_decibels(), decibels(expected.power));
            close(level.peak_decibels(), 2.0 * decibels(expected.peak));
        }
    }

    fn matches_reference<R: Power>() {
        let (channels, windows): (&[usize], &[usize]) = if cfg!(miri) {
            (&[1, 2, MAXIMUM_CHANNELS], &[1, 7, 40])
        } else {
            (&[1, 2, 3, 4, 5, 6, 7, 8], &[1, 7, 480, 1000])
        };

        for &channels in channels {
            for &window in windows {
                let tail = window / 2;
                let input = signal(channels, 4 * window + tail);
                let expected = reference(&input, channels, window);
                let mut meter = LevelMeter::<R>::new(window as u32);
                let mut output = std::vec![0i16; input.len()];

                let frames = samples_using::<R, _>(&input, channels as u8)
                    .unwrap()
                    .tap(&mut meter)
                    .write_samples(&mut output);

                assert_eq!(frames, 4 * window + tail);
                assert_eq!(output, input);
                assert_eq!(meter.windows(), 4);
                assert_eq!(meter.channels() as usize, channels);
                assert_eq!(meter.level(0).frames(), window as u32);
                check(&meter, &expected[3], 1e-6);

                let held = (0..4).flat_map(|index| {
                    input[index * window * channels..]
                        .iter()
                        .take(window * channels)
                });
                let rest = &input[4 * window * channels..];
                for channel in 0..channels {
                    let largest = held
                        .clone()
                        .chain(rest.iter())
                        .skip(channel)
                        .step_by(channels)
                        .map(|sample| (*sample as f64 / 32_768.0).abs())
                        .fold(0.0, f64::max);
                    assert!((linear::<R>(meter.peak_hold(channel)) - largest).abs() < 1e-6);
                }

                let mut sliced = LevelMeter::<R>::new(window as u32);
                let bytes: Vec<u8> = input
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect();
                assert_eq!(
                    analyze_bytes_using::<R, _>(&bytes, Format::I16, channels as u8, &mut sliced),
                    Ok(frames)
                );
                assert_eq!(sliced, meter);

                let mut first = LevelMeter::<R>::new(window as u32);
                analyze_using::<R, _, _>(&input[..window * channels], channels as u8, &mut first)
                    .unwrap();
                check(&first, &expected[0], 1e-6);
            }
        }
    }

    #[test]
    fn fixed_matches_reference() {
        matches_reference::<Fixed>();
    }

    #[test]
    fn float_matches_reference() {
        matches_reference::<Float>();
    }

    fn closed_forms<R: Power>() {
        let mut meter = LevelMeter::<R, 2>::new(64);
        let frames: Vec<i16> = (0..64)
            .flat_map(|frame| [if frame % 2 == 0 { i16::MIN } else { i16::MAX }, 0])
            .collect();
        analyze_using::<R, _, _>(&frames, 2, &mut meter).unwrap();

        let level = meter.level(0);
        assert!(level.rms_decibels().abs() < 2e-4);
        assert!(level.peak_decibels().abs() < 1e-6);
        assert!(linear::<R>(level.rms()) > 0.9999);
        assert_eq!(meter.level(1).rms_decibels(), f32::NEG_INFINITY);
        assert_eq!(meter.level(1).peak_decibels(), f32::NEG_INFINITY);
        assert_eq!(meter.level(1).rms(), R::SILENCE);
        assert_eq!(meter.level(1).mean_square(), R::SILENCE);
        assert_eq!(meter.peak_hold_decibels(1), f32::NEG_INFINITY);

        let mut meter = LevelMeter::<R>::new(400);
        let sine: Vec<i32> = (0..400)
            .map(|frame| {
                (0.5 * SCALE * (core::f64::consts::TAU * frame as f64 / 40.0).sin()).round() as i32
            })
            .collect();
        analyze_using::<R, _, _>(&sine, 1, &mut meter).unwrap();
        let level = meter.level(0);
        assert!((level.rms_decibels() as f64 - decibels(0.125)).abs() < 1e-3);
        assert!((linear::<R>(level.rms()) - 0.125f64.sqrt()).abs() < 1e-5);
        assert!((level.peak_decibels() as f64 + 6.0206).abs() < 1e-3);

        let mut meter = LevelMeter::<R>::new(10);
        analyze_using::<R, _, _>(&[I24::from_i32(-1 << 30); 10], 1, &mut meter).unwrap();
        assert!((meter.level(0).rms_decibels() as f64 + 6.0206).abs() < 1e-3);
        assert!((linear::<R>(meter.level(0).mean_square()) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn fixed_closed_forms() {
        closed_forms::<Fixed>();
    }

    #[test]
    fn float_closed_forms() {
        closed_forms::<Float>();
    }

    fn lifecycle<R: Power>() {
        let mut meter = LevelMeter::<R, 1>::new(3);

        assert_eq!(analyze_using::<R, i16, _>(&[], 2, &mut meter), Ok(0));
        assert_eq!(meter.windows(), 0);
        assert_eq!(meter.level(0), Level::SILENCE);
        assert_eq!(meter.level(5), Level::default());
        assert_eq!(meter.peak_hold(5), R::SILENCE);

        let loud = R::from_sample(i16::MAX / 2);
        let quiet = R::from_sample(-100i16);
        assert!(!meter.measure(&[loud, quiet]));
        assert!(!meter.measure(&[quiet, loud]));
        assert_eq!(meter.channels(), 1);
        assert_eq!(meter.peak_hold(0), loud);
        assert!(meter.measure(&[quiet, loud]));
        assert_eq!(meter.windows(), 1);
        assert_eq!(meter.level(0).peak(), loud);
        assert_eq!(meter.level(1), Level::SILENCE);

        for _ in 0..3 {
            meter.measure(&[quiet]);
        }
        assert_eq!(meter.windows(), 2);
        assert_eq!(meter.level(0).peak(), R::magnitude(quiet));
        assert_eq!(meter.peak_hold(0), loud);

        meter.reset_peak_hold();
        assert_eq!(meter.peak_hold(0), R::SILENCE);
        meter.measure(&[quiet]);
        assert_eq!(meter.peak_hold(0), R::magnitude(quiet));

        meter.reset();
        assert_eq!(meter, LevelMeter::new(3));
    }

    #[test]
    fn fixed_lifecycle() {
        lifecycle::<Fixed>();
    }

    #[test]
    fn float_lifecycle() {
        lifecycle::<Float>();
    }

    #[test]
    fn windows_from_durations() {
        let window = |duration, rate| LevelMeter::<Fixed>::with_duration(duration, rate).window();

        assert_eq!(window(Duration::from_millis(300), 48_000), 14_400);
        assert_eq!(window(Duration::from_millis(1), 44_100), 44);
        assert_eq!(window(Duration::from_micros(1_010), 44_100), 45);
        assert_eq!(window(Duration::ZERO, 48_000), 1);
        assert_eq!(window(Duration::from_secs(1 << 40), 192_000), u32::MAX);
        assert_eq!(LevelMeter::<Float>::new(0).window(), 1);
    }

    #[test]
    fn fixed_extremes() {
        let mut meter = LevelMeter::<Fixed>::new(2);
        analyze_using::<Fixed, _, _>(&[i32::MIN, i32::MIN, i32::MIN, i32::MAX], 2, &mut meter)
            .unwrap();

        for channel in 0..2 {
            let level = meter.level(channel);
            assert_eq!(level.peak(), i32::MAX);
            assert_eq!(level.mean_square(), i32::MAX);
            assert_eq!(level.rms(), i32::MAX);
            assert_eq!(level.rms_decibels(), 0.0);
            assert!(level.peak_decibels().abs() < 1e-6);
        }

        assert_eq!(fixed_power(u64::from(u32::MAX) << 30, u32::MAX), 1 << 62);
        assert_eq!(fixed_power(0, 0), 0);
        assert_eq!(Level::<Fixed>::SILENCE.rms(), 0);

        let smallest = Level::<Fixed> {
            peak: 1,
            sum: 1,
            frames: 1,
        };
        assert!((smallest.peak_decibels() as f64 - 20.0 * (1.0 / SCALE).log10()).abs() < 1e-3);
        assert!((smallest.rms_decibels() as f64 - 10.0 * (2f64.powi(-30)).log10()).abs() < 1e-3);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn long_runs() {
        let frames = 3 << 20;
        let mut fixed = LevelMeter::<Fixed>::new(frames);
        let mut float = LevelMeter::<Float>::new(frames);
        let value = 0.1f32;

        for _ in 0..frames {
            fixed.measure(&[i32::MIN, Fixed::from_sample(value)]);
            float.measure(&[value, -1.0]);
        }

        assert_eq!(fixed.level(0).rms(), i32::MAX);
        assert_eq!(fixed.level(0).rms_decibels(), 0.0);
        assert!((fixed.level(1).rms_decibels() + 20.0).abs() < 1e-3);
        assert!(
            ((float.level(0).mean_square() as f64) / (value as f64 * value as f64) - 1.0).abs()
                < 2e-5
        );
        assert_eq!(float.level(1).mean_square(), 1.0);
    }

    #[test]
    fn float_special_values() {
        let mut meter = LevelMeter::<Float, 2>::new(5000);

        meter.measure(&[f32::NAN, f32::INFINITY]);
        for _ in 0..4999 {
            meter.measure(&[0.5, 0.5]);
        }

        assert_eq!(meter.level(0).peak(), 0.5);
        assert!(meter.level(0).mean_square().is_nan());
        assert!(meter.level(0).rms_decibels().is_nan());
        assert_eq!(meter.level(1).peak(), f32::INFINITY);
        assert_eq!(meter.level(1).mean_square(), f32::INFINITY);
        assert_eq!(meter.level(1).rms_decibels(), f32::INFINITY);
        assert_eq!(meter.level(1).peak_decibels(), f32::INFINITY);

        for _ in 0..5000 {
            meter.measure(&[0.5, -0.5]);
        }

        for channel in 0..2 {
            assert_eq!(meter.level(channel).mean_square(), 0.25);
            assert_eq!(meter.level(channel).rms(), 0.5);
        }

        let level = |value: f32| Level::<Float> {
            peak: value,
            sum: FloatSum {
                block: 0.0,
                total: value,
                compensation: 0.0,
            },
            frames: 1,
        };
        let tiny = level(1e-40);
        assert!((tiny.rms_decibels() as f64 + 400.0).abs() < 1e-2);
        assert!((linear::<Float>(level(0.0).rms())).abs() < 1e-12);
        assert!(((tiny.rms() as f64) / 1e-20 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn helpers() {
        for index in 1..2000 {
            let value = 1.000_37f64.powi(index * 37) * 1e-30;
            let approximation = log2(value as f32) as f64;
            let exact = (value as f32 as f64).log2();
            assert!((approximation - exact).abs() < 1e-6 + exact.abs() * 6e-8);
            let root = sqrt(value as f32) as f64;
            assert!((root / (value as f32 as f64).sqrt() - 1.0).abs() < 2e-7);
        }

        assert!(log2(-1.0).is_nan());
        assert!(log2(f32::NAN).is_nan());
        assert_eq!(log2(0.0), f32::NEG_INFINITY);
        assert_eq!(log2(f32::INFINITY), f32::INFINITY);
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(f32::INFINITY), f32::INFINITY);
        assert!(sqrt(f32::NAN).is_nan());
    }
}
