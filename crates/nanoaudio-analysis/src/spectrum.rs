use core::fmt::Debug;
use core::ops::Range;
use core::time::Duration;

use microfft::Complex32;
use nanoaudio_dsp::{Analyzer, Fixed, Float, Representation};

mod sealed {
    pub trait Sealed {}

    impl Sealed for nanoaudio_dsp::Fixed {}
    impl Sealed for nanoaudio_dsp::Float {}
    impl Sealed for super::Points<256> {}
    impl Sealed for super::Points<512> {}
    impl Sealed for super::Points<1024> {}
}

/// The size of a [`Spectrum`] transform, in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Points<const N: usize>;

/// A transform size a [`Spectrum`] supports: [`Points`] of 256, 512 or 1024.
pub trait Size: sealed::Sealed {
    #[doc(hidden)]
    type Bins<T: Copy + PartialEq + Debug>: Copy + PartialEq + Debug + AsRef<[T]> + AsMut<[T]>;

    #[doc(hidden)]
    fn bins<T: Copy + PartialEq + Debug>(value: T) -> Self::Bins<T>;

    #[doc(hidden)]
    fn transform(buffer: &mut [f32]) -> &mut [Complex32];
}

macro_rules! sizes {
    ($($points:literal => $transform:ident,)*) => {
        $(
            impl Size for Points<$points> {
                type Bins<T: Copy + PartialEq + Debug> = [T; $points / 2 + 1];

                #[inline]
                fn bins<T: Copy + PartialEq + Debug>(value: T) -> Self::Bins<T> {
                    [value; $points / 2 + 1]
                }

                #[inline]
                fn transform(buffer: &mut [f32]) -> &mut [Complex32] {
                    microfft::real::$transform(buffer.try_into().unwrap())
                }
            }
        )*
    };
}

sizes! {
    256 => rfft_256,
    512 => rfft_512,
    1024 => rfft_1024,
}

/// A representation a [`Spectrum`] can transform, [`Fixed`] or [`Float`].
///
/// [`Fixed`] uses an integer FFT with no floating point per frame or per block, and keeps the power of each bin in a `u64`. [`Float`] uses an `f32` FFT and keeps it in an `f32`.
pub trait Spectral:
    Representation<Value: Debug + PartialEq> + Debug + PartialEq + sealed::Sealed
{
    #[doc(hidden)]
    type Power: Copy + PartialEq + PartialOrd + Debug;
    #[doc(hidden)]
    type Weight: Copy + PartialEq + Debug;
    #[doc(hidden)]
    const ZERO: Self::Power;

    #[doc(hidden)]
    fn weight(weight: f64) -> Self::Weight;
}

impl Spectral for Fixed {
    type Power = u64;
    type Weight = u8;
    const ZERO: u64 = 0;

    fn weight(weight: f64) -> u8 {
        if weight.is_nan() || weight >= 1.0 {
            return 0;
        }

        let shift = math::round(-math::log2(weight));

        shift.clamp(0, 30) as u8
    }
}

impl Spectral for Float {
    type Power = f32;
    type Weight = f32;
    const ZERO: f32 = 0.0;

    fn weight(weight: f64) -> f32 {
        if weight < 1.0 { weight as f32 } else { 1.0 }
    }
}

/// The window a [`Spectrum`] applies to each block before its transform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Window {
    /// No window: the best resolution and exact readings at bin centres, but strong leakage from tones between bins.
    Rectangular,
    /// The Hann window: tones leak little beyond two bins on each side, at the cost of a wider peak.
    #[default]
    Hann,
}

/// Measures the spectrum of a signal with a windowed real FFT of `N` points, 256, 512 or 1024, with the `fft` feature.
///
/// It analyzes one selected channel or the mix of all channels, and transforms the last `N` frames once every hop, `N` frames by default. The result holds `N / 2 + 1` bins spaced `sample_rate / N` Hz apart, from 0 Hz to half the sample rate, and reads the amplitude of a sine at a bin centre, 0 dBFS for a full-scale one, for any window. Successive spectra can be averaged with an exponential time constant, and [`bars`](Self::bars) groups the bins into log-spaced bars for a display.
///
/// With [`Fixed`], the transform uses only integer arithmetic, scales by one half per stage so that it cannot overflow, and keeps about 29 bits, which puts its noise floor below -140 dBFS. With [`Float`], it uses the `microfft` crate, and a NaN or infinite sample makes every bin of its block NaN, and of every later block too with averaging, until [`reset`](Self::reset).
///
/// # Cost
///
/// The analyzer holds `N` values of input and `N / 2 + 1` bins, and the window and twiddle tables take 3 KiB of flash for every size. With a hop shorter than `N`, each transform also needs `N` values of stack. Each frame costs about 25 instructions, 200 in [`Float`] without an FPU; each block, measured in instructions on a Cortex-M4F and a Cortex-M0+:
///
/// | Points | RAM, [`Float`] | RAM, [`Fixed`] | M4F, [`Float`] | M4F, [`Fixed`] | M0+, [`Fixed`] | M0+, [`Float`] |
/// |---|---|---|---|---|---|---|
/// | 256 | 1.5 KiB | 2 KiB | 12 k | 24 k | 114 k | 650 k |
/// | 512 | 3 KiB | 4 KiB | 28 k | 49 k | 248 k | 1.5 M |
/// | 1024 | 6 KiB | 8 KiB | 60 k | 104 k | 540 k | 3.2 M |
///
/// [`Float`] is the faster choice on a core with a single-precision FPU, and [`Fixed`] on any other, where [`Float`] runs on software floating point.
///
/// ```
/// use nanoaudio_analysis::{Spectrum, analyze_using};
/// use nanoaudio_dsp::Float;
///
/// let tone: [f32; 1024] = core::array::from_fn(|index| {
///     0.5 * (2.0 * core::f32::consts::PI * 1500.0 * index as f32 / 48000.0).sin()
/// });
/// let mut spectrum = Spectrum::<Float, 1024>::new(48000);
///
/// analyze_using::<Float, _, _>(&tone, 1, &mut spectrum).unwrap();
///
/// let peak = spectrum.decibels().nth(32).unwrap();
/// assert!((peak + 6.02).abs() < 0.01);
/// assert_eq!(spectrum.frequency(32), 1500.0);
///
/// let bars = spectrum.bars::<10>(50.0, 16000.0);
/// let levels = spectrum.bar_decibels(&bars);
/// assert_eq!(levels.iter().copied().fold(f32::MIN, f32::max), peak);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum<R: Spectral, const N: usize>
where
    Points<N>: Size,
{
    samples: [R::Value; N],
    bins: <Points<N> as Size>::Bins<R::Power>,
    weight: R::Weight,
    averaging: Duration,
    sample_rate: u32,
    channels: u32,
    blocks: u64,
    position: u16,
    pending: u16,
    hop: u16,
    window: Window,
    channel: Option<u8>,
}

