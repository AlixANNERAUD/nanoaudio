use core::marker::PhantomData;

use nanoaudio_core::MAXIMUM_CHANNELS;
use nanoaudio_dsp::{Analyzer, Representation};

use crate::Measure;

fn log2(value: f32) -> f32 {
    if value.is_nan() || value == f32::INFINITY {
        return value;
    }

    if value <= 0.0 {
        return f32::NEG_INFINITY;
    }

    if value < f32::MIN_POSITIVE {
        return log2(value * 16_777_216.0) - 24.0;
    }

    let bits = value.to_bits();
    let mut exponent = ((bits >> 23) as i32) - 127;
    let mut mantissa = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000);

    if mantissa > core::f32::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }

    let ratio = (mantissa - 1.0) / (mantissa + 1.0);
    let square = ratio * ratio;
    let series =
        1.0 + square * (1.0 / 3.0 + square * (1.0 / 5.0 + square * (1.0 / 7.0 + square / 9.0)));

    exponent as f32 + 2.0 * ratio * series * core::f32::consts::LOG2_E
}

fn decibels(ratio: f32) -> f32 {
    20.0 * core::f32::consts::LOG10_2 * log2(ratio)
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
    pub fn threshold(&self) -> R::Value {
        self.threshold
    }

    /// Returns the number of consecutive clipped samples that make a run.
    pub fn run_length(&self) -> u32 {
        self.run_length
    }

    /// Returns the number of channels of the last frame, or 0 before the first one.
    pub fn channels(&self) -> u8 {
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
/// It runs in fixed point for both representations and is exact for a constant input. In [`Float`](nanoaudio_dsp::Float), values beyond full scale count as full scale and NaN counts as 0.
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
    /// The time constant is rounded to the nearest power of two frames, up to 2³¹.
    pub fn with_time_constant(sample_rate: u32, milliseconds: u32) -> Self {
        let frames = frames(sample_rate, milliseconds);
        let mut shift = frames.ilog2();

        if (frames as u128 * frames as u128) > 1u128 << (2 * shift + 1) {
            shift += 1;
        }

        Self {
            shift: shift.min(31),
            channels: 0,
            sums: [0; MAXIMUM_CHANNELS],
            representation: PhantomData,
        }
    }

    /// Returns the time constant in frames.
    pub fn time_constant(&self) -> u32 {
        1 << self.shift
    }

    /// Returns the number of channels of the last frame, or 0 before the first one.
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Returns the DC offset of `channel`, or silence if there is no such channel.
    pub fn offset(&self, channel: usize) -> R::Value {
        self.sums.get(channel).map_or(R::SILENCE, |sum| {
            R::from_sample((*sum >> self.shift) as i32)
        })
    }

    /// Returns the magnitude of the DC offset of `channel` in dBFS, or negative infinity when it is 0.
    pub fn offset_decibels(&self, channel: usize) -> f32 {
        decibels(R::to_sample::<f32>(self.offset(channel)).abs())
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
            *sum += R::to_sample::<i32>(*value) as i64 - (*sum >> self.shift);
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
/// Results are those of the last complete window, and are 0 before the first one ends. The peak is exact. The RMS level is measured on samples rounded to 16 bits, which is within 0.03 dB down to -80 dBFS.
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
    pub fn window(&self) -> u32 {
        self.window
    }

    /// Returns the number of channels of the last frame, or 0 before the first one.
    pub fn channels(&self) -> u8 {
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

        Some(decibels(R::to_sample::<f32>(self.peak(channel)) / rms))
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

        for (value, window) in frame.iter().zip(self.current.iter_mut()) {
            let magnitude = R::magnitude(*value);

            if magnitude > window.peak {
                window.peak = magnitude;
            }

            let rounded = ((R::to_sample::<i32>(*value).unsigned_abs() >> 14) + 1) >> 1;
            let rounded = rounded.min(u16::MAX as u32);

            window.squares += (rounded * rounded) as u64;
        }

        self.frames += 1;

        if self.frames == self.window {
            self.frames = 0;
            self.last = self.current;
            self.current = [Window::new(R::SILENCE); MAXIMUM_CHANNELS];
        }
    }
}
