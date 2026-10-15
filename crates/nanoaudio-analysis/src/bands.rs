use core::fmt::Debug;
use core::time::Duration;

use nanoaudio_core::MAXIMUM_CHANNELS;
use nanoaudio_dsp::{Analyzer, Fixed, Float, Representation};

use crate::math::{self, wide};
use crate::{Power, sealed};

mod state {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct FixedBand {
        pub(super) b0: i32,
        pub(super) a1: i32,
        pub(super) a2: i32,
        pub(super) base: u32,
        pub(super) shift: u32,
        pub(super) y1: i32,
        pub(super) y2: i32,
        pub(super) error: i32,
        pub(super) power: u64,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct FloatBand {
        pub(super) a1: f32,
        pub(super) a2: f32,
        pub(super) a3: f32,
        pub(super) base: u32,
        pub(super) rate: f32,
        pub(super) gain: f32,
        pub(super) ic1: f32,
        pub(super) ic2: f32,
        pub(super) power: f32,
    }
}

use state::{FixedBand, FloatBand};

/// A representation a [`BandAnalyzer`] can filter, [`Fixed`] or [`Float`].
pub trait Filterable: Representation + Debug + Copy + PartialEq + sealed::Sealed {
    #[doc(hidden)]
    type Band: Copy + Debug + PartialEq;
    #[doc(hidden)]
    type Level: Copy + Debug + PartialEq;
    #[doc(hidden)]
    type Coefficient: Copy + Debug + PartialEq;
    #[doc(hidden)]
    type History: Copy + Debug + PartialEq;
}

impl Filterable for Fixed {
    type Band = FixedBand;
    type Level = u64;
    type Coefficient = i32;
    type History = [i32; 2];
}

impl Filterable for Float {
    type Band = FloatBand;
    type Level = f32;
    type Coefficient = f32;
    type History = ();
}

const BLOCK: u8 = 16;
const PERIODS: f64 = 4.0;
const HEADROOM: u32 = 2;
const FRACTION: u32 = 30;
const ONE: f64 = (1u64 << FRACTION) as f64;
const HIGHEST: f64 = 0.45;
const LOWEST: f64 = 1.0 / 65536.0;
const NARROWEST: f64 = 1.0 / 48.0;
const WIDEST: f64 = 5.0;
const FLOAT_LIMIT: f32 = 65536.0;
const DEFAULT_ATTACK: Duration = Duration::from_millis(50);
const DEFAULT_RELEASE: Duration = Duration::from_millis(300);
const ISO_LOWEST: f32 = 31.5;
const ISO_HIGHEST: f32 = 16_000.0;

/// Levels of `N` frequency bands for an equalizer or spectrum display, measured with a bank of band-pass filters instead of an FFT.
///
/// Each band is a second-order band-pass filter, the bilinear transform of an analog resonator with its bandwidth corrected for frequency warping. Its output is squared and averaged, and the level falls from its last peak with a release time (see [`with_ballistics`](Self::with_ballistics)). A full-scale sine at the centre of a band reads full scale, 0 dB, in that band. Filters are 3 dB down at the band edges and fall by 6 dB per octave beyond, so a sine one octave from the centre reads about 7.4 dB lower in an octave band and 16.3 dB lower in a third-octave band. Averaged over time, pink noise reads within about 1 dB in every band of [`octaves`](Self::octaves) or [`third_octaves`](Self::third_octaves).
///
/// It analyzes the mean of all channels until [`with_channel`](Self::with_channel) selects one. Centres are kept between `sample_rate / 65536` and 0.9 times the Nyquist frequency, so bands above it measure the top of the spectrum.
///
/// With [`Fixed`], each band costs four 32 × 32-bit multiplies per frame and no floating point. The filters run in direct form I with Q30 coefficients, a 64-bit accumulator and error feedback, which keeps the self noise of a 20 Hz band at 192 kHz below -120 dBFS. Rounding the coefficients places centres within 0.1 % down to `sample_rate / 13000`, and changes responses by less than 0.03 dB down to 20 Hz at 192 kHz. With [`Float`], each band is a state-variable filter, which keeps its precision at low frequencies in `f32`. NaN samples read as silence and other samples are clamped to ±65536, so they cannot stick in the filters.
///
/// ```
/// use nanoaudio_analysis::{BandAnalyzer, analyze_using};
/// use nanoaudio_dsp::Fixed;
///
/// let tone: [i16; 48_000] = core::array::from_fn(|index| {
///     let phase = 2.0 * core::f32::consts::PI * 1000.0 * index as f32 / 48_000.0;
///     (16384.0 * phase.sin()) as i16
/// });
/// let mut bands = BandAnalyzer::<Fixed, 10>::octaves(48_000);
///
/// analyze_using::<Fixed, _, _>(&tone, 1, &mut bands).unwrap();
///
/// let decibels = bands.decibels();
/// assert_eq!(bands.centres()[5], 1000.0);
/// assert!((decibels[5] + 6.02).abs() < 0.2);
/// assert!((decibels[4] + 13.4).abs() < 0.3 && (decibels[6] + 13.4).abs() < 0.3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandAnalyzer<R: Filterable, const N: usize> {
    bands: [R::Band; N],
    levels: [R::Level; N],
    history: R::History,
    centres: [f32; N],
    release: R::Coefficient,
    sample_rate: u32,
    position: u8,
    channel: Option<u8>,
}

impl<R: Filterable, const N: usize> BandAnalyzer<R, N> {
    /// Returns the centre frequency of each band in Hz, after clamping.
    pub const fn centres(&self) -> [f32; N] {
        self.centres
    }

    /// Analyzes only the channel at `channel`, counted from 0, instead of the mix of all channels.
    ///
    /// A frame without that channel counts as silence.
    pub const fn with_channel(mut self, channel: u8) -> Self {
        self.channel = Some(channel);
        self
    }
}

#[derive(Clone, Copy)]
struct Design {
    centre: f64,
    k: f64,
    g: f64,
    b0: f64,
    a1: f64,
    a2: f64,
    shift: u32,
}

const fn design(sample_rate: u32, centre: f32, octaves: f32) -> Design {
    let rate = if sample_rate == 0 {
        1.0
    } else {
        sample_rate as f64
    };
    let centre = clamp(centre as f64, LOWEST * rate, HIGHEST * rate);
    let octaves = clamp(octaves as f64, NARROWEST, WIDEST);
    let w0 = 2.0 * core::f64::consts::PI * centre / rate;
    let (sine, cosine) = (math::sine(w0), math::cosine(w0));
    let k = 2.0 * math::sinh(core::f64::consts::LN_2 / 2.0 * octaves * w0 / sine);
    let k = clamp(k, bandwidth(NARROWEST), bandwidth(WIDEST));
    let alpha = sine * k / 2.0;
    let lower = centre * (math::square_root(k * k + 4.0) - k) / 2.0;
    let shift = math::round(math::log2(PERIODS * rate / lower));

    Design {
        centre,
        k,
        g: sine / (1.0 + cosine),
        b0: alpha / (1.0 + alpha),
        a1: -2.0 * cosine / (1.0 + alpha),
        a2: (1.0 - alpha) / (1.0 + alpha),
        shift: if shift < 0 {
            0
        } else if shift > 40 {
            40
        } else {
            shift as u32
        },
    }
}

