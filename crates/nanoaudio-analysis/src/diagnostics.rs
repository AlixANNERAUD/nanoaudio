use core::marker::PhantomData;

use nanoaudio_core::MAXIMUM_CHANNELS;
use nanoaudio_dsp::{Analyzer, Representation};

use crate::{Measure, math};

#[inline]
fn shift_right(sum: i64, shift: u32) -> i32 {
    (((sum >> 32) as i32) << (32 - shift)) | ((sum as u32) >> shift) as i32
}

fn frames(sample_rate: u32, milliseconds: u32) -> u64 {
    (sample_rate as u64 * milliseconds as u64 / 1000).max(1)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
struct Clips {
    samples: u64,
    runs: u64,
    run: u32,
    longest: u32,
}

/// Counts clipped samples on each channel, and runs of consecutive clipped samples.
///
/// A sample is clipped when its magnitude is at or above a threshold, by default the largest 16-bit sample, so that 16-bit audio stuck at either rail counts. A single full-scale sample is common in loud audio, so a run of several clipped samples in a row, 3 by default, is a better sign of real clipping.
///
/// In [`Float`](nanoaudio_dsp::Float), infinities are clipped and NaN is not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipCounter<R: Measure> {
    threshold: R::Value,
    run_length: u32,
    channels: u8,
    clips: [Clips; MAXIMUM_CHANNELS],
}

impl<R: Measure> ClipCounter<R> {
    /// Creates a counter with the default threshold and run length.
    pub fn new() -> Self {
        Self::with_threshold(R::from_sample(i16::MAX), 3)
    }

    /// Creates a counter that clips at `threshold`, and counts a run once it reaches `run_length` consecutive clipped samples.
    ///
    /// A `run_length` of 0 counts as 1.
    pub fn with_threshold(threshold: R::Value, run_length: u32) -> Self {
        Self {
            threshold,
            run_length: run_length.max(1),
            channels: 0,
            clips: [Clips::default(); MAXIMUM_CHANNELS],
        }
    }

    /// Returns the magnitude at and above which a sample is clipped.
    pub const fn threshold(&self) -> R::Value {
        self.threshold
    }

    /// Returns the number of consecutive clipped samples that make a run.
    pub const fn run_length(&self) -> u32 {
        self.run_length
    }

    /// Returns the number of channels of the last frame, or 0 before the first one.
    pub const fn channels(&self) -> u8 {
        self.channels
    }

    /// Returns the number of clipped samples on `channel`, or 0 if there is no such channel.
    pub fn samples(&self, channel: usize) -> u64 {
        self.clips.get(channel).map_or(0, |clips| clips.samples)
    }

    /// Returns the number of runs of [`run_length`](Self::run_length) or more clipped samples on `channel`.
    pub fn runs(&self, channel: usize) -> u64 {
        self.clips.get(channel).map_or(0, |clips| clips.runs)
    }

    /// Returns the length of the longest run of clipped samples on `channel`, saturating at `u32::MAX`.
    pub fn longest_run(&self, channel: usize) -> u32 {
        self.clips.get(channel).map_or(0, |clips| clips.longest)
    }

    /// Clears the counts and keeps the settings.
    pub fn reset(&mut self) {
        *self = Self::with_threshold(self.threshold, self.run_length);
    }
}

impl<R: Measure> Default for ClipCounter<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Measure> Analyzer<R> for ClipCounter<R> {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        self.channels = frame.len() as u8;

        for (value, clips) in frame.iter().zip(self.clips.iter_mut()) {
            if R::magnitude(*value) >= self.threshold {
                clips.samples += 1;
                clips.run = clips.run.saturating_add(1);
                clips.longest = clips.longest.max(clips.run);

                if clips.run == self.run_length {
                    clips.runs += 1;
                }
            } else {
                clips.run = 0;
            }
        }
    }
}

/// Measures the DC offset of each channel, the slow average its samples drift around.
///
/// It is a one-pole low-pass filter, an exponential moving average, the part a one-pole DC-blocking high-pass would remove. Its time constant is rounded to a power of two frames, so the filter needs no multiplication, and it settles to 63% of a step in DC after one time constant. It starts at 0.
///
/// It runs in fixed point for both representations and settles exactly on a constant input. In [`Float`](nanoaudio_dsp::Float), values beyond full scale count as full scale and NaN counts as 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DcOffset<R: Representation> {
    shift: u32,
    channels: u8,
    sums: [i64; MAXIMUM_CHANNELS],
    representation: PhantomData<R>,
}