impl<R: Spectral, const N: usize> Spectrum<R, N>
where
    Points<N>: Size,
{
    /// The number of bins, from 0 Hz to half the sample rate.
    pub const BINS: usize = N / 2 + 1;

    /// Creates an analyzer for audio at `sample_rate`, with a Hann window, a hop of `N` frames and no averaging.
    ///
    /// It mixes all channels until [`with_channel`](Self::with_channel) selects one.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            samples: [R::SILENCE; N],
            bins: Points::<N>::bins(R::ZERO),
            weight: R::weight(1.0),
            averaging: Duration::ZERO,
            sample_rate: sample_rate.max(1),
            channels: 1,
            blocks: 0,
            position: 0,
            pending: N as u16,
            hop: N as u16,
            window: Window::Hann,
            channel: None,
        }
    }

    /// Analyzes only the channel at `channel`, counted from 0, instead of the mix of all channels.
    ///
    /// A frame without that channel counts as silence.
    pub const fn with_channel(mut self, channel: u8) -> Self {
        self.channel = Some(channel);
        self
    }

    /// Applies `window` to each block.
    pub const fn with_window(mut self, window: Window) -> Self {
        self.window = window;
        self
    }

    /// Transforms once every `frames` frames, clamped to 1 and `N`, so that successive blocks overlap.
    ///
    /// The first transform still waits for `N` frames.
    pub fn with_hop(mut self, frames: u16) -> Self {
        self.hop = frames.clamp(1, N as u16);
        self.pending = N as u16;
        self.refresh();
        self
    }

    /// Averages successive spectra with the time constant `time`, or not at all for [`Duration::ZERO`].
    ///
    /// Each spectrum is weighed against the average by `1 - e^(-hop / (sample_rate · time))`. In [`Fixed`], the weight is rounded to the nearest power of two.
    pub fn with_averaging(mut self, time: Duration) -> Self {
        self.averaging = time;
        self.refresh();
        self
    }

    fn refresh(&mut self) {
        let time = self.averaging.as_secs_f64() * self.sample_rate as f64;
        let weight = if time > 0.0 {
            1.0 - math::exp2(-(self.hop as f64) / time / core::f64::consts::LN_2)
        } else {
            1.0
        };

        self.weight = R::weight(weight);
    }

    /// Returns the sample rate, in Hz.
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the number of frames between two transforms.
    pub const fn hop(&self) -> u16 {
        self.hop
    }

    /// Returns the window applied to each block.
    pub const fn window(&self) -> Window {
        self.window
    }

    /// Returns the number of spectra completed since the start or the last [`reset`](Self::reset).
    ///
    /// The results change only when it does, so it tells when to read them again.
    pub const fn blocks(&self) -> u64 {
        self.blocks
    }

    /// Returns the centre frequency of `bin`, in Hz.
    pub fn frequency(&self, bin: usize) -> f32 {
        (bin as f64 * self.sample_rate as f64 / N as f64) as f32
    }

    /// Forgets every frame analyzed, as if newly created.
    pub fn reset(&mut self) {
        self.samples = [R::SILENCE; N];
        self.bins = Points::<N>::bins(R::ZERO);
        self.channels = 1;
        self.blocks = 0;
        self.position = 0;
        self.pending = N as u16;
    }

    /// Groups the bins into `B` bars spaced logarithmically from `lowest` to `highest`, in Hz, for a spectrum display.
    ///
    /// Both are clamped to half the bin spacing and half the sample rate, so that no bar reads the 0 Hz bin. A bar covers the bins whose centre lies within it, or the bin nearest its centre when it is narrower than a bin, so that no bar is empty.
    pub fn bars<const B: usize>(&self, lowest: f32, highest: f32) -> Bars<B> {
        let resolution = self.sample_rate as f64 / N as f64;
        let nyquist = self.sample_rate as f64 / 2.0;
        let lowest = (lowest as f64).clamp(resolution / 2.0, nyquist);
        let highest = (highest as f64).clamp(lowest, nyquist);
        let octaves = math::log2(highest / lowest);
        let last = N / 2;
        let boundary = |index: usize| lowest * math::exp2(octaves * index as f64 / B.max(1) as f64);

        let ranges = core::array::from_fn(|index| {
            let (low, high) = (
                boundary(index) / resolution,
                boundary(index + 1) / resolution,
            );
            let start = math::ceiling(low - 1e-9);
            let end = if index + 1 == B {
                math::floor(high + 1e-9) + 1
            } else {
                math::ceiling(high - 1e-9)
            };
            let (start, end) = if end > start {
                (start, end)
            } else {
                let centre = math::round(math::square_root(low * high)) as usize;

                (centre, centre + 1)
            };

            [start.min(last) as u16, end.min(last + 1) as u16]
        });

        Bars { ranges }
    }

    #[inline(always)]
    fn push(&mut self, value: R::Value, channels: usize) -> bool {
        let position = self.position as usize & (N - 1);

        self.samples[position] = value;
        self.position = ((position + 1) & (N - 1)) as u16;
        self.pending -= 1;

        if self.pending == 0 {
            self.pending = self.hop;
            self.channels = channels.clamp(1, u32::MAX as usize) as u32;
            true
        } else {
            false
        }
    }

    fn bar_powers<const B: usize>(&self, bars: &Bars<B>) -> [(R::Power, bool); B] {
        let bins = self.bins.as_ref();

        bars.ranges.map(|[start, end]| {
            let (start, end) = (start as usize, end as usize);
            let mut best = (bins[start], edge(start, N));

            for (bin, power) in bins.iter().enumerate().take(end).skip(start + 1) {
                if *power > best.0 || power.partial_cmp(power).is_none() {
                    best = (*power, edge(bin, N));
                }
            }

            best
        })
    }
}

const fn edge(bin: usize, points: usize) -> bool {
    bin == 0 || bin == points / 2
}

/// Log-spaced groups of [`Spectrum`] bins, one per bar of a spectrum display, made by [`Spectrum::bars`].
///
/// A bar reads its loudest bin, so that a tone reads its level whatever the width of the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bars<const B: usize> {
    ranges: [[u16; 2]; B],
}

impl<const B: usize> Bars<B> {
    /// Returns the range of bins of each bar, from the lowest frequency.
    pub fn ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.ranges
            .iter()
            .map(|[start, end]| *start as usize..*end as usize)
    }
}

impl<const N: usize> Spectrum<Fixed, N>
where
    Points<N>: Size,
{
    fn shift(&self) -> u32 {
        match self.channel {
            Some(_) => 0,
            None => mix_shift(self.channels as usize),
        }
    }

    fn scale(&self, edge: bool) -> u32 {
        let base = match self.window {
            Window::Rectangular => 3,
            Window::Hann => 4,
        };

        base - edge as u32 + self.shift()
    }

    fn mixed(&self) -> u32 {
        match self.channel {
            Some(_) => 1,
            None => self.channels,
        }
    }

    fn magnitude(&self, power: u64, edge: bool) -> i32 {
        let root = math::root(power);
        let scale = self.scale(edge);
        let mixed = self.mixed();
        let (quotient, remainder) = (root / mixed, root % mixed);
        let magnitude = ((quotient as u64) << scale)
            + ((((remainder as u64) << scale) as u32 + mixed / 2) / mixed) as u64;

        magnitude.min(i32::MAX as u64) as i32
    }

    fn decibel(&self, power: u64, edge: bool) -> f32 {
        let scale =
            2.0 * self.scale(edge) as f32 - 2.0 * math::log2_f32(self.mixed() as f32) - 62.0;

        DECIBELS_PER_OCTAVE * (math::log2_f32(power as f32) + scale)
    }

    /// Returns the amplitude of each bin in the last spectrum, `i32::MAX` for a full-scale sine at its centre.
    pub fn magnitudes(&self) -> impl Iterator<Item = i32> + '_ {
        self.bins
            .as_ref()
            .iter()
            .enumerate()
            .map(|(bin, power)| self.magnitude(*power, edge(bin, N)))
    }

    /// Returns the level of each bin in the last spectrum in dBFS, 0 for a full-scale sine at its centre and negative infinity for silence.
    pub fn decibels(&self) -> impl Iterator<Item = f32> + '_ {
        self.bins
            .as_ref()
            .iter()
            .enumerate()
            .map(|(bin, power)| self.decibel(*power, edge(bin, N)))
    }

    /// Returns the amplitude of each bar in the last spectrum, as [`magnitudes`](Self::magnitudes) does for bins.
    pub fn bar_magnitudes<const B: usize>(&self, bars: &Bars<B>) -> [i32; B] {
        self.bar_powers(bars)
            .map(|(power, edge)| self.magnitude(power, edge))
    }

    /// Returns the level of each bar in the last spectrum in dBFS, as [`decibels`](Self::decibels) does for bins.
    pub fn bar_decibels<const B: usize>(&self, bars: &Bars<B>) -> [f32; B] {
        self.bar_powers(bars)
            .map(|(power, edge)| self.decibel(power, edge))
    }

    #[inline(never)]
    fn complete(&mut self) {
        if self.hop as usize == N {
            window_fixed(&mut self.samples, self.window);
            fixed::transform(&mut self.samples);
            store_fixed(
                self.bins.as_mut(),
                &self.samples,
                self.weight,
                self.blocks == 0,
            );
        } else {
            let mut scratch = [0; N];
            rotate(&mut scratch, &self.samples, self.position as usize);
            window_fixed(&mut scratch, self.window);
            fixed::transform(&mut scratch);
            store_fixed(self.bins.as_mut(), &scratch, self.weight, self.blocks == 0);
        }

        self.blocks += 1;
    }
}