const fn bandwidth(octaves: f64) -> f64 {
    2.0 * math::sinh(core::f64::consts::LN_2 / 2.0 * octaves)
}

const fn clamp(value: f64, lowest: f64, highest: f64) -> f64 {
    if value >= highest {
        highest
    } else if value >= lowest {
        value
    } else {
        lowest
    }
}

const fn quantize(value: f64) -> i32 {
    let value = math::round(value * ONE);

    if value >= i32::MAX as i64 {
        i32::MAX
    } else if value <= i32::MIN as i64 {
        i32::MIN
    } else {
        value as i32
    }
}

const fn integration(duration: Duration, sample_rate: u32) -> u32 {
    let frames = duration.as_secs_f64() * sample_rate as f64;

    if frames <= 1.0 {
        return 0;
    }

    let shift = math::round(math::log2(frames));

    if shift > 40 { 40 } else { shift as u32 }
}

const fn larger(first: u32, second: u32) -> u32 {
    if first > second { first } else { second }
}

const fn ballistic(duration: Duration, sample_rate: u32) -> f64 {
    let seconds = duration.as_secs_f64() * sample_rate as f64;

    if seconds <= 0.0 {
        return 1.0;
    }

    1.0 - math::exp(-2.0 * BLOCK as f64 / seconds)
}

const fn fixed_ballistic(duration: Duration, sample_rate: u32) -> i32 {
    let value = math::round(ballistic(duration, sample_rate) * (1u64 << 31) as f64);

    if value >= i32::MAX as i64 {
        i32::MAX
    } else {
        value as i32
    }
}

const fn power_of_two(shift: u32) -> f32 {
    f32::from_bits((127 - shift) << 23)
}

const fn spaced<const N: usize>(sample_rate: u32, lowest: f32, highest: f32) -> ([f32; N], f32) {
    let rate = if sample_rate == 0 {
        1.0
    } else {
        sample_rate as f64
    };
    let highest = clamp(highest as f64, LOWEST * rate, HIGHEST * rate);
    let lowest = clamp(lowest as f64, LOWEST * rate, highest);
    let span = math::log2(highest / lowest);
    let mut centres = [0.0; N];

    if N == 1 {
        centres[0] = math::square_root(lowest * highest) as f32;
        return (centres, span as f32);
    }

    let step = span / (N as f64 - 1.0);
    let mut index = 0;

    while index < N {
        centres[index] = (lowest * math::exp2(step * index as f64)) as f32;
        index += 1;
    }

    (centres, step as f32)
}

const fn standard<const N: usize>(per_octave: i32, reference: i32) -> [f32; N] {
    let mut centres = [0.0; N];
    let mut index = 0;

    while index < N {
        let exponent = (index as i32 - reference) as f64 / per_octave as f64;

        centres[index] = (1000.0 * math::exp2(exponent)) as f32;
        index += 1;
    }

    centres
}

impl<const N: usize> BandAnalyzer<Fixed, N> {
    /// Creates an analyzer of bands centred on `centres`, in Hz, each `octaves` wide between its -3 dB edges, for audio at `sample_rate`.
    ///
    /// Bandwidths are clamped between 1/48 and 5 octaves. Floating point is used here only, never per frame.
    pub const fn new(sample_rate: u32, centres: [f32; N], octaves: f32) -> Self {
        let mut bands = [FixedBand {
            b0: 0,
            a1: 0,
            a2: 0,
            base: 0,
            shift: 0,
            y1: 0,
            y2: 0,
            error: 0,
            power: 0,
        }; N];
        let mut clamped = [0.0; N];
        let mut index = 0;

        while index < N {
            let design = design(sample_rate, centres[index], octaves);

            bands[index].b0 = quantize(design.b0);
            bands[index].a1 = quantize(-design.a1);
            bands[index].a2 = quantize(-design.a2);
            bands[index].base = design.shift;
            bands[index].shift = larger(design.shift, integration(DEFAULT_ATTACK, sample_rate));
            clamped[index] = design.centre as f32;
            index += 1;
        }

        Self {
            bands,
            levels: [0; N],
            history: [0; 2],
            centres: clamped,
            release: fixed_ballistic(DEFAULT_RELEASE, sample_rate),
            sample_rate,
            position: 0,
            channel: None,
        }
    }

    /// Creates an analyzer of `N` bands spread evenly on a logarithmic scale from `lowest` to `highest`, in Hz, each as wide as the spacing.
    pub const fn logarithmic(sample_rate: u32, lowest: f32, highest: f32) -> Self {
        let (centres, octaves) = spaced::<N>(sample_rate, lowest, highest);

        Self::new(sample_rate, centres, octaves)
    }

    /// Creates an analyzer of `N` bands spread from 31.5 Hz to 16 kHz, for displays of 3, 5 or any other number of bars.
    pub const fn spread(sample_rate: u32) -> Self {
        Self::logarithmic(sample_rate, ISO_LOWEST, ISO_HIGHEST)
    }

    /// Sets how fast the levels rise, `attack`, and fall, `release`.
    ///
    /// Squares are averaged with a time constant of `attack`, rounded to a power of two frames, or of four periods of the band's lower edge if that is longer. The level then falls by about 8.7 dB per `release`. The default is 50 ms and 300 ms; shorter attacks make narrow bands of noise flicker and read up to 1 dB higher than wide ones.
    pub const fn with_ballistics(mut self, attack: Duration, release: Duration) -> Self {
        let attack = integration(attack, self.sample_rate);
        let mut index = 0;

        while index < N {
            self.bands[index].shift = larger(self.bands[index].base, attack);
            index += 1;
        }

        self.release = fixed_ballistic(release, self.sample_rate);
        self
    }

    /// Forgets every frame analyzed and keeps the settings.
    pub fn reset(&mut self) {
        for band in &mut self.bands {
            band.y1 = 0;
            band.y2 = 0;
            band.error = 0;
            band.power = 0;
        }

        self.levels = [0; N];
        self.history = [0; 2];
        self.position = 0;
    }

    /// Returns the level of each band, `i32::MAX` for a full-scale sine at its centre.
    ///
    /// It saturates at `i32::MAX`, which louder signals such as a full-scale square wave can reach.
    pub fn levels(&self) -> [i32; N] {
        self.levels.map(|level| {
            let amplitude = (level << 1).isqrt() << HEADROOM;

            amplitude.min(i32::MAX as u64) as i32
        })
    }

    /// Returns the level of each band in dBFS, 0 for a full-scale sine at its centre and negative infinity for silence.
    pub fn decibels(&self) -> [f32; N] {
        self.levels.map(|level| {
            let power = level as f32 * 2.0 / (1u64 << (2 * (31 - HEADROOM))) as f32;

            <Float as Power>::amplitude_decibels(power) / 2.0
        })
    }