impl<R: Representation> DcOffset<R> {
    /// Creates a meter with a time constant of about one second, at `sample_rate` in Hz.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_time_constant(sample_rate, 1000)
    }

    /// Creates a meter with a time constant of about `milliseconds`, at `sample_rate` in Hz.
    ///
    /// The time constant is rounded to the nearest power of two frames, from 2 to 2³¹.
    pub fn with_time_constant(sample_rate: u32, milliseconds: u32) -> Self {
        let frames = frames(sample_rate, milliseconds);
        let mut shift = frames.ilog2();

        if (frames as u128 * frames as u128) > 1u128 << (2 * shift + 1) {
            shift += 1;
        }

        Self {
            shift: shift.clamp(1, 31),
            channels: 0,
            sums: [0; MAXIMUM_CHANNELS],
            representation: PhantomData,
        }
    }

    /// Returns the time constant in frames.
    pub const fn time_constant(&self) -> u32 {
        1 << self.shift
    }

    /// Returns the number of channels of the last frame, or 0 before the first one.
    pub const fn channels(&self) -> u8 {
        self.channels
    }

    /// Returns the DC offset of `channel`, or silence if there is no such channel.
    pub fn offset(&self, channel: usize) -> R::Value {
        self.sums.get(channel).map_or(R::SILENCE, |sum| {
            R::from_sample(shift_right(*sum, self.shift))
        })
    }

    /// Returns the magnitude of the DC offset of `channel` in dBFS, or negative infinity when it is 0.
    pub fn offset_decibels(&self, channel: usize) -> f32 {
        math::amplitude_decibels(R::to_sample::<f32>(self.offset(channel)).abs())
    }

    /// Sets the offsets back to 0 and keeps the time constant.
    pub fn reset(&mut self) {
        self.channels = 0;
        self.sums = [0; MAXIMUM_CHANNELS];
    }
}

impl<R: Representation> Analyzer<R> for DcOffset<R> {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        self.channels = frame.len() as u8;

        for (value, sum) in frame.iter().zip(self.sums.iter_mut()) {
            *sum += R::to_sample::<i32>(*value) as i64 - shift_right(*sum, self.shift) as i64;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Window<V> {
    peak: V,
    squares: u64,
}

impl<V> Window<V> {
    const fn new(silence: V) -> Self {
        Self {
            peak: silence,
            squares: 0,
        }
    }
}

/// Measures the crest factor of each channel, its peak over its RMS level, over a window.
///
/// Results are those of the last complete window, and are 0 before the first one ends. The peak is exact. The RMS level is measured on magnitudes rounded to 16 bits, which keeps it within about 0.01 dB down to -50 dBFS and 0.1 dB at -80 dBFS.
///
/// In [`Float`](nanoaudio_dsp::Float), the peak keeps values beyond full scale while the RMS level counts them as full scale, and NaN counts as silence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrestFactor<R: Measure> {
    window: u32,
    frames: u32,
    channels: u8,
    current: [Window<R::Value>; MAXIMUM_CHANNELS],
    last: [Window<R::Value>; MAXIMUM_CHANNELS],
}

impl<R: Measure> CrestFactor<R> {
    /// Creates a meter with a window of one second, at `sample_rate` in Hz.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_window(sample_rate, 1000)
    }

    /// Creates a meter with a window of `milliseconds`, at `sample_rate` in Hz.
    ///
    /// The window is at least one frame and at most `u32::MAX` frames.
    pub fn with_window(sample_rate: u32, milliseconds: u32) -> Self {
        Self {
            window: frames(sample_rate, milliseconds).min(u32::MAX as u64) as u32,
            frames: 0,
            channels: 0,
            current: [Window::new(R::SILENCE); MAXIMUM_CHANNELS],
            last: [Window::new(R::SILENCE); MAXIMUM_CHANNELS],
        }
    }

    /// Returns the window length in frames.
    pub const fn window(&self) -> u32 {
        self.window
    }

    /// Returns the number of channels of the last frame, or 0 before the first one.
    pub const fn channels(&self) -> u8 {
        self.channels
    }

    /// Returns the peak magnitude of `channel` over the last window.
    pub fn peak(&self, channel: usize) -> R::Value {
        self.last
            .get(channel)
            .map_or(R::SILENCE, |window| window.peak)
    }