impl<const N: usize> Spectrum<Float, N>
where
    Points<N>: Size,
{
    fn magnitude(power: f32) -> f32 {
        math::square_root_f32(power)
    }

    fn decibel(power: f32) -> f32 {
        DECIBELS_PER_OCTAVE * math::log2_f32(power)
    }

    /// Returns the amplitude of each bin in the last spectrum, 1.0 for a full-scale sine at its centre.
    pub fn magnitudes(&self) -> impl Iterator<Item = f32> + '_ {
        self.bins
            .as_ref()
            .iter()
            .map(|power| Self::magnitude(*power))
    }

    /// Returns the level of each bin in the last spectrum in dBFS, 0 for a full-scale sine at its centre and negative infinity for silence.
    pub fn decibels(&self) -> impl Iterator<Item = f32> + '_ {
        self.bins.as_ref().iter().map(|power| Self::decibel(*power))
    }

    /// Returns the amplitude of each bar in the last spectrum, as [`magnitudes`](Self::magnitudes) does for bins.
    pub fn bar_magnitudes<const B: usize>(&self, bars: &Bars<B>) -> [f32; B] {
        self.bar_powers(bars)
            .map(|(power, _)| Self::magnitude(power))
    }

    /// Returns the level of each bar in the last spectrum in dBFS, as [`decibels`](Self::decibels) does for bins.
    pub fn bar_decibels<const B: usize>(&self, bars: &Bars<B>) -> [f32; B] {
        self.bar_powers(bars).map(|(power, _)| Self::decibel(power))
    }

    fn normalization(&self) -> f32 {
        let gain = match self.window {
            Window::Rectangular => 1.0,
            Window::Hann => 0.5 * ONE,
        };
        let channels = match self.channel {
            Some(_) => 1.0,
            None => self.channels as f32,
        };
        let divisor = N as f32 * gain * channels;

        4.0 / (divisor * divisor)
    }

    #[inline(never)]
    fn complete(&mut self) {
        let scale = self.normalization();
        let first = self.blocks == 0;

        if self.hop as usize == N {
            window_float(&mut self.samples, self.window);
            let spectrum = Points::<N>::transform(&mut self.samples);
            store_float(self.bins.as_mut(), spectrum, scale, self.weight, first);
        } else {
            let mut scratch = [0.0; N];
            rotate(&mut scratch, &self.samples, self.position as usize);
            window_float(&mut scratch, self.window);
            let spectrum = Points::<N>::transform(&mut scratch);
            store_float(self.bins.as_mut(), spectrum, scale, self.weight, first);
        }

        self.blocks += 1;
    }
}

impl<const N: usize> Analyzer<Fixed> for Spectrum<Fixed, N>
where
    Points<N>: Size,
{
    #[inline]
    fn analyze(&mut self, frame: &[i32]) {
        let (value, channels) = match self.channel {
            Some(channel) => (frame.get(channel as usize).copied().unwrap_or(0), 1),
            None => (mix(frame), frame.len()),
        };

        if self.push(value, channels) {
            self.complete();
        }
    }
}

impl<const N: usize> Analyzer<Float> for Spectrum<Float, N>
where
    Points<N>: Size,
{
    #[inline]
    fn analyze(&mut self, frame: &[f32]) {
        let (value, channels) = match self.channel {
            Some(channel) => (frame.get(channel as usize).copied().unwrap_or(0.0), 1),
            None => (frame.iter().sum(), frame.len()),
        };

        if self.push(value, channels) {
            self.complete();
        }
    }
}

const DECIBELS_PER_OCTAVE: f32 = 3.010_3;

const ONE: f32 = (1u32 << 30) as f32;

const TABLE: usize = 1024;

static WINDOW: [i32; TABLE / 2 + 1] = {
    let mut table = [0; TABLE / 2 + 1];
    let mut index = 0;

    while index <= TABLE / 2 {
        let cosine = math::cosine(2.0 * core::f64::consts::PI * index as f64 / TABLE as f64);
        table[index] = math::round((1.0 - cosine) / 2.0 * (1u32 << 30) as f64) as i32;
        index += 1;
    }

    table
};

static SINE: [i32; TABLE / 4 + 1] = {
    let mut table = [0; TABLE / 4 + 1];
    let mut index = 0;

    while index <= TABLE / 4 {
        let cosine =
            math::cosine(2.0 * core::f64::consts::PI * (TABLE / 4 - index) as f64 / TABLE as f64);
        let value = math::round(cosine * (1u64 << 31) as f64);
        table[index] = if value > i32::MAX as i64 {
            i32::MAX
        } else {
            value as i32
        };
        index += 1;
    }

    table
};

#[inline]
fn windowed<T>(buffer: &mut [T], mut apply: impl FnMut(&mut T, i32)) {
    let points = buffer.len();
    let stride = TABLE / points;
    let (head, tail) = buffer.split_at_mut(points / 2);

    for (value, weight) in head.iter_mut().zip(WINDOW.iter().step_by(stride)) {
        apply(value, *weight);
    }

    for (value, weight) in tail.iter_mut().zip(WINDOW.iter().rev().step_by(stride)) {
        apply(value, *weight);
    }
}

fn window_fixed(buffer: &mut [i32], window: Window) {
    match window {
        Window::Rectangular => buffer.iter_mut().for_each(|value| *value >>= 2),
        Window::Hann => windowed(buffer, |value, weight| {
            *value = ((fixed::wide(*value, weight) + (1 << 31)) >> 32) as i32;
        }),
    }
}

fn window_float(buffer: &mut [f32], window: Window) {
    if window == Window::Hann {
        windowed(buffer, |value, weight| *value *= weight as f32);
    }
}

fn rotate<T: Copy>(output: &mut [T], ring: &[T], position: usize) {
    let (older, newer) = ring.split_at(position);
    let (head, tail) = output.split_at_mut(newer.len());

    head.copy_from_slice(newer);
    tail.copy_from_slice(older);
}

fn store_fixed(bins: &mut [u64], spectrum: &[i32], shift: u8, first: bool) {
    let points = spectrum.len();
    let power = |real: i32, imaginary: i32| {
        fixed::wide(real, real) as u64 + fixed::wide(imaginary, imaginary) as u64
    };
    let update = |bin: &mut u64, power: u64| {
        *bin = if first {
            power
        } else {
            (*bin as i64 + ((power as i64 - *bin as i64) >> shift)) as u64
        };
    };

    update(&mut bins[0], power(spectrum[0], 0));
    update(&mut bins[points / 2], power(spectrum[1], 0));

    for (bin, pair) in bins[1..points / 2]
        .iter_mut()
        .zip(spectrum[2..].as_chunks::<2>().0)
    {
        update(bin, power(pair[0], pair[1]));
    }
}

fn store_float(bins: &mut [f32], spectrum: &[Complex32], scale: f32, weight: f32, first: bool) {
    let points = spectrum.len() * 2;
    let update = |bin: &mut f32, power: f32| {
        *bin = if first || weight >= 1.0 {
            power
        } else {
            *bin + (power - *bin) * weight
        };
    };
    let edge = scale * 0.25;

    update(&mut bins[0], spectrum[0].re * spectrum[0].re * edge);
    update(
        &mut bins[points / 2],
        spectrum[0].im * spectrum[0].im * edge,
    );

    for (bin, value) in bins[1..points / 2].iter_mut().zip(&spectrum[1..]) {
        update(bin, (value.re * value.re + value.im * value.im) * scale);
    }
}

const MIX_SHIFTS: [u8; 9] = [0, 0, 1, 2, 2, 3, 3, 3, 3];

#[inline]
fn mix_shift(channels: usize) -> u32 {
    match MIX_SHIFTS.get(channels) {
        Some(shift) => *shift as u32,
        None => usize::BITS - (channels - 1).leading_zeros(),
    }
}

#[inline]
fn mix(frame: &[i32]) -> i32 {
    let shift = mix_shift(frame.len());

    frame
        .iter()
        .fold(0i32, |sum, value| sum.wrapping_add(value >> shift))
}

mod fixed {
    use super::{SINE, TABLE};

    #[cfg(not(all(target_arch = "arm", not(target_has_atomic = "32"))))]
    #[inline(always)]
    pub fn wide(left: i32, right: i32) -> i64 {
        left as i64 * right as i64
    }

    #[cfg(any(test, all(target_arch = "arm", not(target_has_atomic = "32"))))]
    #[inline(always)]
    pub fn wide_split(left: i32, right: i32) -> i64 {
        let (unsigned_left, unsigned_right) = (left as u32, right as u32);
        let (left_low, left_high) = (unsigned_left & 0xFFFF, unsigned_left >> 16);
        let (right_low, right_high) = (unsigned_right & 0xFFFF, unsigned_right >> 16);
        let low = left_low * right_low;
        let cross_left = left_high * right_low;
        let cross_right = left_low * right_high;
        let middle = (low >> 16) + (cross_left & 0xFFFF) + (cross_right & 0xFFFF);
        let low = (low & 0xFFFF) | (middle << 16);
        let mut high =
            left_high * right_high + (cross_left >> 16) + (cross_right >> 16) + (middle >> 16);

        if left < 0 {
            high = high.wrapping_sub(unsigned_right);
        }

        if right < 0 {
            high = high.wrapping_sub(unsigned_left);
        }

        (((high as u64) << 32) | low as u64) as i64
    }