    #[cold]
    #[inline(never)]
    fn update(&mut self) {
        self.position = 0;

        for (level, band) in self.levels.iter_mut().zip(&self.bands) {
            if band.power > *level {
                *level = band.power;
            } else {
                let difference = band.power as i64 - *level as i64;

                *level = (*level as i64 + scale(difference, self.release)) as u64;
            }
        }
    }
}

impl BandAnalyzer<Fixed, 10> {
    /// Creates an analyzer of the ten octave bands from 31.25 Hz to 16 kHz, centred on powers of two times 1 kHz.
    pub const fn octaves(sample_rate: u32) -> Self {
        Self::new(sample_rate, standard::<10>(1, 5), 1.0)
    }
}

impl BandAnalyzer<Fixed, 31> {
    /// Creates an analyzer of the 31 third-octave bands from 19.7 Hz to 20.2 kHz, centred on 1 kHz times powers of 2^(1/3).
    pub const fn third_octaves(sample_rate: u32) -> Self {
        Self::new(sample_rate, standard::<31>(3, 17), 1.0 / 3.0)
    }
}

impl<const N: usize> BandAnalyzer<Float, N> {
    /// Creates an analyzer of bands centred on `centres`, in Hz, each `octaves` wide between its -3 dB edges, for audio at `sample_rate`.
    ///
    /// Bandwidths are clamped between 1/48 and 5 octaves.
    pub const fn new(sample_rate: u32, centres: [f32; N], octaves: f32) -> Self {
        let mut bands = [FloatBand {
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            base: 0,
            rate: 0.0,
            gain: 0.0,
            ic1: 0.0,
            ic2: 0.0,
            power: 0.0,
        }; N];
        let mut clamped = [0.0; N];
        let mut index = 0;

        while index < N {
            let design = design(sample_rate, centres[index], octaves);
            let a1 = 1.0 / (1.0 + design.g * (design.g + design.k));

            bands[index].a1 = a1 as f32;
            bands[index].a2 = (design.g * a1) as f32;
            bands[index].a3 = (design.g * design.g * a1) as f32;
            bands[index].base = design.shift;
            bands[index].rate = power_of_two(larger(
                design.shift,
                integration(DEFAULT_ATTACK, sample_rate),
            ));
            bands[index].gain = (2.0 * design.k * design.k) as f32;
            clamped[index] = design.centre as f32;
            index += 1;
        }

        Self {
            bands,
            levels: [0.0; N],
            history: (),
            centres: clamped,
            release: ballistic(DEFAULT_RELEASE, sample_rate) as f32,
            sample_rate,
            position: 0,
            channel: None,
        }
    }

    /// Creates an analyzer of `N` bands spread evenly on a logarithmic scale from `lowest` to `highest`, in Hz, each as wide as the spacing.
    pub const fn logarithmic(sample_rate: u32, lowest: f32, highest: f32) -> Self {
        let (centres, octaves) = spaced::<N>(sample_rate, lowest, highest);

        Self::new(sample_rate, centres, octaves)
    }

    /// Creates an analyzer of `N` bands spread from 31.5 Hz to 16 kHz, for displays of 3, 5 or any other number of bars.
    pub const fn spread(sample_rate: u32) -> Self {
        Self::logarithmic(sample_rate, ISO_LOWEST, ISO_HIGHEST)
    }

    /// Sets how fast the levels rise, `attack`, and fall, `release`.
    ///
    /// Squares are averaged with a time constant of `attack`, rounded to a power of two frames, or of four periods of the band's lower edge if that is longer. The level then falls by about 8.7 dB per `release`. The default is 50 ms and 300 ms; shorter attacks make narrow bands of noise flicker and read up to 1 dB higher than wide ones.
    pub const fn with_ballistics(mut self, attack: Duration, release: Duration) -> Self {
        let attack = integration(attack, self.sample_rate);
        let mut index = 0;

        while index < N {
            let shift = larger(self.bands[index].base, attack);

            self.bands[index].rate = power_of_two(shift);
            index += 1;
        }

        self.release = ballistic(release, self.sample_rate) as f32;
        self
    }

    /// Forgets every frame analyzed and keeps the settings.
    pub fn reset(&mut self) {
        for band in &mut self.bands {
            band.ic1 = 0.0;
            band.ic2 = 0.0;
            band.power = 0.0;
        }

        self.levels = [0.0; N];
        self.position = 0;
    }

    /// Returns the level of each band, 1.0 for a full-scale sine at its centre.
    pub fn levels(&self) -> [f32; N] {
        core::array::from_fn(|index| {
            math::square_root((self.levels[index] * self.bands[index].gain) as f64) as f32
        })
    }

    /// Returns the level of each band in dBFS, 0 for a full-scale sine at its centre and negative infinity for silence.
    pub fn decibels(&self) -> [f32; N] {
        core::array::from_fn(|index| {
            let power = self.levels[index] * self.bands[index].gain;

            <Float as Power>::amplitude_decibels(power) / 2.0
        })
    }

    #[cold]
    #[inline(never)]
    fn update(&mut self) {
        self.position = 0;

        for (level, band) in self.levels.iter_mut().zip(&self.bands) {
            if band.power > *level {
                *level = band.power;
            } else {
                *level += (band.power - *level) * self.release;
            }
        }
    }
}

impl BandAnalyzer<Float, 10> {
    /// Creates an analyzer of the ten octave bands from 31.25 Hz to 16 kHz, centred on powers of two times 1 kHz.
    pub const fn octaves(sample_rate: u32) -> Self {
        Self::new(sample_rate, standard::<10>(1, 5), 1.0)
    }
}

impl BandAnalyzer<Float, 31> {
    /// Creates an analyzer of the 31 third-octave bands from 19.7 Hz to 20.2 kHz, centred on 1 kHz times powers of 2^(1/3).
    pub const fn third_octaves(sample_rate: u32) -> Self {
        Self::new(sample_rate, standard::<31>(3, 17), 1.0 / 3.0)
    }
}

const MIX: [(u32, i32); MAXIMUM_CHANNELS + 1] = mix_table();

const fn mix_table() -> [(u32, i32); MAXIMUM_CHANNELS + 1] {
    let mut table = [(0, 0); MAXIMUM_CHANNELS + 1];
    let mut channels = 1;

    while channels <= MAXIMUM_CHANNELS {
        let shift = (channels as u32).next_power_of_two().trailing_zeros();
        let correction = if channels.is_power_of_two() {
            0
        } else {
            quantize((1u32 << shift) as f64 / channels as f64)
        };

        table[channels] = (shift, correction);
        channels += 1;
    }

    table
}

#[inline]
fn mix(frame: &[i32]) -> i32 {
    let frame = &frame[..frame.len().min(MAXIMUM_CHANNELS)];
    let (shift, correction) = MIX[frame.len()];
    let sum = frame
        .iter()
        .fold(0i32, |sum, value| sum + (value >> (shift + HEADROOM)));

    if correction == 0 {
        sum
    } else {
        (wide(sum, correction) >> FRACTION) as i32
    }
}