    /// Returns the RMS level of `channel` over the last window.
    pub fn rms(&self, channel: usize) -> R::Value {
        let squares = self.last.get(channel).map_or(0, |window| window.squares);
        let window = self.window as u64;
        let mean = ((squares / window) << 30) + ((squares % window) << 30) / window;

        R::from_sample(mean.isqrt() as i32)
    }

    /// Returns the crest factor of `channel` over the last window in dB, or `None` when its RMS level is 0.
    pub fn crest_factor(&self, channel: usize) -> Option<f32> {
        let rms = R::to_sample::<f32>(self.rms(channel));

        if rms == 0.0 {
            return None;
        }

        Some(math::amplitude_decibels(
            R::to_sample::<f32>(self.peak(channel)) / rms,
        ))
    }

    /// Clears the results and the current window, and keeps the window length.
    pub fn reset(&mut self) {
        *self = Self {
            window: self.window,
            ..Self::with_window(0, 0)
        };
    }
}

impl<R: Measure> Analyzer<R> for CrestFactor<R> {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        self.channels = frame.len() as u8;

        self.frames += 1;

        let end = self.frames == self.window;

        if end {
            self.frames = 0;
        }

        for ((value, window), last) in frame
            .iter()
            .zip(self.current.iter_mut())
            .zip(self.last.iter_mut())
        {
            let magnitude = R::magnitude(*value);

            if magnitude > window.peak {
                window.peak = magnitude;
            }

            let rounded = ((R::to_sample::<i32>(*value).unsigned_abs() >> 14) + 1) >> 1;
            let rounded = rounded.min(u16::MAX as u32);

            window.squares += (rounded * rounded) as u64;

            if end {
                *last = *window;
                *window = Window::new(R::SILENCE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use nanoaudio_core::Format;
    use nanoaudio_dsp::{Fixed, Float, Stream, analyze_bytes_using, analyze_using, samples_using};

    use super::*;

    fn feed<R, A>(analyzer: A, input: &[i16], channels: u8) -> A
    where
        R: Representation,
        A: Analyzer<R> + Clone + PartialEq + core::fmt::Debug,
    {
        let mut tapped = analyzer.clone();
        let mut sliced = analyzer.clone();
        let mut bytes = analyzer;
        let mut output = std::vec![0i16; input.len()];

        let frames = samples_using::<R, _>(input, channels)
            .unwrap()
            .tap(&mut tapped)
            .write_samples(&mut output);
        let raw: Vec<u8> = input
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();

        assert_eq!(frames, input.len() / channels as usize);
        assert_eq!(
            &output[..frames * channels as usize],
            &input[..frames * channels as usize]
        );
        assert_eq!(
            analyze_using::<R, _, _>(input, channels, &mut sliced),
            Ok(frames)
        );
        assert_eq!(
            analyze_bytes_using::<R, _>(&raw, Format::I16, channels, &mut bytes),
            Ok(frames)
        );
        assert_eq!(tapped, sliced);
        assert_eq!(tapped, bytes);

        tapped
    }

    fn interleave(channels: &[Vec<i16>]) -> Vec<i16> {
        (0..channels[0].len())
            .flat_map(|index| channels.iter().map(move |channel| channel[index]))
            .collect()
    }

    fn sine(frames: usize, period: f64, amplitude: f64, offset: f64) -> Vec<i16> {
        (0..frames)
            .map(|index| {
                let phase = 2.0 * core::f64::consts::PI * index as f64 / period;

                (offset + amplitude * phase.sin()).round() as i16
            })
            .collect()
    }

    fn reference_clips(samples: &[i16], run_length: u32) -> (u64, u64, u32) {
        let (mut clipped, mut runs, mut run, mut longest) = (0, 0, 0, 0);

        for sample in samples {
            if *sample == i16::MAX || *sample <= -i16::MAX {
                clipped += 1;
                run += 1;
                longest = longest.max(run);

                if run == run_length {
                    runs += 1;
                }
            } else {
                run = 0;
            }
        }

        (clipped, runs, longest)
    }

    fn clips<R>()
    where
        R: Measure + PartialEq + core::fmt::Debug,
        R::Value: core::fmt::Debug,
    {
        let left = std::vec![
            0, 32767, 0, 32767, 32767, 0, -32768, -32767, 32767, 1, 32766, -32766, 32767, 32767,
            32767, 32767, 32767, 0, -32768, -32768, -32768,
        ];
        let right: Vec<i16> = left.iter().rev().map(|sample| sample / 2).collect();

        for (channels, signals) in [
            (1, std::vec![left.clone()]),
            (2, std::vec![left.clone(), right]),
        ] {
            let counter = feed::<R, _>(ClipCounter::<R>::new(), &interleave(&signals), channels);

            assert_eq!(counter.channels(), channels);
            assert_eq!(counter.run_length(), 3);

            for (channel, signal) in signals.iter().enumerate() {
                let (clipped, runs, longest) = reference_clips(signal, 3);

                assert_eq!(counter.samples(channel), clipped);
                assert_eq!(counter.runs(channel), runs);
                assert_eq!(counter.longest_run(channel), longest);
            }

            assert_eq!(counter.samples(channels as usize), 0);
            assert_eq!(counter.samples(MAXIMUM_CHANNELS), 0);
        }

        let (clipped, runs, longest) = reference_clips(&left, 3);

        assert_eq!((clipped, runs, longest), (14, 3, 5));

        let mut counter = feed::<R, _>(
            ClipCounter::<R>::with_threshold(R::from_sample(16384i16), 0),
            &left,
            1,
        );

        assert_eq!(counter.run_length(), 1);
        assert_eq!(counter.samples(0), 16);
        assert_eq!(counter.runs(0), 5);

        counter.reset();
        assert_eq!(
            counter,
            ClipCounter::with_threshold(R::from_sample(16384i16), 1)
        );

        let empty = feed::<R, _>(ClipCounter::<R>::default(), &[], 2);

        assert_eq!(empty, ClipCounter::new());
        assert_eq!(empty.channels(), 0);

        let mut long = ClipCounter::<R>::new();

        long.clips[0].run = u32::MAX - 1;
        long.analyze(&[R::FULL_SCALE]);
        long.analyze(&[R::FULL_SCALE]);
        assert_eq!(long.longest_run(0), u32::MAX);
        assert_eq!(long.samples(0), 2);
    }

    #[test]
    fn clips_fixed() {
        clips::<Fixed>();

        let mut counter = ClipCounter::<Fixed>::new();

        counter.analyze(&[i32::MIN, i32::MAX, 0x7FFF_0000, 0x7FFE_FFFF]);
        assert_eq!(
            [0, 1, 2, 3].map(|channel| counter.samples(channel)),
            [1, 1, 1, 0]
        );
        assert_eq!(counter.threshold(), 0x7FFF_0000);
    }

    #[test]
    fn clips_float() {
        clips::<Float>();

        let mut counter = ClipCounter::<Float>::new();

        counter.analyze(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.5, -1.0, 0.999]);
        assert_eq!(
            [0, 1, 2, 3, 4, 5].map(|channel| counter.samples(channel)),
            [0, 1, 1, 1, 1, 0]
        );
    }

    #[test]
    fn time_constants() {
        let constant = |rate, milliseconds| {
            DcOffset::<Fixed>::with_time_constant(rate, milliseconds).time_constant()
        };

        assert_eq!(DcOffset::<Fixed>::new(48_000).time_constant(), 65_536);
        assert_eq!(DcOffset::<Float>::new(44_100).time_constant(), 32_768);
        assert_eq!(constant(48_000, 10), 512);
        assert_eq!(constant(1000, 1), 2);
        assert_eq!(constant(0, 0), 2);
        assert_eq!(constant(u32::MAX, u32::MAX), 1 << 31);

        for frames in 2u64..5000 {
            let shift = constant(1000, frames as u32).ilog2() as i32;
            let error = ((frames as f64).log2() - shift as f64).abs();

            assert!(error <= 0.5 + 1.0e-9, "{frames}");
        }
    }

    fn offsets<R>()
    where
        R: Representation + PartialEq + core::fmt::Debug,
    {
        let rate = 8000;
        let meter = DcOffset::<R>::with_time_constant(rate, 16);
        let shift = meter.time_constant().ilog2();

        assert_eq!(meter.time_constant(), 128);

        let frames = 300;
        let signals = std::vec![std::vec![1000i16; frames], std::vec![-12_345i16; frames]];

        for channels in [1u8, 2] {
            let meter = feed::<R, _>(meter, &interleave(&signals[..channels as usize]), channels);

            for (channel, signal) in signals.iter().take(channels as usize).enumerate() {
                let value = signal[0] as f64 / 32_768.0;
                let expected =
                    value * (1.0 - (1.0 - 1.0 / (1u64 << shift) as f64).powi(frames as i32));
                let offset = R::to_sample::<f32>(meter.offset(channel)) as f64;

                assert!((offset - expected).abs() < 1.0e-8, "{offset} {expected}");
            }
        }

        let settled = feed::<R, _>(
            meter,
            &interleave(&[std::vec![1000i16; 10_000], std::vec![-32_768; 10_000]]),
            2,
        );

        assert_eq!(R::to_sample::<i32>(settled.offset(0)), 1000 << 16);
        assert_eq!(R::to_sample::<i32>(settled.offset(1)), i32::MIN);
        assert_eq!(R::to_sample::<i16>(settled.offset(2)), 0);
        assert_eq!(settled.channels(), 2);
        assert!((settled.offset_decibels(1) as f64).abs() < 1.0e-6);
        assert!(
            (settled.offset_decibels(0) as f64 - 20.0 * (1000.0f64 / 32_768.0).log10()).abs()
                < 1.0e-4
        );
        assert_eq!(settled.offset_decibels(2), f32::NEG_INFINITY);

        let biased = sine(48_000, 48.0, 8000.0, 500.0);
        let meter = feed::<R, _>(DcOffset::<R>::new(48_000), &biased, 1);
        let ripple = 8000.0 / (2.0 * core::f64::consts::PI * 65_536.0 / 48.0);
        let expected = 500.0 * (1.0 - (1.0 - 1.0 / 65_536.0f64).powi(48_000));
        let offset = R::to_sample::<f32>(meter.offset(0)) as f64 * 32_768.0;

        assert!(
            (offset - expected).abs() < ripple + 1.0,
            "{offset} {expected}"
        );

        let mut cleared = settled;

        cleared.reset();
        assert_eq!(cleared, DcOffset::with_time_constant(rate, 16));

        let empty = feed::<R, _>(DcOffset::<R>::new(rate), &[], 1);

        assert_eq!(empty, DcOffset::new(rate));
        assert_eq!(empty.offset_decibels(0), f32::NEG_INFINITY);
    }

    #[test]
    fn offsets_fixed() {
        offsets::<Fixed>();

        let mut meter = DcOffset::<Fixed>::with_time_constant(u32::MAX, u32::MAX);

        meter.sums[0] = i64::MIN / 2;
        meter.sums[1] = i64::MAX / 2;

        for _ in 0..100_000 {
            meter.analyze(&[i32::MIN, i32::MAX]);
        }

        assert_eq!(meter.offset(0), i32::MIN);
        assert_eq!(meter.offset(1), i32::MAX);
    }

    #[test]
    fn offsets_float() {
        offsets::<Float>();

        let mut meter = DcOffset::<Float>::with_time_constant(1, 1);

        meter.analyze(&[f32::NAN, f32::INFINITY, -2.0, 0.25]);
        assert_eq!(
            [0, 1, 2, 3].map(|channel| meter.offset(channel)),
            [0.0, 0.5, -0.5, 0.125]
        );
    }

    fn reference_crest(samples: &[i16]) -> (f64, f64) {
        let peak = samples
            .iter()
            .map(|sample| (*sample as f64).abs().min(32_767.0))
            .fold(0.0, f64::max);
        let squares: f64 = samples
            .iter()
            .map(|sample| (*sample as f64 / 32_768.0).powi(2))
            .sum();

        (peak / 32_768.0, (squares / samples.len() as f64).sqrt())
    }

    fn crests<R>()
    where
        R: Measure + PartialEq + core::fmt::Debug,
        R::Value: core::fmt::Debug,
    {
        let window = 4800;
        let signals = std::vec![
            sine(2 * window + 100, 48.0, 20_000.0, 0.0),
            (0..2 * window + 100)
                .map(|index| if index % 10 < 5 { 9000 } else { -9000 })
                .collect::<Vec<i16>>(),
        ];
        let meter = CrestFactor::<R>::with_window(48_000, 100);

        assert_eq!(meter.window(), window as u32);
        assert_eq!(meter.crest_factor(0), None);
        assert_eq!(R::to_sample::<i32>(meter.rms(0)), 0);

        for channels in [1u8, 2] {
            let meter = feed::<R, _>(meter, &interleave(&signals[..channels as usize]), channels);

            assert_eq!(meter.channels(), channels);

            for (channel, signal) in signals.iter().take(channels as usize).enumerate() {
                let (peak, rms) = reference_crest(&signal[window..2 * window]);
                let measured_peak = R::to_sample::<f32>(meter.peak(channel)) as f64;
                let measured_rms = R::to_sample::<f32>(meter.rms(channel)) as f64;
                let crest = meter.crest_factor(channel).unwrap() as f64;

                assert!(
                    (measured_peak - peak).abs() < 1.0e-9,
                    "{measured_peak} {peak}"
                );
                assert!(
                    (measured_rms / rms - 1.0).abs() < 1.0e-6,
                    "{measured_rms} {rms}"
                );
                assert!((crest - 20.0 * (peak / rms).log10()).abs() < 1.0e-4);
            }
        }

        let both = feed::<R, _>(meter, &interleave(&signals), 2);

        assert!((both.crest_factor(0).unwrap() - 3.0103).abs() < 0.01);
        assert!(both.crest_factor(1).unwrap().abs() < 1.0e-4);
        assert_eq!(both.crest_factor(2), None);

        let quiet = sine(window, 48.0, 3.3, 0.0);
        let (peak, rms) = reference_crest(&quiet);
        let measured = feed::<R, _>(meter, &quiet, 1);

        assert!((R::to_sample::<f32>(measured.rms(0)) as f64 - rms).abs() < 1.0e-9);
        assert!((R::to_sample::<f32>(measured.peak(0)) as f64 - peak).abs() < 1.0e-9);

        let silent = feed::<R, _>(meter, &std::vec![0; window], 1);

        assert_eq!(silent.crest_factor(0), None);

        let mut cleared = both;

        cleared.reset();
        assert_eq!(cleared, meter);

        let empty = feed::<R, _>(CrestFactor::<R>::new(48_000), &[], 2);

        assert_eq!(empty, CrestFactor::new(48_000));
    }

    #[test]
    fn crests_fixed() {
        crests::<Fixed>();

        let mut meter = CrestFactor::<Fixed>::with_window(1_000_000, 1000);

        for _ in 0..1_000_000 {
            meter.analyze(&[i32::MIN, i32::MAX, 1 << 15]);
        }

        assert_eq!(meter.peak(0), i32::MAX);
        assert_eq!(meter.rms(0), 65_535 << 15);
        assert_eq!(meter.rms(1), 65_535 << 15);
        assert_eq!(meter.rms(2), 1 << 15);
        assert!(meter.crest_factor(0).unwrap().abs() < 0.001);
    }

    #[test]
    fn crests_float() {
        crests::<Float>();

        let mut meter = CrestFactor::<Float>::with_window(1, 1000);

        meter.analyze(&[f32::NAN, 2.0, -0.5]);
        assert_eq!(meter.peak(0), 0.0);
        assert_eq!(meter.rms(0), 0.0);
        assert_eq!(meter.peak(1), 2.0);
        assert_eq!(meter.rms(1), 65_535.0 / 65_536.0);
        assert_eq!(meter.rms(2), 0.5);
        assert!((meter.crest_factor(1).unwrap() - 6.0207).abs() < 0.001);

        for (amplitude, tolerance) in [
            (1.0e-4f64, 0.1),
            (1.0e-3, 0.03),
            (0.01, 0.01),
            (0.1, 0.01),
            (0.9, 0.01),
        ] {
            let signal: Vec<f32> = (0..4800)
                .map(|index| {
                    (amplitude * (2.0 * core::f64::consts::PI * index as f64 / 48.0 + 0.3).sin())
                        as f32
                })
                .collect();
            let peak = signal
                .iter()
                .fold(0.0f64, |peak, value| peak.max(value.abs() as f64));
            let rms = (signal
                .iter()
                .map(|value| (*value as f64).powi(2))
                .sum::<f64>()
                / signal.len() as f64)
                .sqrt();
            let mut meter = CrestFactor::<Float>::with_window(48_000, 100);

            assert_eq!(
                analyze_using::<Float, _, _>(&signal, 1, &mut meter),
                Ok(4800)
            );
            assert_eq!(meter.peak(0) as f64, peak);

            let error = meter.crest_factor(0).unwrap() as f64 - 20.0 * (peak / rms).log10();

            assert!(error.abs() < tolerance, "{amplitude} {error}");
        }
    }
}