    #[cfg(all(target_arch = "arm", not(target_has_atomic = "32")))]
    #[inline(always)]
    pub fn wide(left: i32, right: i32) -> i64 {
        wide_split(left, right)
    }

    #[inline(always)]
    fn high(product: i64) -> i32 {
        (product >> 32) as i32
    }

    #[inline(always)]
    fn butterfly(first: &mut [i32; 2], second: &mut [i32; 2], cosine: i32, sine: i32) {
        let [other_real, other_imaginary] = *second;
        let real = high(wide(other_real, cosine) + wide(other_imaginary, sine));
        let imaginary = high(wide(other_imaginary, cosine) + wide(other_real, -sine));
        let [first_real, first_imaginary] = first.map(|value| value >> 1);

        *first = [first_real + real, first_imaginary + imaginary];
        *second = [first_real - real, first_imaginary - imaginary];
    }

    fn reverse(data: &mut [[i32; 2]]) {
        let points = data.len();
        let mut target = 0;

        for index in 0..points {
            if index < target {
                data.swap(index, target);
            }

            let mut bit = points >> 1;

            while target & bit != 0 {
                target ^= bit;
                bit >>= 1;
            }

            target |= bit;
        }
    }

    pub fn transform(data: &mut [i32]) {
        let (data, _) = data.as_chunks_mut::<2>();
        let points = data.len();
        let mut half = 1;

        reverse(data);

        while half < points {
            let stride = TABLE / (2 * half);

            for group in data.chunks_exact_mut(2 * half) {
                let (first, second) = group.split_at_mut(half);
                let [real, imaginary] = first[0].map(|value| value >> 1);
                let [other_real, other_imaginary] = second[0].map(|value| value >> 1);

                first[0] = [real + other_real, imaginary + other_imaginary];
                second[0] = [real - other_real, imaginary - other_imaginary];
            }

            for offset in 1..half {
                let index = offset * stride;
                let (cosine, sine) = if index <= TABLE / 4 {
                    (SINE[TABLE / 4 - index], SINE[index])
                } else {
                    (-SINE[index - TABLE / 4], SINE[TABLE / 2 - index])
                };

                for group in data.chunks_exact_mut(2 * half) {
                    let (first, second) = group.split_at_mut(half);

                    if let (Some(first), Some(second)) =
                        (first.get_mut(offset), second.get_mut(offset))
                    {
                        butterfly(first, second, cosine, sine);
                    }
                }
            }

            half *= 2;
        }

        recombine(data);
    }

    fn recombine(data: &mut [[i32; 2]]) {
        let quarter = data.len() / 2;
        let stride = TABLE / (2 * data.len());
        let [real, imaginary] = data[0].map(|value| value >> 1);

        data[0] = [real + imaginary, real - imaginary];

        let (low, high_half) = data.split_at_mut(quarter);
        let [real, imaginary] = high_half[0].map(|value| value >> 1);

        high_half[0] = [real, -imaginary];

        let cosines = SINE.iter().rev().step_by(stride).skip(1);
        let sines = SINE.iter().step_by(stride).skip(1);

        for ((first, second), (cosine, sine)) in low[1..]
            .iter_mut()
            .zip(high_half[1..].iter_mut().rev())
            .zip(cosines.zip(sines))
        {
            let [real, imaginary] = first.map(|value| value >> 1);
            let [other_real, other_imaginary] = second.map(|value| value >> 1);
            let even_real = (real + other_real) >> 1;
            let even_imaginary = (imaginary - other_imaginary) >> 1;
            let odd_real = real - other_real;
            let odd_imaginary = imaginary + other_imaginary;
            let rotated_real = high(wide(*cosine, odd_imaginary) + wide(-*sine, odd_real));
            let rotated_imaginary = high(wide(-*cosine, odd_real) + wide(-*sine, odd_imaginary));

            *first = [even_real + rotated_real, even_imaginary + rotated_imaginary];
            *second = [even_real - rotated_real, rotated_imaginary - even_imaginary];
        }
    }
}

mod math {
    pub fn root(value: u64) -> u32 {
        let mut remainder = value;
        let mut root = 0u64;
        let mut bit = 1u64 << 62;

        while bit > remainder {
            bit >>= 2;
        }

        while bit != 0 {
            if remainder >= root + bit {
                remainder -= root + bit;
                root = (root >> 1) + bit;
            } else {
                root >>= 1;
            }

            bit >>= 2;
        }

        root as u32
    }

    pub const fn round(value: f64) -> i64 {
        if value >= 0.0 {
            (value + 0.5) as i64
        } else {
            (value - 0.5) as i64
        }
    }

    pub fn floor(value: f64) -> usize {
        if value <= 0.0 { 0 } else { value as usize }
    }

    pub fn ceiling(value: f64) -> usize {
        let floor = floor(value);

        if (floor as f64) < value {
            floor + 1
        } else {
            floor
        }
    }

    pub const fn cosine(angle: f64) -> f64 {
        let half = core::f64::consts::FRAC_PI_2;

        if angle > half {
            return -cosine(core::f64::consts::PI - angle);
        }

        let square = angle * angle;
        let mut term = 1.0;
        let mut sum = 1.0;
        let mut index = 1;

        while index <= 14 {
            term = -term * square / ((2 * index - 1) * (2 * index)) as f64;
            sum += term;
            index += 1;
        }

        sum
    }

    pub fn square_root(value: f64) -> f64 {
        if value.is_nan() || value <= 0.0 || value == f64::INFINITY {
            return if value < 0.0 { f64::NAN } else { value };
        }

        if value < f64::MIN_POSITIVE {
            return square_root(value * (1u128 << 64) as f64) / (1u64 << 32) as f64;
        }

        let mut root = f64::from_bits((value.to_bits() >> 1) + 0x1FF8_0000_0000_0000);

        for _ in 0..6 {
            root = 0.5 * (root + value / root);
        }

        root
    }

    pub fn exp2(value: f64) -> f64 {
        if value < -1000.0 {
            return 0.0;
        }

        let whole = value as i64 - (value < (value as i64) as f64) as i64;
        let fraction = (value - whole as f64) * core::f64::consts::LN_2;
        let mut term = 1.0;
        let mut sum = 1.0;
        let mut index = 1;

        while index <= 18 {
            term *= fraction / index as f64;
            sum += term;
            index += 1;
        }

        let mut scale = 1.0;
        let mut exponent = whole;

        while exponent > 0 {
            scale *= 2.0;
            exponent -= 1;
        }

        while exponent < 0 {
            scale *= 0.5;
            exponent += 1;
        }

        sum * scale
    }

    pub fn log2(value: f64) -> f64 {
        if value.is_nan() || value <= 0.0 {
            return if value == 0.0 {
                f64::NEG_INFINITY
            } else {
                f64::NAN
            };
        }

        if value == f64::INFINITY {
            return value;
        }

        let (value, offset) = if value < f64::MIN_POSITIVE {
            (value * (1u128 << 64) as f64, -64.0)
        } else {
            (value, 0.0)
        };
        let bits = value.to_bits();
        let mut exponent = ((bits >> 52) & 0x7FF) as i64 - 1023;
        let mut mantissa = f64::from_bits((bits & ((1 << 52) - 1)) | (1023 << 52));

        if mantissa > core::f64::consts::SQRT_2 {
            mantissa /= 2.0;
            exponent += 1;
        }

        let ratio = (mantissa - 1.0) / (mantissa + 1.0);
        let square = ratio * ratio;
        let mut term = ratio;
        let mut sum = 0.0;
        let mut index = 0;

        while index < 10 {
            sum += term / (2 * index + 1) as f64;
            term *= square;
            index += 1;
        }

        exponent as f64 + offset + 2.0 * sum / core::f64::consts::LN_2
    }