#[inline]
fn scale(difference: i64, coefficient: i32) -> i64 {
    let high = (difference >> 31) as i32;
    let low = (difference & 0x7FFF_FFFF) as i32;

    wide(high, coefficient) + (wide(low, coefficient) >> 31)
}

#[cfg(not(all(target_arch = "arm", not(target_has_atomic = "32"))))]
#[inline(always)]
fn shift_right(value: i64, shift: u32) -> i64 {
    value >> shift
}

#[cfg(any(test, all(target_arch = "arm", not(target_has_atomic = "32"))))]
#[inline(always)]
fn shift_right_split(value: i64, shift: u32) -> i64 {
    let high = (value >> 32) as i32;

    if shift >= 32 {
        (high >> (shift - 32)) as i64
    } else if shift == 0 {
        value
    } else {
        let low = (value as u32 >> shift) | ((high as u32) << (32 - shift));

        (((high >> shift) as i64) << 32) | low as i64
    }
}

#[cfg(all(target_arch = "arm", not(target_has_atomic = "32")))]
#[inline(always)]
fn shift_right(value: i64, shift: u32) -> i64 {
    shift_right_split(value, shift)
}

impl<const N: usize> Analyzer<Fixed> for BandAnalyzer<Fixed, N> {
    #[inline]
    fn analyze(&mut self, frame: &[i32]) {
        let input = match self.channel {
            Some(channel) => frame.get(channel as usize).copied().unwrap_or(0) >> HEADROOM,
            None => mix(frame),
        };
        let [previous, before] = self.history;
        let difference = input - before;

        self.history = [input, previous];

        for band in &mut self.bands {
            let accumulator = wide(band.b0, difference)
                + wide(band.a1, band.y1)
                + wide(band.a2, band.y2)
                + band.error as i64;
            let output = (accumulator >> FRACTION) as i32;

            band.error = (accumulator as i32) & ((1 << FRACTION) - 1);
            band.y2 = band.y1;
            band.y1 = output;

            let square = wide(output, output);

            band.power =
                (band.power as i64 + shift_right(square - band.power as i64, band.shift)) as u64;
        }

        self.position += 1;

        if self.position == BLOCK {
            self.update();
        }
    }
}

impl<const N: usize> Analyzer<Float> for BandAnalyzer<Float, N> {
    #[inline]
    fn analyze(&mut self, frame: &[f32]) {
        let input = match self.channel {
            Some(channel) => frame.get(channel as usize).copied().unwrap_or(0.0),
            None => {
                let frame = &frame[..frame.len().min(MAXIMUM_CHANNELS)];

                frame.iter().sum::<f32>() * RECIPROCALS[frame.len()]
            }
        };
        let input = if input.is_nan() {
            0.0
        } else {
            input.clamp(-FLOAT_LIMIT, FLOAT_LIMIT)
        };

        for band in &mut self.bands {
            let v3 = input - band.ic2;
            let v1 = band.a1 * band.ic1 + band.a2 * v3;
            let v2 = band.ic2 + band.a2 * band.ic1 + band.a3 * v3;

            band.ic1 = 2.0 * v1 - band.ic1;
            band.ic2 = 2.0 * v2 - band.ic2;
            band.power += (v1 * v1 - band.power) * band.rate;
        }

        self.position += 1;

        if self.position == BLOCK {
            self.update();
        }
    }
}

const RECIPROCALS: [f32; MAXIMUM_CHANNELS + 1] = {
    let mut table = [0.0; MAXIMUM_CHANNELS + 1];
    let mut channels = 1;

    while channels <= MAXIMUM_CHANNELS {
        table[channels] = 1.0 / channels as f32;
        channels += 1;
    }

    table
};

#[cfg(test)]
mod tests {
    extern crate std;

    use std::f64::consts::{LN_2, PI};
    use std::vec::Vec;

    use nanoaudio_dsp::{Stream, analyze_using, samples_using};

    use super::*;

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    const SCALE: f64 = 2_147_483_648.0;

    fn quantized(value: f64) -> i32 {
        (value * SCALE)
            .round()
            .clamp(i32::MIN as f64, i32::MAX as f64) as i32
    }

    fn sine(frequency: f64, amplitude: f64, sample_rate: f64, frames: usize) -> Vec<i32> {
        (0..frames)
            .map(|index| {
                quantized(amplitude * (2.0 * PI * frequency * index as f64 / sample_rate).sin())
            })
            .collect()
    }

    #[derive(Clone, Copy)]
    struct Coefficients {
        centre: f64,
        b0: f64,
        a1: f64,
        a2: f64,
        shift: u32,
    }

    fn coefficients(sample_rate: f64, centre: f64, octaves: f64) -> Coefficients {
        let centre = centre.clamp(sample_rate / 65536.0, 0.45 * sample_rate);
        let octaves = octaves.clamp(1.0 / 48.0, 5.0);
        let w0 = 2.0 * PI * centre / sample_rate;
        let k = (2.0 * (LN_2 / 2.0 * octaves * w0 / w0.sin()).sinh())
            .clamp(2.0 * (LN_2 / 96.0).sinh(), 2.0 * (LN_2 * 2.5).sinh());
        let alpha = w0.sin() * k / 2.0;
        let lower = centre * ((k * k + 4.0).sqrt() - k) / 2.0;

        Coefficients {
            centre,
            b0: alpha / (1.0 + alpha),
            a1: -2.0 * w0.cos() / (1.0 + alpha),
            a2: (1.0 - alpha) / (1.0 + alpha),
            shift: (4.0 * sample_rate / lower).log2().round() as u32,
        }
    }

    fn response(coefficients: &Coefficients, frequency: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * frequency / sample_rate;
        let (cosine, sine) = (w.cos(), w.sin());
        let (cosine2, sine2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let numerator = (coefficients.b0 * (1.0 - cosine2), coefficients.b0 * sine2);
        let denominator = (
            1.0 + coefficients.a1 * cosine + coefficients.a2 * cosine2,
            -coefficients.a1 * sine - coefficients.a2 * sine2,
        );

        ((numerator.0 * numerator.0 + numerator.1 * numerator.1)
            / (denominator.0 * denominator.0 + denominator.1 * denominator.1))
            .sqrt()
    }

    struct Reference {
        bands: Vec<(Coefficients, [f64; 4], f64, f64)>,
        release: f64,
        position: usize,
    }

    impl Reference {
        fn new(sample_rate: f64, centres: &[f32], octaves: f64, ballistics: (f64, f64)) -> Self {
            let attack = if ballistics.0 * sample_rate > 1.0 {
                (ballistics.0 * sample_rate).log2().round() as u32
            } else {
                0
            };

            Self {
                bands: centres
                    .iter()
                    .map(|centre| {
                        let mut coefficients = coefficients(sample_rate, *centre as f64, octaves);
                        coefficients.shift = coefficients.shift.max(attack);
                        (coefficients, [0.0; 4], 0.0, 0.0)
                    })
                    .collect(),
                release: if ballistics.1 <= 0.0 {
                    1.0
                } else {
                    1.0 - (-2.0 * 16.0 / (ballistics.1 * sample_rate)).exp()
                },
                position: 0,
            }
        }

        fn process(&mut self, input: f64) {
            for (coefficients, [x1, x2, y1, y2], power, _) in &mut self.bands {
                let output =
                    coefficients.b0 * (input - *x2) - coefficients.a1 * *y1 - coefficients.a2 * *y2;

                *x2 = *x1;
                *x1 = input;
                *y2 = *y1;
                *y1 = output;
                *power += (output * output - *power) / (1u64 << coefficients.shift) as f64;
            }

            self.position += 1;

            if self.position == 16 {
                self.position = 0;

                for (_, _, power, level) in &mut self.bands {
                    if *power > *level {
                        *level = *power;
                    } else {
                        *level += (*power - *level) * self.release;
                    }
                }
            }
        }

        fn decibels(&self) -> Vec<f64> {
            self.bands
                .iter()
                .map(|(_, _, _, level)| 10.0 * (2.0 * level).log10())
                .collect()
        }
    }

    trait Tested: Filterable {
        const TOLERANCE: f64;

        fn create<const N: usize>(
            sample_rate: u32,
            centres: [f32; N],
            octaves: f32,
        ) -> BandAnalyzer<Self, N>;

        fn feed<const N: usize>(analyzer: &mut BandAnalyzer<Self, N>, signal: &[i32], channels: u8);

        fn decibels<const N: usize>(analyzer: &BandAnalyzer<Self, N>) -> [f32; N];

        fn levels<const N: usize>(analyzer: &BandAnalyzer<Self, N>) -> [f64; N];

        fn reset<const N: usize>(analyzer: &mut BandAnalyzer<Self, N>);

        fn ballistics<const N: usize>(
            analyzer: BandAnalyzer<Self, N>,
            attack: Duration,
            release: Duration,
        ) -> BandAnalyzer<Self, N>;
    }

    impl Tested for Fixed {
        const TOLERANCE: f64 = 0.03;

        fn create<const N: usize>(
            sample_rate: u32,
            centres: [f32; N],
            octaves: f32,
        ) -> BandAnalyzer<Self, N> {
            BandAnalyzer::<Fixed, N>::new(sample_rate, centres, octaves)
        }

        fn feed<const N: usize>(
            analyzer: &mut BandAnalyzer<Self, N>,
            signal: &[i32],
            channels: u8,
        ) {
            analyze_using::<Fixed, i32, _>(signal, channels, analyzer).unwrap();
        }

        fn decibels<const N: usize>(analyzer: &BandAnalyzer<Self, N>) -> [f32; N] {
            analyzer.decibels()
        }

        fn levels<const N: usize>(analyzer: &BandAnalyzer<Self, N>) -> [f64; N] {
            analyzer.levels().map(|level| level as f64 / SCALE)
        }

        fn reset<const N: usize>(analyzer: &mut BandAnalyzer<Self, N>) {
            analyzer.reset();
        }

        fn ballistics<const N: usize>(
            analyzer: BandAnalyzer<Self, N>,
            attack: Duration,
            release: Duration,
        ) -> BandAnalyzer<Self, N> {
            analyzer.with_ballistics(attack, release)
        }
    }

    impl Tested for Float {
        const TOLERANCE: f64 = 0.01;

        fn create<const N: usize>(
            sample_rate: u32,
            centres: [f32; N],
            octaves: f32,
        ) -> BandAnalyzer<Self, N> {
            BandAnalyzer::<Float, N>::new(sample_rate, centres, octaves)
        }

        fn feed<const N: usize>(
            analyzer: &mut BandAnalyzer<Self, N>,
            signal: &[i32],
            channels: u8,
        ) {
            analyze_using::<Float, i32, _>(signal, channels, analyzer).unwrap();
        }

        fn decibels<const N: usize>(analyzer: &BandAnalyzer<Self, N>) -> [f32; N] {
            analyzer.decibels()
        }

        fn levels<const N: usize>(analyzer: &BandAnalyzer<Self, N>) -> [f64; N] {
            analyzer.levels().map(|level| level as f64)
        }

        fn reset<const N: usize>(analyzer: &mut BandAnalyzer<Self, N>) {
            analyzer.reset();
        }

        fn ballistics<const N: usize>(
            analyzer: BandAnalyzer<Self, N>,
            attack: Duration,
            release: Duration,
        ) -> BandAnalyzer<Self, N> {
            analyzer.with_ballistics(attack, release)
        }
    }

    fn compare<R: Tested>(sample_rate: u32, centre: f32, octaves: f32, frequency: f64)
    where
        BandAnalyzer<R, 1>: Analyzer<R>,
    {
        let rate = sample_rate as f64;
        let mut analyzer = R::create(sample_rate, [centre], octaves);
        let mut reference = Reference::new(rate, &[centre], octaves as f64, (0.05, 0.3));
        let design = reference.bands[0].0;
        let frames = (12usize << design.shift) + sample_rate as usize / 2;
        let amplitude = 0.5;
        let signal = sine(frequency, amplitude, rate, frames);

        R::feed(&mut analyzer, &signal, 1);

        for value in &signal {
            reference.process(*value as f64 / SCALE);
        }

        let measured = R::decibels(&analyzer)[0] as f64;
        let expected = reference.decibels()[0];
        assert!((analyzer.centres()[0] as f64 - design.centre).abs() <= design.centre * 1e-6);
        let ideal = 20.0 * (amplitude * response(&design, frequency, rate)).log10();

        assert!(
            (measured - expected).abs() < R::TOLERANCE,
            "{sample_rate} Hz, band {centre} Hz, tone {frequency} Hz: {measured} dB, reference {expected} dB"
        );
        assert!(
            (measured - ideal).abs() < 0.25,
            "{sample_rate} Hz, band {centre} Hz, tone {frequency} Hz: {measured} dB, ideal {ideal} dB"
        );

        let level = R::levels(&analyzer)[0];
        assert!((20.0 * level.log10() - measured).abs() < 1e-3);
    }

    fn layouts() -> Vec<(u32, Vec<f32>, f32)> {
        std::vec![
            (
                48_000,
                BandAnalyzer::<Float, 10>::octaves(48_000)
                    .centres()
                    .to_vec(),
                1.0
            ),
            (
                48_000,
                BandAnalyzer::<Float, 31>::third_octaves(48_000)
                    .centres()
                    .to_vec(),
                1.0 / 3.0
            ),
            (
                44_100,
                spaced::<5>(44_100, 31.5, 16_000.0).0.to_vec(),
                spaced::<5>(44_100, 31.5, 16_000.0).1
            ),
            (
                8_000,
                spaced::<3>(8_000, 31.5, 16_000.0).0.to_vec(),
                spaced::<3>(8_000, 31.5, 16_000.0).1
            ),
            (
                192_000,
                std::vec![19.69, 31.25, 1000.0, 20_000.0],
                1.0 / 3.0
            ),
        ]
    }

    fn sines<R: Tested>()
    where
        BandAnalyzer<R, 1>: Analyzer<R>,
    {
        for (sample_rate, centres, octaves) in layouts() {
            for centre in centres {
                for ratio in [1.0, 2.0, 0.5] {
                    let frequency = centre as f64 * ratio;

                    if frequency < 0.48 * sample_rate as f64 && frequency > 5.0 {
                        compare::<R>(sample_rate, centre, octaves, frequency);
                    }
                }
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn fixed_sines() {
        sines::<Fixed>();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn float_sines() {
        sines::<Float>();
    }

    #[test]
    fn layouts_and_clamping() {
        let octaves = BandAnalyzer::<Fixed, 10>::octaves(48_000).centres();
        assert_eq!(octaves[0], 31.25);
        assert_eq!(octaves[5], 1000.0);
        assert_eq!(octaves[9], 16_000.0);

        let thirds = BandAnalyzer::<Float, 31>::third_octaves(48_000).centres();
        assert!((thirds[0] - 19.686).abs() < 1e-3);
        assert_eq!(thirds[17], 1000.0);
        assert!((thirds[30] - 20_158.737).abs() < 1e-2);
        assert_eq!(
            BandAnalyzer::<Float, 31>::third_octaves(44_100).centres()[30],
            19_845.0
        );

        let three = BandAnalyzer::<Fixed, 3>::spread(48_000).centres();
        assert!((three[0] - 31.5).abs() < 1e-4);
        assert!((three[1] as f64 - (31.5f64 * 16_000.0).sqrt()).abs() < 1e-2);
        assert!((three[2] - 16_000.0).abs() < 1e-2);

        let low = BandAnalyzer::<Float, 5>::spread(8_000).centres();
        assert!((low[4] - 3600.0).abs() < 1e-2);
        assert_eq!(
            BandAnalyzer::<Float, 1>::logarithmic(48_000, 100.0, 400.0).centres(),
            [200.0]
        );
        assert_eq!(
            BandAnalyzer::<Fixed, 2>::new(192_000, [0.0, 1e9], 1.0).centres(),
            [192_000.0 / 65536.0, 86_400.0]
        );

        let none = BandAnalyzer::<Fixed, 0>::spread(48_000);
        assert_eq!(none.levels(), []);
        let _ = BandAnalyzer::<Fixed, 4>::spread(0);
    }

    fn pink<R: Tested, const N: usize>(
        mut analyzer: BandAnalyzer<R, N>,
        highest: f32,
        tolerance: f64,
    ) where
        BandAnalyzer<R, N>: Analyzer<R>,
    {
        let mut random = Random(11);
        let mut state = [0.0f64; 7];
        let frames = 48_000 * 12;
        let signal: Vec<i32> = (0..frames)
            .map(|_| {
                let white = random.next();
                state[0] = 0.99886 * state[0] + white * 0.0555179;
                state[1] = 0.99332 * state[1] + white * 0.0750759;
                state[2] = 0.96900 * state[2] + white * 0.1538520;
                state[3] = 0.86650 * state[3] + white * 0.3104856;
                state[4] = 0.55000 * state[4] + white * 0.5329522;
                state[5] = -0.7616 * state[5] - white * 0.0168980;
                let pink = state.iter().sum::<f64>() + white * 0.5362;
                state[6] = white * 0.115926;
                quantized(0.05 * pink)
            })
            .collect();
        let mut sums = [0.0f64; N];
        let mut reads = 0;

        for (index, chunk) in signal.chunks(480).enumerate() {
            R::feed(&mut analyzer, chunk, 1);

            if index >= 200 {
                for (sum, level) in sums.iter_mut().zip(R::levels(&analyzer)) {
                    *sum += level * level;
                }
                reads += 1;
            }
        }

        let centres = analyzer.centres();
        let averages: Vec<f64> = sums
            .iter()
            .zip(centres)
            .filter(|(_, centre)| *centre <= highest)
            .map(|(sum, _)| 10.0 * (sum / reads as f64).log10())
            .collect();
        let mean = averages.iter().sum::<f64>() / averages.len() as f64;

        for (average, centre) in averages.iter().zip(centres) {
            assert!(
                (average - mean).abs() < tolerance,
                "band {centre} Hz: {average} dB, mean {mean} dB"
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn pink_noise_is_flat() {
        let instant = BandAnalyzer::<Fixed, 31>::third_octaves(48_000)
            .with_ballistics(Duration::ZERO, Duration::ZERO);

        pink(instant, 16_000.0, 0.75);
        pink(BandAnalyzer::<Fixed, 10>::octaves(48_000), 16_000.0, 1.0);
        pink(BandAnalyzer::<Float, 10>::octaves(48_000), 16_000.0, 1.0);
        pink(
            BandAnalyzer::<Fixed, 31>::third_octaves(48_000),
            16_000.0,
            1.25,
        );
        pink(
            BandAnalyzer::<Float, 31>::third_octaves(48_000),
            16_000.0,
            1.25,
        );
    }

    fn channels<R: Tested>()
    where
        BandAnalyzer<R, 10>: Analyzer<R>,
    {
        let frames = 48_000;
        let left = sine(1000.0, 0.5, 48_000.0, frames);
        let right = sine(4000.0, 0.25, 48_000.0, frames);
        let stereo: Vec<i32> = left
            .iter()
            .zip(&right)
            .flat_map(|(left, right)| [*left, *right])
            .collect();
        let triple: Vec<i32> = left.iter().flat_map(|value| [*value; 3]).collect();
        let centres = BandAnalyzer::<Float, 10>::octaves(48_000).centres();
        let check = |analyzer: &BandAnalyzer<R, 10>, input: &mut dyn Iterator<Item = f64>| {
            let mut reference = Reference::new(48_000.0, &centres, 1.0, (0.05, 0.3));
            input.for_each(|value| reference.process(value));

            for (measured, expected) in R::decibels(analyzer).iter().zip(reference.decibels()) {
                assert!(
                    (*measured as f64 - expected).abs() < R::TOLERANCE,
                    "{measured} {expected}"
                );
            }
        };
        let normalized = |values: &[i32]| -> Vec<f64> {
            values.iter().map(|value| *value as f64 / SCALE).collect()
        };

        let mut mixed = R::create(48_000, centres, 1.0);
        R::feed(&mut mixed, &stereo, 2);
        check(
            &mixed,
            &mut left
                .iter()
                .zip(&right)
                .map(|(left, right)| (*left as f64 + *right as f64) / 2.0 / SCALE),
        );
        assert!((R::decibels(&mixed)[5] as f64 - 20.0 * 0.25f64.log10()).abs() < 0.25);

        for (channel, signal) in [(0u8, &left), (1, &right)] {
            let mut selected = R::create(48_000, centres, 1.0).with_channel(channel);
            R::feed(&mut selected, &stereo, 2);
            check(&selected, &mut normalized(signal).into_iter());

            let mut tapped = R::create(48_000, mixed.centres(), 1.0).with_channel(channel);
            let mut output = std::vec![0i32; stereo.len()];
            samples_using::<R, i32>(&stereo, 2)
                .unwrap()
                .tap(&mut tapped)
                .write_samples(&mut output);
            for (output, input) in output.iter().zip(&stereo) {
                assert!((*output as i64 - *input as i64).abs() <= 256);
            }
            assert!(tapped == selected);
        }

        let mut missing = R::create(48_000, mixed.centres(), 1.0).with_channel(2);
        R::feed(&mut missing, &stereo, 2);
        assert_eq!(R::levels(&missing), [0.0; 10]);

        let mut three = R::create(48_000, mixed.centres(), 1.0);
        R::feed(&mut three, &triple, 3);
        let mut mono = R::create(48_000, mixed.centres(), 1.0);
        R::feed(&mut mono, &left, 1);
        for (three, mono) in R::decibels(&three).iter().zip(R::decibels(&mono)) {
            assert!(
                (three - mono).abs() < 1e-3 || mono < -100.0,
                "{three} {mono}"
            );
        }
        check(&mono, &mut normalized(&left).into_iter());

        let mut first = R::create(48_000, mixed.centres(), 1.0).with_channel(0);
        R::feed(&mut first, &left, 1);
        assert!(first.with_channel(0) == mono.with_channel(0));

        let before = mono;
        assert_eq!(analyze_using::<R, i32, _>(&[], 1, &mut mono), Ok(0));
        assert!(before == mono);

        R::reset(&mut mono);
        assert!(mono == R::create(48_000, mixed.centres(), 1.0));
        assert_eq!(R::levels(&mono), [0.0; 10]);
        assert_eq!(R::decibels(&mono), [f32::NEG_INFINITY; 10]);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn fixed_channels() {
        channels::<Fixed>();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn float_channels() {
        channels::<Float>();
    }

    fn ballistics<R: Tested>()
    where
        BandAnalyzer<R, 1>: Analyzer<R>,
    {
        let tone = sine(1000.0, 0.5, 48_000.0, 24_000);
        let silence = std::vec![0; 4800];
        let mut analyzer = R::ballistics(
            R::create(48_000, [1000.0], 1.0),
            Duration::from_millis(20),
            Duration::from_millis(100),
        );
        let mut reference = Reference::new(48_000.0, &[1000.0], 1.0, (0.02, 0.1));

        for signal in [&tone, &silence] {
            R::feed(&mut analyzer, signal, 1);

            for value in signal.iter() {
                reference.process(*value as f64 / SCALE);
            }

            let measured = R::decibels(&analyzer)[0] as f64;
            let expected = reference.decibels()[0];
            assert!(
                (measured - expected).abs() < R::TOLERANCE,
                "{measured} {expected}"
            );
        }

        let (integration, release) = (1024.0 / 48_000.0, 0.05f64);
        let time = 0.1f64;
        let expected = (release * (-time / release).exp()
            - integration * (-time / integration).exp())
            / (release - integration);
        let fallen = R::decibels(&analyzer)[0] as f64 - 20.0 * 0.5f64.log10();
        assert!((fallen - 10.0 * expected.log10()).abs() < 0.2, "{fallen}");

        let mut fast = R::ballistics(
            R::create(48_000, [1000.0], 1.0),
            Duration::ZERO,
            Duration::ZERO,
        );
        let mut slow = R::ballistics(fast, Duration::from_millis(200), Duration::ZERO);
        let short = &tone[..2400];
        R::feed(&mut fast, short, 1);
        R::feed(&mut slow, short, 1);
        assert!(R::decibels(&fast)[0] > -6.5);
        assert!(R::decibels(&slow)[0] < -9.0);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn fixed_ballistics() {
        ballistics::<Fixed>();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn float_ballistics() {
        ballistics::<Float>();
    }

    #[test]
    fn centres_are_placed_by_the_coefficients() {
        for sample_rate in [8_000u32, 48_000, 192_000] {
            for octaves in [1.0 / 3.0, 1.0] {
                let lowest = sample_rate as f64 / 13_000.0;

                for step in 0..40 {
                    let centre =
                        lowest * (0.4 * sample_rate as f64 / lowest).powf(step as f64 / 39.0);
                    let band = BandAnalyzer::<Fixed, 1>::new(sample_rate, [centre as f32], octaves)
                        .bands[0];
                    let centre =
                        BandAnalyzer::<Fixed, 1>::new(sample_rate, [centre as f32], octaves)
                            .centres()[0] as f64;
                    let ideal = coefficients(sample_rate as f64, centre, octaves as f64);
                    let quantized = Coefficients {
                        b0: band.b0 as f64 / SCALE * 2.0,
                        a1: -band.a1 as f64 / SCALE * 2.0,
                        a2: -band.a2 as f64 / SCALE * 2.0,
                        ..ideal
                    };
                    let rate = sample_rate as f64;
                    let slope = |coefficients: &Coefficients, frequency: f64| {
                        response(coefficients, frequency, rate)
                    };
                    let offset = 0.001 * centre;
                    let reference = slope(&ideal, centre + offset);

                    assert!(
                        (slope(&quantized, centre) - 1.0).abs() < 1e-4,
                        "{sample_rate} Hz, {centre} Hz"
                    );
                    assert!(
                        slope(&quantized, centre - offset) > reference * 0.999
                            && slope(&quantized, centre + offset) > reference * 0.999,
                        "{sample_rate} Hz, {centre} Hz"
                    );
                    assert_eq!(
                        band.shift,
                        ideal.shift.max(integration(DEFAULT_ATTACK, sample_rate))
                    );
                }
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn self_noise() {
        let sample_rate = 192_000.0;
        let mut analyzer = BandAnalyzer::<Fixed, 1>::new(192_000, [19.69], 1.0 / 3.0);
        let band = analyzer.bands[0];
        let (b0, a1, a2) = (
            band.b0 as f64 / SCALE * 2.0,
            band.a1 as f64 / SCALE * 2.0,
            band.a2 as f64 / SCALE * 2.0,
        );
        let mut random = Random(3);
        let mut history = [0.0f64; 4];
        let mut error = 0.0;
        let frames = 1 << 20;

        for index in 0..frames {
            let value = quantized(
                0.5 * (2.0 * PI * 19.69 * index as f64 / sample_rate).sin() + 0.25 * random.next(),
            );
            analyzer.analyze(&[value]);

            let input = (value >> HEADROOM) as f64;
            let [x1, x2, y1, y2] = history;
            let output = b0 * (input - x2) + a1 * y1 + a2 * y2;
            history = [input, x1, output, y1];

            let difference = (analyzer.bands[0].y1 as f64 - output) / (1u64 << 29) as f64;
            error += difference * difference;
        }

        let noise = 10.0 * (2.0 * error / frames as f64).log10();
        assert!(noise < -120.0, "{noise} dB");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn headroom() {
        let (sample_rate, centre, octaves) = (8_000u32, 528.4f32, 4.5f32);
        let mut analyzer = BandAnalyzer::<Fixed, 1>::new(sample_rate, [centre], octaves);
        let band = analyzer.bands[0];
        let (b0, a1, a2) = (
            band.b0 as f64 / SCALE * 2.0,
            band.a1 as f64 / SCALE * 2.0,
            band.a2 as f64 / SCALE * 2.0,
        );
        let length = 4096;
        let mut impulse = std::vec![0.0f64; length];
        let mut history = [0.0f64; 4];

        for (index, output) in impulse.iter_mut().enumerate() {
            let input = if index == 0 { 1.0 } else { 0.0 };
            let [x1, x2, y1, y2] = history;
            *output = b0 * (input - x2) + a1 * y1 + a2 * y2;
            history = [input, x1, *output, y1];
        }

        let norm: f64 = impulse.iter().map(|value| value.abs()).sum();
        assert!(norm > 1.8 && norm < 1.9, "{norm}");

        let worst: Vec<i32> = impulse
            .iter()
            .rev()
            .map(|value| if *value < 0.0 { i32::MIN } else { i32::MAX })
            .collect();
        let mut peak = 0i64;

        for _ in 0..64 {
            for value in &worst {
                analyzer.analyze(&[*value]);
                peak = peak.max((analyzer.bands[0].y1 as i64).abs());
            }
        }

        let bound = norm * (1u64 << 29) as f64;
        assert!(
            peak as f64 > 0.99 * bound && (peak as f64) < 1.001 * bound + 2.0,
            "{peak} {bound}"
        );

        let mut reference =
            Reference::new(sample_rate as f64, &[centre], octaves as f64, (0.05, 0.3));
        for _ in 0..64 {
            for value in &worst {
                reference.process(*value as f64 / SCALE);
            }
        }
        assert!((analyzer.decibels()[0] as f64 - reference.decibels()[0]).abs() < 0.01);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn long_runs() {
        let mut fixed = BandAnalyzer::<Fixed, 10>::octaves(48_000);
        let mut float = BandAnalyzer::<Float, 10>::octaves(48_000);
        let mut reference = Reference::new(48_000.0, &fixed.centres(), 1.0, (0.05, 0.3));
        let period = 48;

        for index in 0..(1 << 22) {
            let value = if index % period < period / 2 {
                i32::MAX
            } else {
                i32::MIN
            };
            fixed.analyze(&[value]);
            float.analyze(&[Float::from_sample(value)]);
            reference.process(value as f64 / SCALE);
        }

        let expected = reference.decibels();
        for (band, expected) in expected.iter().enumerate() {
            assert!((fixed.decibels()[band] as f64 - expected).abs() < 0.03);
            assert!((float.decibels()[band] as f64 - expected).abs() < 0.01);
        }

        assert!((fixed.decibels()[5] as f64 - 20.0 * (4.0 / PI).log10()).abs() < 0.3);
        assert_eq!(fixed.levels()[5], i32::MAX);
        assert!(float.levels()[5] > 1.2);
    }

    #[test]
    fn float_special_values() {
        let mut analyzer = BandAnalyzer::<Float, 3>::spread(48_000);

        for _ in 0..64 {
            analyzer.analyze(&[f32::NAN, f32::NAN]);
            analyzer.analyze(&[f32::NAN]);
        }
        assert_eq!(analyzer.levels(), [0.0; 3]);
        assert!(
            analyzer
                .bands
                .iter()
                .all(|band| band.ic1 == 0.0 && band.ic2 == 0.0)
        );

        analyzer.analyze(&[f32::INFINITY]);
        analyzer.analyze(&[f32::NEG_INFINITY, 0.0]);
        analyzer.analyze(&[0.0, f32::NAN]);
        for _ in 0..64 {
            analyzer.analyze(&[0.0]);
        }

        for decibels in analyzer.decibels() {
            assert!(decibels.is_finite() && decibels > 0.0, "{decibels}");
        }
        for level in analyzer.levels() {
            assert!(level.is_finite());
        }

        let mut selected = BandAnalyzer::<Float, 3>::spread(48_000).with_channel(1);
        for _ in 0..64 {
            selected.analyze(&[0.5, f32::NAN]);
            selected.analyze(&[]);
        }
        assert_eq!(selected.levels(), [0.0; 3]);
    }

    #[test]
    fn fixed_extremes() {
        let mut analyzer = BandAnalyzer::<Fixed, 3>::spread(48_000);

        for _ in 0..64 {
            analyzer.analyze(&[]);
            analyzer.analyze(&[0, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
        assert_eq!(analyzer.levels(), [0; 3]);

        for _ in 0..1000 {
            analyzer.analyze(&[i32::MIN; 8]);
            analyzer.analyze(&[i32::MIN, i32::MIN, i32::MIN]);
            analyzer.analyze(&[i32::MAX, i32::MAX, i32::MAX]);
        }

        assert!(
            analyzer
                .decibels()
                .iter()
                .all(|decibels| decibels.is_finite())
        );
    }

    #[test]
    fn wide_multiplies() {
        let mut random = Random(5);
        let mut values = std::vec![0, 1, -1, i32::MAX, i32::MIN, 0xFFFF, -0x10000, 0x7FFF_0000];

        for _ in 0..if cfg!(miri) { 24 } else { 1000 } {
            values.push((random.next() * SCALE) as i32);
        }

        for left in &values {
            for right in &values {
                assert_eq!(wide(*left, *right), *left as i64 * *right as i64);
            }
        }

        for difference in [
            0i64,
            1,
            -1,
            1 << 61,
            -(1 << 61),
            (1 << 62) - 1,
            12345678901234,
        ] {
            for coefficient in [0, 1, 1 << 30, i32::MAX] {
                let expected = (difference as i128 * coefficient as i128) >> 31;
                assert_eq!(scale(difference, coefficient) as i128, expected);
            }
        }

        for value in [0i64, 1, -1, i64::MAX, i64::MIN, 1 << 62, -(1 << 61) + 12345] {
            for shift in 0..=40 {
                assert_eq!(shift_right_split(value, shift), value >> shift);
                assert_eq!(shift_right(value, shift), value >> shift);
            }
        }

        for channels in 1..=MAXIMUM_CHANNELS {
            let frame = std::vec![i32::MIN; channels];
            let value = mix(&frame);
            assert!((value as i64 + (1 << 29)).abs() <= 2, "{channels}: {value}");
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(power_of_two(0), 1.0);
        assert_eq!(power_of_two(40), 1.0 / (1u64 << 40) as f32);
        assert_eq!(integration(Duration::ZERO, 48_000), 0);
        assert_eq!(integration(Duration::from_millis(50), 48_000), 11);
        assert_eq!(integration(Duration::from_secs(1 << 30), 48_000), 40);
        assert_eq!(fixed_ballistic(Duration::ZERO, 48_000), i32::MAX);
    }
}