    pub fn log2_f32(value: f32) -> f32 {
        if value.is_nan() || value <= 0.0 {
            return if value == 0.0 {
                f32::NEG_INFINITY
            } else {
                f32::NAN
            };
        }

        if value == f32::INFINITY {
            return value;
        }

        let (value, offset) = if value < f32::MIN_POSITIVE {
            (value * (1u64 << 32) as f32, -32.0)
        } else {
            (value, 0.0)
        };
        let bits = value.to_bits();
        let mut exponent = ((bits >> 23) & 0xFF) as i32 - 127;
        let mut mantissa = f32::from_bits((bits & ((1 << 23) - 1)) | (127 << 23));

        if mantissa > core::f32::consts::SQRT_2 {
            mantissa *= 0.5;
            exponent += 1;
        }

        let ratio = (mantissa - 1.0) / (mantissa + 1.0);
        let square = ratio * ratio;
        let series =
            ratio * (1.0 + square * (1.0 / 3.0 + square * (1.0 / 5.0 + square * (1.0 / 7.0))));

        exponent as f32 + offset + series * (2.0 / core::f32::consts::LN_2)
    }

    pub fn square_root_f32(value: f32) -> f32 {
        if value.is_nan() || value <= 0.0 || value == f32::INFINITY {
            return if value < 0.0 { f32::NAN } else { value };
        }

        let (value, scale) = if value < f32::MIN_POSITIVE {
            (value * (1u64 << 32) as f32, 1.0 / (1u32 << 16) as f32)
        } else {
            (value, 1.0)
        };
        let mut root = f32::from_bits((value.to_bits() >> 1) + 0x1FC0_0000);

        for _ in 0..4 {
            root = 0.5 * (root + value / root);
        }

        root * scale
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::f64::consts::PI;
    use std::vec::Vec;

    use nanoaudio_dsp::{Stream, analyze_using, samples_using};

    use super::*;

    const TOLERANCE: f64 = 2e-7;

    const FULL: f64 = 2_147_483_648.0;

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

    fn hann(index: usize, points: usize) -> f64 {
        let sine = (PI * index as f64 / points as f64).sin();

        sine * sine
    }

    fn reference(signal: &[f64], window: Window) -> Vec<f64> {
        let points = signal.len();
        let gain = match window {
            Window::Rectangular => 1.0,
            Window::Hann => 0.5,
        };

        (0..=points / 2)
            .map(|bin| {
                let (mut real, mut imaginary) = (0.0, 0.0);

                for (index, value) in signal.iter().enumerate() {
                    let weight = match window {
                        Window::Rectangular => 1.0,
                        Window::Hann => hann(index, points),
                    };
                    let angle = 2.0 * PI * ((bin * index) % points) as f64 / points as f64;

                    real += weight * value * angle.cos();
                    imaginary -= weight * value * angle.sin();
                }

                let scale = if bin == 0 || bin == points / 2 {
                    1.0
                } else {
                    2.0
                };

                scale * (real * real + imaginary * imaginary).sqrt() / (points as f64 * gain)
            })
            .collect()
    }

    fn tones(length: usize, sample_rate: f64, tones: &[(f64, f64)], noise: f64) -> Vec<i16> {
        let mut random = Random(length as u64 + 11);

        (0..length)
            .map(|index| {
                let time = index as f64 / sample_rate;
                let value: f64 = tones
                    .iter()
                    .map(|(frequency, amplitude)| {
                        amplitude * (2.0 * PI * frequency * time + 0.3).sin()
                    })
                    .sum::<f64>()
                    + noise * random.next();

                (value * 32768.0).round().clamp(-32768.0, 32767.0) as i16
            })
            .collect()
    }

    fn normalized(samples: &[i16]) -> Vec<f64> {
        samples
            .iter()
            .map(|sample| *sample as f64 / 32768.0)
            .collect()
    }

    trait Read: Spectral {
        fn magnitudes<const N: usize>(spectrum: &Spectrum<Self, N>) -> Vec<f64>
        where
            Points<N>: Size;

        fn decibels<const N: usize>(spectrum: &Spectrum<Self, N>) -> Vec<f32>
        where
            Points<N>: Size;

        fn bars<const N: usize, const B: usize>(
            spectrum: &Spectrum<Self, N>,
            bars: &Bars<B>,
        ) -> ([f64; B], [f32; B])
        where
            Points<N>: Size;
    }

    impl Read for Fixed {
        fn magnitudes<const N: usize>(spectrum: &Spectrum<Self, N>) -> Vec<f64>
        where
            Points<N>: Size,
        {
            spectrum
                .magnitudes()
                .map(|value| value as f64 / FULL)
                .collect()
        }

        fn decibels<const N: usize>(spectrum: &Spectrum<Self, N>) -> Vec<f32>
        where
            Points<N>: Size,
        {
            spectrum.decibels().collect()
        }

        fn bars<const N: usize, const B: usize>(
            spectrum: &Spectrum<Self, N>,
            bars: &Bars<B>,
        ) -> ([f64; B], [f32; B])
        where
            Points<N>: Size,
        {
            (
                spectrum
                    .bar_magnitudes(bars)
                    .map(|value| value as f64 / FULL),
                spectrum.bar_decibels(bars),
            )
        }
    }

    impl Read for Float {
        fn magnitudes<const N: usize>(spectrum: &Spectrum<Self, N>) -> Vec<f64>
        where
            Points<N>: Size,
        {
            spectrum.magnitudes().map(|value| value as f64).collect()
        }

        fn decibels<const N: usize>(spectrum: &Spectrum<Self, N>) -> Vec<f32>
        where
            Points<N>: Size,
        {
            spectrum.decibels().collect()
        }

        fn bars<const N: usize, const B: usize>(
            spectrum: &Spectrum<Self, N>,
            bars: &Bars<B>,
        ) -> ([f64; B], [f32; B])
        where
            Points<N>: Size,
        {
            (
                spectrum.bar_magnitudes(bars).map(|value| value as f64),
                spectrum.bar_decibels(bars),
            )
        }
    }

    fn check<R: Read, const N: usize>(spectrum: &Spectrum<R, N>, expected: &[f64])
    where
        Points<N>: Size,
    {
        let magnitudes = R::magnitudes(spectrum);
        let decibels = R::decibels(spectrum);

        assert_eq!(magnitudes.len(), Spectrum::<R, N>::BINS);
        assert_eq!(expected.len(), Spectrum::<R, N>::BINS);

        for bin in 0..expected.len() {
            assert!(
                (magnitudes[bin] - expected[bin]).abs() <= TOLERANCE,
                "bin {bin}: {} instead of {}",
                magnitudes[bin],
                expected[bin]
            );

            if expected[bin] > 1e-3 {
                let level = 20.0 * expected[bin].log10();

                assert!(
                    (decibels[bin] as f64 - level).abs()
                        <= 20.0 * (1.0 + TOLERANCE / expected[bin]).log10() + 1e-4,
                    "bin {bin}: {} dB instead of {level}",
                    decibels[bin]
                );
            }
        }
    }

    fn matches_the_reference<R: Read, const N: usize>()
    where
        Points<N>: Size,
        Spectrum<R, N>: Analyzer<R>,
    {
        let rate = 48000.0;
        let bin = rate / N as f64;
        let frequencies = [32.0 * bin, 32.37 * bin, 47.0, 0.49 * rate];
        let frequencies = if cfg!(miri) {
            &frequencies[..1]
        } else {
            &frequencies[..]
        };

        for frequency in frequencies.iter().copied() {
            for window in [Window::Hann, Window::Rectangular] {
                let samples = tones(
                    N,
                    rate,
                    &[(frequency, 0.5), (frequency * 0.37 + 300.0, 0.01)],
                    1e-4,
                );
                let mut spectrum = Spectrum::<R, N>::new(48000).with_window(window);

                assert_eq!(spectrum.window(), window);
                assert_eq!(analyze_using::<R, _, _>(&samples, 1, &mut spectrum), Ok(N));
                assert_eq!(spectrum.blocks(), 1);
                check(&spectrum, &reference(&normalized(&samples), window));
            }
        }

        let spectrum = Spectrum::<R, N>::new(48000);

        assert_eq!(spectrum.frequency(0), 0.0);
        assert_eq!(spectrum.frequency(N / 2), 24000.0);
        assert_eq!(spectrum.frequency(1), (48000.0 / N as f64) as f32);
        assert_eq!(spectrum.sample_rate(), 48000);
        assert_eq!(spectrum.hop(), N as u16);
        assert_eq!(spectrum.window(), Window::Hann);
    }

    fn sine(points: usize, bin: f64, amplitude: f64) -> Vec<i32> {
        (0..points)
            .map(|index| {
                let value = amplitude * (2.0 * PI * bin * index as f64 / points as f64 + 0.7).sin();

                (value * FULL).round().clamp(-FULL, FULL - 1.0) as i32
            })
            .collect()
    }

    fn levels<R: Read>(samples: &[i32], window: Window) -> Vec<f32>
    where
        Spectrum<R, 1024>: Analyzer<R>,
    {
        let mut spectrum = Spectrum::<R, 1024>::new(48000).with_window(window);

        analyze_using::<R, _, _>(samples, 1, &mut spectrum).unwrap();
        R::decibels(&spectrum)
    }

    fn leaks_as_expected<R: Read>()
    where
        Spectrum<R, 1024>: Analyzer<R>,
    {
        let amplitude: f64 = 0.8;
        let level = 20.0 * amplitude.log10();
        let decibels = |value: f64| 20.0 * value.abs().log10();
        let sinc = |offset: f64| (PI * offset).sin() / (PI * offset);

        let centred = levels::<R>(&sine(1024, 256.0, amplitude), Window::Hann);

        assert!((centred[256] as f64 - level).abs() < 1e-4);
        assert!((centred[255] as f64 - level - decibels(0.5)).abs() < 1e-3);
        assert!((centred[257] as f64 - level - decibels(0.5)).abs() < 1e-3);
        assert!(
            centred
                .iter()
                .enumerate()
                .filter(|(bin, _)| bin.abs_diff(256) >= 2)
                .all(|(_, value)| *value < -125.0)
        );

        let between = levels::<R>(&sine(1024, 256.5, amplitude), Window::Hann);

        for offset in [0.5, 1.5, 2.5, 5.5, 20.5] {
            let expected = level + decibels(sinc(offset) / (1.0 - offset * offset));

            for bin in [256.5 - offset, 256.5 + offset] {
                assert!(
                    (between[bin as usize] as f64 - expected).abs() < 0.01,
                    "Hann {offset}: {} instead of {expected}",
                    between[bin as usize]
                );
            }
        }

        assert!((between[256] as f64 - level + 1.4236).abs() < 1e-3);

        let between = levels::<R>(&sine(1024, 256.5, amplitude), Window::Rectangular);

        for offset in [0.5, 1.5] {
            let expected = level + decibels(sinc(offset));

            for bin in [256.5 - offset, 256.5 + offset] {
                assert!(
                    (between[bin as usize] as f64 - expected).abs() < 0.05,
                    "rectangular {offset}: {} instead of {expected}",
                    between[bin as usize]
                );
            }
        }

        assert!(between[236] as f64 > level - 40.0);

        let centred = levels::<R>(&sine(1024, 100.0, amplitude), Window::Rectangular);

        assert!((centred[100] as f64 - level).abs() < 1e-4);
        assert!(
            centred
                .iter()
                .enumerate()
                .filter(|(bin, _)| *bin != 100)
                .all(|(_, value)| *value < -125.0)
        );
    }

    fn averages<R: Read>()
    where
        Spectrum<R, 256>: Analyzer<R> + PartialEq,
    {
        let time = Duration::from_secs_f64(256.0 / 48000.0 / core::f64::consts::LN_2);
        let mut spectrum = Spectrum::<R, 256>::new(48000).with_averaging(time);
        let first = tones(256, 48000.0, &[(1875.0, 0.5)], 0.0);
        let second = tones(256, 48000.0, &[(3750.0, 0.25), (1875.0, 0.1)], 0.0);
        let silence = [0i16; 256];
        let power = |samples: &[i16]| -> Vec<f64> {
            reference(&normalized(samples), Window::Hann)
                .iter()
                .map(|value| value * value)
                .collect()
        };
        let (first_power, second_power) = (power(&first), power(&second));

        analyze_using::<R, _, _>(&first, 1, &mut spectrum).unwrap();
        check(&spectrum, &reference(&normalized(&first), Window::Hann));

        analyze_using::<R, _, _>(&second, 1, &mut spectrum).unwrap();
        let averaged: Vec<f64> = first_power
            .iter()
            .zip(&second_power)
            .map(|(first, second)| first + 0.5 * (second - first))
            .collect();
        check(
            &spectrum,
            &averaged
                .iter()
                .map(|value| value.sqrt())
                .collect::<Vec<_>>(),
        );

        analyze_using::<R, _, _>(&silence, 1, &mut spectrum).unwrap();
        check(
            &spectrum,
            &averaged
                .iter()
                .map(|value| (value / 2.0).sqrt())
                .collect::<Vec<_>>(),
        );

        let mut plain = Spectrum::<R, 256>::new(48000).with_averaging(Duration::ZERO);

        analyze_using::<R, _, _>(&first, 1, &mut plain).unwrap();
        analyze_using::<R, _, _>(&second, 1, &mut plain).unwrap();
        check(&plain, &reference(&normalized(&second), Window::Hann));

        let ordered = Spectrum::<R, 256>::new(48000)
            .with_hop(128)
            .with_averaging(Duration::from_millis(100));
        let reversed = Spectrum::<R, 256>::new(48000)
            .with_averaging(Duration::from_millis(100))
            .with_hop(128);

        assert!(ordered == reversed);
    }

    #[test]
    fn weights() {
        let weight = |time: f64, hop: f64| 1.0 - (-hop / (48000.0 * time)).exp();

        assert_eq!(Fixed::weight(1.0), 0);
        assert_eq!(Fixed::weight(f64::NAN), 0);
        assert_eq!(Fixed::weight(0.5), 1);
        assert_eq!(Fixed::weight(0.3), 2);
        assert_eq!(Fixed::weight(0.0), 30);
        assert_eq!(Float::weight(1.0), 1.0);
        assert_eq!(Float::weight(2.0), 1.0);

        for (time, hop) in [(0.1, 1024u16), (0.5, 256), (2.0, 512), (0.01, 1024)] {
            let spectrum = Spectrum::<Float, 1024>::new(48000)
                .with_hop(hop)
                .with_averaging(Duration::from_secs_f64(time));
            let expected = weight(time, hop as f64);

            assert!((spectrum.weight as f64 - expected).abs() < 1e-6 * expected);

            let spectrum = Spectrum::<Fixed, 1024>::new(48000)
                .with_hop(hop)
                .with_averaging(Duration::from_secs_f64(time));

            assert_eq!(spectrum.weight as i64, (-expected.log2()).round() as i64);
        }
    }

    fn groups_bars<R: Read>()
    where
        Spectrum<R, 1024>: Analyzer<R>,
    {
        let mut spectrum = Spectrum::<R, 1024>::new(48000);
        let resolution = 48000.0 / 1024.0;
        let bars = spectrum.bars::<16>(30.0, 20000.0);
        let ranges: Vec<_> = bars.ranges().collect();

        assert_eq!(ranges.len(), 16);
        assert_eq!(ranges[0].start, 1);
        assert_eq!(ranges[15].end, (20000.0 / resolution) as usize + 1);

        let mut covered = ranges[0].start;

        for (index, range) in ranges.iter().enumerate() {
            let ratio = 20000.0f64 / 30.0;
            let low = 30.0 * ratio.powf(index as f64 / 16.0);
            let high = 30.0 * ratio.powf((index + 1) as f64 / 16.0);

            assert!(range.start < range.end);

            if (high - low) / resolution > 1.0 {
                for bin in range.clone() {
                    let centre = bin as f64 * resolution;
                    assert!(centre >= low - 1e-6 && centre <= high + 1e-6);
                }
            } else {
                assert_eq!(range.len(), 1);
                let centre = (low * high).sqrt() / resolution;
                assert!((range.start as f64 - centre).abs() <= 0.5 + 1e-9);
            }

            assert!(range.start <= covered);
            covered = covered.max(range.end);
        }

        let samples = tones(1024, 48000.0, &[(1000.0, 0.5), (8000.0, 0.05)], 1e-3);

        analyze_using::<R, _, _>(&samples, 1, &mut spectrum).unwrap();

        let magnitudes = R::magnitudes(&spectrum);
        let decibels = R::decibels(&spectrum);
        let (bar_magnitudes, bar_decibels) = R::bars(&spectrum, &bars);

        for (index, range) in ranges.iter().enumerate() {
            let loudest = range
                .clone()
                .max_by(|left, right| magnitudes[*left].total_cmp(&magnitudes[*right]))
                .unwrap();

            assert_eq!(bar_magnitudes[index], magnitudes[loudest]);
            assert_eq!(bar_decibels[index], decibels[loudest]);
        }

        let tone = ranges
            .iter()
            .position(|range| range.contains(&((1000.0 / resolution) as usize)))
            .unwrap();
        let peak = decibels.iter().copied().fold(f32::NEG_INFINITY, f32::max);

        assert_eq!(bar_decibels[tone], peak);

        assert_eq!(
            spectrum.bars::<2>(1.0, 24.0),
            spectrum.bars::<2>(23.4375, 24.0)
        );
        assert_eq!(spectrum.bars::<2>(1.0, 24.0).ranges().next(), Some(1..2));

        let single = spectrum.bars::<1>(0.0, 1e9);
        assert_eq!(single.ranges().next(), Some(1..513));

        let inverted = spectrum.bars::<4>(5000.0, 100.0);
        assert!(inverted.ranges().all(|range| range == (107..108)));

        let none = spectrum.bars::<0>(20.0, 20000.0);
        assert_eq!(none.ranges().count(), 0);
        assert_eq!(R::bars(&spectrum, &none).0.len(), 0);
    }

    fn selects_or_mixes_channels<R: Read>()
    where
        Spectrum<R, 512>: Analyzer<R> + PartialEq,
    {
        let frames = 512;
        let left = tones(frames, 48000.0, &[(1000.0, 0.6)], 1e-4);
        let right = tones(frames, 48000.0, &[(2345.6, 0.4), (1000.0, 0.2)], 1e-4);
        let interleaved: Vec<i16> = left
            .iter()
            .zip(&right)
            .flat_map(|(left, right)| [*left, *right])
            .collect();
        let mut output = std::vec![0i16; interleaved.len()];

        for channel in [None, Some(0), Some(1)] {
            let mut tapped = Spectrum::<R, 512>::new(48000);
            let mut sliced = Spectrum::<R, 512>::new(48000);

            if let Some(channel) = channel {
                tapped = tapped.with_channel(channel);
                sliced = sliced.with_channel(channel);
            }

            let written = samples_using::<R, _>(&interleaved, 2)
                .unwrap()
                .tap(&mut tapped)
                .write_samples(&mut output);

            assert_eq!(written, frames);
            assert_eq!(output, interleaved);
            assert_eq!(
                analyze_using::<R, _, _>(&interleaved, 2, &mut sliced),
                Ok(frames)
            );
            assert!(tapped == sliced);

            let signal: Vec<f64> = match channel {
                None => normalized(&left)
                    .iter()
                    .zip(normalized(&right))
                    .map(|(left, right)| (left + right) / 2.0)
                    .collect(),
                Some(0) => normalized(&left),
                _ => normalized(&right),
            };

            check(&tapped, &reference(&signal, Window::Hann));
        }

        let mut missing = Spectrum::<R, 512>::new(48000).with_channel(2);

        analyze_using::<R, _, _>(&interleaved, 2, &mut missing).unwrap();
        assert_eq!(missing.blocks(), 1);
        assert!(R::magnitudes(&missing).iter().all(|value| *value == 0.0));

        let third = tones(frames, 48000.0, &[(5000.0, 0.3)], 0.0);
        let triple: Vec<i16> = (0..frames)
            .flat_map(|index| [left[index], right[index], third[index]])
            .collect();
        let mut mixed = Spectrum::<R, 512>::new(48000);

        analyze_using::<R, _, _>(&triple, 3, &mut mixed).unwrap();

        let signal: Vec<f64> = (0..frames)
            .map(|index| {
                (left[index] as f64 + right[index] as f64 + third[index] as f64) / 3.0 / 32768.0
            })
            .collect();

        check(&mixed, &reference(&signal, Window::Hann));
    }

    fn overlaps<R: Read>()
    where
        Spectrum<R, 256>: Analyzer<R> + PartialEq,
    {
        let samples = tones(
            700,
            48000.0,
            &[(3000.0, 0.5), (11000.0, 0.05), (440.0, 0.2)],
            1e-3,
        );
        let mut spectrum = Spectrum::<R, 256>::new(48000).with_hop(64);

        assert_eq!(spectrum.hop(), 64);

        for chunk in samples[..458].chunks(37) {
            analyze_using::<R, _, _>(chunk, 1, &mut spectrum).unwrap();
        }

        assert_eq!(spectrum.blocks(), 4);
        check(
            &spectrum,
            &reference(&normalized(&samples[192..448]), Window::Hann),
        );

        analyze_using::<R, _, _>(&samples[458..], 1, &mut spectrum).unwrap();
        assert_eq!(spectrum.blocks(), 4 + (700 - 448) / 64);
        check(
            &spectrum,
            &reference(&normalized(&samples[384..640]), Window::Hann),
        );

        let mut single = Spectrum::<R, 256>::new(48000).with_hop(0);

        assert_eq!(single.hop(), 1);
        analyze_using::<R, _, _>(&samples[..300], 1, &mut single).unwrap();
        assert_eq!(single.blocks(), 45);
        check(
            &single,
            &reference(&normalized(&samples[44..300]), Window::Hann),
        );
        assert_eq!(Spectrum::<R, 256>::new(48000).with_hop(u16::MAX).hop(), 256);
    }

    fn reports_the_last_complete_block<R: Read>()
    where
        Spectrum<R, 256>: Analyzer<R> + PartialEq,
    {
        let mut spectrum = Spectrum::<R, 256>::new(48000);

        assert_eq!(
            analyze_using::<R, _, _>(&[0i16; 0], 1, &mut spectrum),
            Ok(0)
        );
        assert_eq!(spectrum.blocks(), 0);
        assert!(R::magnitudes(&spectrum).iter().all(|value| *value == 0.0));
        assert!(
            R::decibels(&spectrum)
                .iter()
                .all(|value| *value == f32::NEG_INFINITY)
        );

        let loud = tones(256, 48000.0, &[(3000.0, 0.5)], 0.0);
        let quiet = tones(200, 48000.0, &[(5000.0, 0.05)], 0.0);

        analyze_using::<R, _, _>(&loud, 1, &mut spectrum).unwrap();
        analyze_using::<R, _, _>(&quiet, 1, &mut spectrum).unwrap();
        assert_eq!(spectrum.blocks(), 1);
        check(&spectrum, &reference(&normalized(&loud), Window::Hann));

        spectrum.reset();
        assert!(spectrum == Spectrum::<R, 256>::new(48000));

        let mut changed = Spectrum::<R, 256>::new(0)
            .with_channel(1)
            .with_window(Window::Rectangular)
            .with_hop(100);

        analyze_using::<R, _, _>(&loud, 2, &mut changed).unwrap();
        changed.reset();
        assert!(
            changed
                == Spectrum::<R, 256>::new(1)
                    .with_channel(1)
                    .with_window(Window::Rectangular)
                    .with_hop(100)
        );
    }

    fn runs_for_long<R: Read>()
    where
        Spectrum<R, 256>: Analyzer<R>,
    {
        let blocks = if cfg!(miri) { 3 } else { 3000 };
        let samples = tones(256, 48000.0, &[(1875.0, 0.9), (375.0, 0.05)], 0.0);
        let mut spectrum = Spectrum::<R, 256>::new(48000).with_averaging(Duration::from_millis(50));

        for _ in 0..blocks {
            analyze_using::<R, _, _>(&samples, 1, &mut spectrum).unwrap();
        }

        assert_eq!(spectrum.blocks(), blocks);
        check(&spectrum, &reference(&normalized(&samples), Window::Hann));
    }

    #[test]
    fn fixed() {
        matches_the_reference::<Fixed, 256>();

        if !cfg!(miri) {
            matches_the_reference::<Fixed, 512>();
            matches_the_reference::<Fixed, 1024>();
            leaks_as_expected::<Fixed>();
            groups_bars::<Fixed>();
        }

        averages::<Fixed>();
        selects_or_mixes_channels::<Fixed>();
        overlaps::<Fixed>();
        reports_the_last_complete_block::<Fixed>();
        runs_for_long::<Fixed>();
    }

    #[test]
    fn float() {
        matches_the_reference::<Float, 256>();

        if !cfg!(miri) {
            matches_the_reference::<Float, 512>();
            matches_the_reference::<Float, 1024>();
            leaks_as_expected::<Float>();
            groups_bars::<Float>();
        }

        averages::<Float>();
        selects_or_mixes_channels::<Float>();
        overlaps::<Float>();
        reports_the_last_complete_block::<Float>();
        runs_for_long::<Float>();
    }

    #[test]
    fn fixed_does_not_overflow() {
        let mut random = Random(5);
        let extremes: Vec<i32> = (0..256)
            .map(|_| {
                if random.next() < 0.0 {
                    i32::MIN
                } else {
                    i32::MAX
                }
            })
            .collect();
        let alternating: Vec<i32> = (0..256)
            .map(|index| if index % 2 == 0 { i32::MIN } else { i32::MAX })
            .collect();
        let constant = [i32::MIN; 256];

        for samples in [&extremes[..], &alternating, &constant] {
            for window in [Window::Hann, Window::Rectangular] {
                let mut spectrum = Spectrum::<Fixed, 256>::new(48000).with_window(window);

                analyze_using::<Fixed, _, _>(samples, 1, &mut spectrum).unwrap();

                let signal: Vec<f64> = samples.iter().map(|value| *value as f64 / FULL).collect();
                let expected: Vec<f64> = reference(&signal, window)
                    .iter()
                    .map(|value| value.min(i32::MAX as f64 / FULL))
                    .collect();

                check(&spectrum, &expected);
            }
        }

        let mut stereo = Spectrum::<Fixed, 256>::new(48000);
        let pairs: Vec<i32> = constant.iter().flat_map(|value| [*value, *value]).collect();

        analyze_using::<Fixed, _, _>(&pairs, 2, &mut stereo).unwrap();
        assert_eq!(stereo.magnitudes().next(), Some(i32::MAX));
        assert!(stereo.decibels().next().unwrap().abs() < 1e-4);
    }

    #[test]
    fn transforms_match_the_reference() {
        let mut random = Random(9);
        let points = if cfg!(miri) { 64 } else { 1024 };
        let input: Vec<f64> = (0..points).map(|_| random.next()).collect();
        let mut data: Vec<i32> = input
            .iter()
            .map(|value| (value * (1 << 29) as f64).round() as i32)
            .collect();
        let reals: Vec<f64> = data.iter().map(|value| *value as f64).collect();

        fixed::transform(&mut data);

        let mut worst: f64 = 0.0;

        for bin in 0..points / 2 {
            let (mut real, mut imaginary) = (0.0, 0.0);

            for (index, value) in reals.iter().enumerate() {
                let angle = 2.0 * PI * ((bin * index) % points) as f64 / points as f64;
                real += value * angle.cos();
                imaginary -= value * angle.sin();
            }

            let (real, imaginary) = (real / points as f64, imaginary / points as f64);
            let (actual_real, actual_imaginary) = if bin == 0 {
                (data[0] as f64, 0.0)
            } else {
                (data[2 * bin] as f64, data[2 * bin + 1] as f64)
            };

            worst = worst
                .max((actual_real - real).abs())
                .max((actual_imaginary - imaginary).abs());
        }

        assert!(worst <= 8.0, "{worst}");
    }

    #[test]
    fn float_propagates_invalid_samples() {
        let mut spectrum = Spectrum::<Float, 256>::new(48000);
        let bars = spectrum.bars::<4>(100.0, 10000.0);
        let mut samples = [0.25f32; 256];

        samples[100] = f32::NAN;
        analyze_using::<Float, _, _>(&samples, 1, &mut spectrum).unwrap();
        assert!(spectrum.magnitudes().all(|value| value.is_nan()));
        assert!(spectrum.decibels().all(|value| value.is_nan()));
        assert!(
            spectrum
                .bar_decibels(&bars)
                .iter()
                .all(|value| value.is_nan())
        );
        assert!(
            spectrum
                .bar_magnitudes(&bars)
                .iter()
                .all(|value| value.is_nan())
        );

        analyze_using::<Float, _, _>(&[0.25f32; 256], 1, &mut spectrum).unwrap();
        assert!((spectrum.magnitudes().next().unwrap() - 0.25).abs() < 1e-6);

        samples[100] = f32::INFINITY;
        analyze_using::<Float, _, _>(&samples, 1, &mut spectrum).unwrap();
        assert!(spectrum.magnitudes().all(|value| !value.is_finite()));

        let mut averaged =
            Spectrum::<Float, 256>::new(48000).with_averaging(Duration::from_millis(10));

        samples[100] = f32::NAN;
        analyze_using::<Float, _, _>(&samples, 1, &mut averaged).unwrap();
        analyze_using::<Float, _, _>(&[0.25f32; 256], 1, &mut averaged).unwrap();
        assert!(averaged.magnitudes().all(|value| value.is_nan()));

        averaged.reset();
        analyze_using::<Float, _, _>(&[0.25f32; 256], 1, &mut averaged).unwrap();
        assert!((averaged.magnitudes().next().unwrap() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn split_multiply_is_exact() {
        let mut random = Random(1);
        let edges = [
            0,
            1,
            -1,
            i32::MAX,
            i32::MIN,
            1 << 30,
            -(1 << 30),
            0xFFFF,
            -0x10000,
        ];
        let count = if cfg!(miri) { 1000 } else { 1_000_000 };

        for left in edges {
            for right in edges {
                assert_eq!(fixed::wide_split(left, right), left as i64 * right as i64);
            }
        }

        for _ in 0..count {
            let left = (random.next() * FULL) as i32;
            let right = (random.next() * FULL) as i32;

            assert_eq!(fixed::wide_split(left, right), left as i64 * right as i64);
        }
    }

    #[test]
    fn tables_and_math_match_the_standard_library() {
        for (index, value) in WINDOW.iter().enumerate() {
            let expected = hann(index, TABLE) * (1u32 << 30) as f64;
            assert!((*value as f64 - expected).abs() <= 0.5 + 1e-6);
        }

        for (index, value) in SINE.iter().enumerate() {
            let expected =
                ((2.0 * PI * index as f64 / TABLE as f64).sin() * FULL).min(i32::MAX as f64);
            assert!((*value as f64 - expected).abs() <= 0.5 + 1e-6);
        }

        for value in [
            1e-30f64, 1e-5, 0.1, 0.5, 1.0, 1.5, 2.0, 3.7, 1e3, 1e30, 5e-310,
        ] {
            assert!((math::log2(value) - value.log2()).abs() < 1e-12 * value.log2().abs().max(1.0));
            assert!(
                (math::square_root(value) - value.sqrt()).abs() <= 1e-15 * value.sqrt(),
                "{value}: {}",
                math::square_root(value)
            );
        }

        for value in [-30.0f64, -1.5, -0.2, 0.0, 0.3, 1.0, 7.9, 40.0] {
            assert!((math::exp2(value) - value.exp2()).abs() <= 1e-14 * value.exp2());
        }

        for value in [
            1e-40f32, 1e-30, 1e-5, 0.1, 0.5, 1.0, 1.42, 1.5, 2.0, 3.7, 1e3, 1e30,
        ] {
            assert!(
                (math::log2_f32(value) - value.log2()).abs() < 2e-6 * value.log2().abs().max(1.0)
            );
            assert!((math::square_root_f32(value) - value.sqrt()).abs() <= 2e-7 * value.sqrt());
        }

        let mut random = Random(3);

        for value in [
            0u64,
            1,
            2,
            3,
            4,
            15,
            16,
            17,
            u32::MAX as u64,
            1 << 61,
            (1 << 62) - 1,
            u64::MAX,
        ] {
            assert_eq!(math::root(value) as u64, value.isqrt());
        }

        for _ in 0..if cfg!(miri) { 100 } else { 100_000 } {
            let value = ((random.next() + 1.0) * (1u64 << 62) as f64) as u64
                >> (random.next().abs() * 62.0) as u32;
            assert_eq!(math::root(value) as u64, value.isqrt());
        }

        assert_eq!(math::log2_f32(0.0), f32::NEG_INFINITY);
        assert_eq!(math::log2_f32(f32::INFINITY), f32::INFINITY);
        assert!(math::log2_f32(-1.0).is_nan());
        assert!(math::log2_f32(f32::NAN).is_nan());
        assert_eq!(math::square_root_f32(0.0), 0.0);
        assert_eq!(math::square_root_f32(f32::INFINITY), f32::INFINITY);
        assert!(math::square_root_f32(-1.0).is_nan());
        assert!(math::square_root_f32(f32::NAN).is_nan());
        assert_eq!(math::log2(0.0), f64::NEG_INFINITY);
        assert!(math::log2(-1.0).is_nan());
        assert_eq!(math::exp2(-2000.0), 0.0);
        assert_eq!(math::ceiling(2.0), 2);
        assert_eq!(math::ceiling(2.1), 3);
        assert_eq!(math::floor(-3.0), 0);
    }
}
