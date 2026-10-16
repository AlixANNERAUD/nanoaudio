use nanoaudio_dsp::{Analyzer, Fixed, Float, Representation};

use crate::math;

/// Measures the level of a few chosen frequencies with the Goertzel algorithm, a cheap alternative to an FFT for tone detection.
///
/// It analyzes blocks of a fixed number of frames, one selected channel or the mix of all channels, and reports the frequencies of the last completed block. Each frequency is measured at its exact value, not rounded to a DFT bin, and a full-scale sine at that frequency reads full scale, 0 dBFS, when the block holds a whole number of its periods. Off-centre tones and other frequencies leak in as they would in a rectangular-window DFT.
///
/// With [`Fixed`], each frame costs one 32 × 32-bit multiply per frequency and no floating point. The input is shifted right so that the filter state stays in `i32` for any input, which keeps about `31 - log2(min(frames² / 2, frames / sin(2π · frequency / sample_rate)))` bits of it: 22 bits for DTMF at 8 kHz, 16 bits for 1 kHz over 4096 frames at 48 kHz, and none near 0 Hz over the largest blocks. Mixing loses one more bit per doubling of the channel count. With [`Float`], the state is `f32`, and a NaN or infinite sample makes the results of its block NaN.
///
/// ```
/// use nanoaudio_analysis::{Goertzel, analyze_using};
/// use nanoaudio_dsp::Fixed;
///
/// let tone: [i16; 400] = core::array::from_fn(|index| [0, 10000, 0, -10000][index % 4]);
/// let mut goertzel = Goertzel::<Fixed, 2>::new(8000, 200, [2000.0, 1000.0]);
///
/// analyze_using::<Fixed, _, _>(&tone, 1, &mut goertzel).unwrap();
///
/// let [high, low] = goertzel.decibels();
/// assert!((high - 20.0 * (10000.0f32 / 32768.0).log10()).abs() < 0.01);
/// assert!(low < -100.0);
/// assert_eq!(goertzel.blocks(), 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Goertzel<R: Representation, const F: usize> {
    coefficients: [R::Value; F],
    shifts: [u8; F],
    previous: [R::Value; F],
    before: [R::Value; F],
    last_previous: [R::Value; F],
    last_before: [R::Value; F],
    frames: u16,
    position: u16,
    channel: Option<u8>,
    channels: u8,
    blocks: u64,
}

impl<R: Representation, const F: usize> Goertzel<R, F> {
    /// The largest block, in frames.
    pub const MAXIMUM_FRAMES: u16 = 32768;

    const fn with(coefficients: [R::Value; F], shifts: [u8; F], frames: u16) -> Self {
        Self {
            coefficients,
            shifts,
            previous: [R::SILENCE; F],
            before: [R::SILENCE; F],
            last_previous: [R::SILENCE; F],
            last_before: [R::SILENCE; F],
            frames,
            position: 0,
            channel: None,
            channels: 1,
            blocks: 0,
        }
    }

    /// Analyzes only the channel at `channel`, counted from 0, instead of the mix of all channels.
    ///
    /// A frame without that channel counts as silence.
    pub const fn with_channel(mut self, channel: u8) -> Self {
        self.channel = Some(channel);
        self
    }

    /// Returns the length of a block, in frames.
    pub const fn frames(&self) -> u16 {
        self.frames
    }

    /// Returns the number of blocks completed since the start or the last [`reset`](Self::reset).
    ///
    /// The results change only when it does, so it tells when to read them again.
    pub const fn blocks(&self) -> u64 {
        self.blocks
    }

    /// Forgets every frame analyzed, as if newly created.
    pub fn reset(&mut self) {
        *self = Self::with(self.coefficients, self.shifts, self.frames).channel(self.channel);
    }

    const fn channel(mut self, channel: Option<u8>) -> Self {
        self.channel = channel;
        self
    }

    #[inline]
    fn advance(&mut self, channels: usize) {
        self.position += 1;

        if self.position == self.frames {
            for index in 0..F {
                self.last_previous[index] =
                    core::mem::replace(&mut self.previous[index], R::SILENCE);
                self.last_before[index] = core::mem::replace(&mut self.before[index], R::SILENCE);
            }
            self.position = 0;
            self.channels = channels.max(1) as u8;
            self.blocks += 1;
        }
    }

    const fn mixed(&self) -> u32 {
        match self.channel {
            Some(_) => 1,
            None => self.channels as u32,
        }
    }
}

const fn clamp_frames(frames: u16) -> u16 {
    if frames == 0 {
        1
    } else if frames > Goertzel::<Fixed, 0>::MAXIMUM_FRAMES {
        Goertzel::<Fixed, 0>::MAXIMUM_FRAMES
    } else {
        frames
    }
}

const fn cosine_of(sample_rate: u32, frequency: f32) -> f64 {
    let nyquist = sample_rate as f64 / 2.0;
    let frequency = frequency as f64;
    let frequency = if frequency > nyquist {
        nyquist
    } else if frequency >= 0.0 {
        frequency
    } else {
        0.0
    };

    if nyquist == 0.0 {
        return 1.0;
    }

    math::cosine(core::f64::consts::PI * frequency / nyquist)
}

const ONE: f64 = (1u64 << 31) as f64;

impl<const F: usize> Goertzel<Fixed, F> {
    /// Creates an analyzer of `frequencies`, in Hz, over blocks of `frames` frames at `sample_rate`.
    ///
    /// Frequencies are clamped to 0 Hz and `sample_rate / 2`, and `frames` to 1 and [`MAXIMUM_FRAMES`](Self::MAXIMUM_FRAMES). It mixes all channels until [`with_channel`](Self::with_channel) selects one. Floating point is used here only, never per frame.
    pub const fn new(sample_rate: u32, frames: u16, frequencies: [f32; F]) -> Self {
        let frames = clamp_frames(frames);
        let mut coefficients = [0; F];
        let mut shifts = [0; F];
        let mut index = 0;

        while index < F {
            let coefficient = math::round(cosine_of(sample_rate, frequencies[index]) * ONE);
            let coefficient = if coefficient >= i32::MAX as i64 {
                i32::MAX
            } else if coefficient <= i32::MIN as i64 {
                i32::MIN
            } else {
                coefficient as i32
            };

            coefficients[index] = coefficient;
            shifts[index] = headroom(coefficient, frames);
            index += 1;
        }

        Self::with(coefficients, shifts, frames)
    }

    fn power(&self, index: usize) -> u128 {
        let previous = self.last_previous[index] as i128;
        let before = self.last_before[index] as i128;
        let coefficient = self.coefficients[index] as i128;

        (((previous * previous + before * before) << 30) - coefficient * previous * before) as u128
    }

    fn scale(&self, index: usize) -> u32 {
        self.shifts[index] as u32 + mix_shift(self.mixed() as usize)
    }

    /// Returns the amplitude of each frequency in the last block, `i32::MAX` for a full-scale sine.
    pub fn magnitudes(&self) -> [i32; F] {
        let divisor = ((self.frames as u128) * self.mixed() as u128) << 15;

        core::array::from_fn(|index| {
            let root = (self.power(index).isqrt()) << (self.scale(index) + 1);
            let magnitude = (root + divisor / 2) / divisor;

            if magnitude > i32::MAX as u128 {
                i32::MAX
            } else {
                magnitude as i32
            }
        })
    }

    /// Returns the squared amplitude of each frequency in the last block, `i32::MAX` for a full-scale sine.
    pub fn powers(&self) -> [i32; F] {
        self.magnitudes()
            .map(|magnitude| ((magnitude as i64 * magnitude as i64) >> 31) as i32)
    }

    /// Returns the level of each frequency in the last block in dBFS, 0 for a full-scale sine and negative infinity for silence.
    pub fn decibels(&self) -> [f32; F] {
        let divisor = math::log2((self.frames as u32 * self.mixed()) as f64);

        core::array::from_fn(|index| {
            let power = math::log2(self.power(index) as f64);
            let scale = self.scale(index) as f64 - 45.0;

            (math::DECIBELS_PER_OCTAVE * (power + 2.0 * (scale - divisor))) as f32
        })
    }
}

impl<const F: usize> Goertzel<Float, F> {
    /// Creates an analyzer of `frequencies`, in Hz, over blocks of `frames` frames at `sample_rate`.
    ///
    /// Frequencies are clamped to 0 Hz and `sample_rate / 2`, and `frames` to 1 and [`MAXIMUM_FRAMES`](Self::MAXIMUM_FRAMES). It mixes all channels until [`with_channel`](Self::with_channel) selects one.
    pub const fn new(sample_rate: u32, frames: u16, frequencies: [f32; F]) -> Self {
        let frames = clamp_frames(frames);
        let mut coefficients = [0.0; F];
        let mut index = 0;

        while index < F {
            coefficients[index] = (2.0 * cosine_of(sample_rate, frequencies[index])) as f32;
            index += 1;
        }

        Self::with(coefficients, [0; F], frames)
    }

    fn power(&self, index: usize) -> f64 {
        let previous = self.last_previous[index] as f64;
        let before = self.last_before[index] as f64;
        let coefficient = self.coefficients[index] as f64;

        previous * previous + before * before - coefficient * previous * before
    }

    fn divisor(&self) -> f64 {
        (self.frames as u32 * self.mixed()) as f64
    }

    /// Returns the amplitude of each frequency in the last block, 1.0 for a full-scale sine.
    pub fn magnitudes(&self) -> [f32; F] {
        let divisor = self.divisor();

        core::array::from_fn(|index| {
            (2.0 * libm::sqrt(positive(self.power(index))) / divisor) as f32
        })
    }

    /// Returns the squared amplitude of each frequency in the last block, 1.0 for a full-scale sine.
    pub fn powers(&self) -> [f32; F] {
        let divisor = self.divisor();

        core::array::from_fn(|index| {
            (4.0 * positive(self.power(index)) / (divisor * divisor)) as f32
        })
    }

    /// Returns the level of each frequency in the last block in dBFS, 0 for a full-scale sine and negative infinity for silence.
    pub fn decibels(&self) -> [f32; F] {
        let divisor = math::log2(self.divisor());

        core::array::from_fn(|index| {
            let power = math::log2(positive(self.power(index)));

            (math::DECIBELS_PER_OCTAVE * (power + 2.0 - 2.0 * divisor)) as f32
        })
    }
}

fn positive(power: f64) -> f64 {
    if power < 0.0 { 0.0 } else { power }
}

const fn headroom(coefficient: i32, frames: u16) -> u8 {
    let frames = frames as f64;
    let cosine = coefficient as f64 / ONE;
    let sine = 1.0 - cosine * cosine;
    let triangle = frames * (frames + 1.0) / 2.0;
    let mut bound = triangle * triangle;

    if sine > 0.0 && frames * frames / sine < bound {
        bound = frames * frames / sine;
    }

    let limit = i32::MAX as f64 * i32::MAX as f64;
    let mut shift = 0;

    while shift < 31 {
        let largest = ((1u64 << (31 - shift)) as f64 + 0.5) * (1.0 + 1e-9);

        if largest * largest * bound <= limit {
            break;
        }

        shift += 1;
    }

    shift
}

#[inline]
fn mix_shift(channels: usize) -> u32 {
    math::MIX_SHIFTS[channels.min(8)] as u32
}

#[inline]
fn mix(frame: &[i32]) -> i32 {
    let shift = mix_shift(frame.len());

    frame
        .iter()
        .fold(0i32, |sum, value| sum.wrapping_add(value >> shift))
}

#[cfg(not(all(target_arch = "arm", not(target_has_atomic = "32"))))]
#[inline(always)]
fn multiply(coefficient: i32, state: i32) -> i32 {
    ((coefficient as i64 * state as i64 + (1 << 29)) >> 30) as i32
}

#[cfg(any(test, all(target_arch = "arm", not(target_has_atomic = "32"))))]
#[inline(always)]
fn multiply_split(coefficient: i32, state: i32) -> i32 {
    let left = coefficient as u32;
    let right = state as u32;
    let (left_low, left_high) = (left & 0xFFFF, left >> 16);
    let (right_low, right_high) = (right & 0xFFFF, right >> 16);
    let low = left_low * right_low;
    let cross_left = left_high * right_low;
    let cross_right = left_low * right_high;
    let middle = (low >> 16) + (cross_left & 0xFFFF) + (cross_right & 0xFFFF);
    let low = (low & 0xFFFF) | (middle << 16);
    let mut high =
        left_high * right_high + (cross_left >> 16) + (cross_right >> 16) + (middle >> 16);

    if coefficient < 0 {
        high = high.wrapping_sub(right);
    }

    if state < 0 {
        high = high.wrapping_sub(left);
    }

    let (low, carry) = low.overflowing_add(1 << 29);
    let high = high.wrapping_add(carry as u32);

    ((high << 2) | (low >> 30)) as i32
}

#[cfg(all(target_arch = "arm", not(target_has_atomic = "32")))]
#[inline(always)]
fn multiply(coefficient: i32, state: i32) -> i32 {
    multiply_split(coefficient, state)
}

impl<const F: usize> Analyzer<Fixed> for Goertzel<Fixed, F> {
    #[inline]
    fn analyze(&mut self, frame: &[i32]) {
        let (input, channels) = match self.channel {
            Some(channel) => (frame.get(channel as usize).copied().unwrap_or(0), 1),
            None => (mix(frame), frame.len()),
        };

        for index in 0..F {
            let previous = self.previous[index];
            let state = (input >> self.shifts[index])
                .wrapping_add(multiply(self.coefficients[index], previous))
                .wrapping_sub(self.before[index]);

            self.before[index] = previous;
            self.previous[index] = state;
        }

        self.advance(channels);
    }
}

impl<const F: usize> Analyzer<Float> for Goertzel<Float, F> {
    #[inline]
    fn analyze(&mut self, frame: &[f32]) {
        let (input, channels) = match self.channel {
            Some(channel) => (frame.get(channel as usize).copied().unwrap_or(0.0), 1),
            None => (frame.iter().sum(), frame.len()),
        };

        for index in 0..F {
            let previous = self.previous[index];
            let state = input + self.coefficients[index] * previous - self.before[index];

            self.before[index] = previous;
            self.previous[index] = state;
        }

        self.advance(channels);
    }
}

/// Detects the digits of DTMF (touch-tone) signals with a [`Goertzel`] analyzer of its eight frequencies.
///
/// A block lasts 205 frames at 8 kHz, about 25.6 ms, and as long at other sample rates. A block holds a digit when the strongest row and column tones are each above -30 dBFS, every other tone of their group is at least 6 dB below them, and the column tone is between 8 dB below and 4 dB above the row tone (the usual normal and reverse twist limits).
///
/// ```
/// use nanoaudio_analysis::{Dtmf, analyze_using};
/// use nanoaudio_dsp::Float;
///
/// let digit_5: [f32; 205] = core::array::from_fn(|index| {
///     let time = index as f32 / 8000.0;
///     0.3 * (2.0 * core::f32::consts::PI * 770.0 * time).sin()
///         + 0.3 * (2.0 * core::f32::consts::PI * 1336.0 * time).sin()
/// });
/// let mut dtmf = Dtmf::<Float>::new(8000);
///
/// analyze_using::<Float, _, _>(&digit_5, 1, &mut dtmf).unwrap();
///
/// assert_eq!(dtmf.digit(), Some('5'));
/// ```
#[derive(Clone, Copy)]
pub struct Dtmf<R: Representation> {
    goertzel: Goertzel<R, 8>,
}

impl<R: Representation> core::fmt::Debug for Dtmf<R>
where
    Goertzel<R, 8>: core::fmt::Debug,
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Dtmf")
            .field("goertzel", &self.goertzel)
            .finish()
    }
}

impl<R: Representation> PartialEq for Dtmf<R>
where
    Goertzel<R, 8>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.goertzel == other.goertzel
    }
}

const DTMF_FREQUENCIES: [f32; 8] = [697.0, 770.0, 852.0, 941.0, 1209.0, 1336.0, 1477.0, 1633.0];

const DTMF_DIGITS: [[char; 4]; 4] = [
    ['1', '2', '3', 'A'],
    ['4', '5', '6', 'B'],
    ['7', '8', '9', 'C'],
    ['*', '0', '#', 'D'],
];

const fn dtmf_frames(sample_rate: u32) -> u16 {
    let frames = (sample_rate as u64 * 205 + 4000) / 8000;

    if frames > u16::MAX as u64 {
        u16::MAX
    } else {
        frames as u16
    }
}

impl<R: Representation> Dtmf<R> {
    /// Returns the analyzer of the eight DTMF frequencies, rows from 697 Hz then columns from 1209 Hz.
    pub const fn goertzel(&self) -> &Goertzel<R, 8> {
        &self.goertzel
    }

    /// Returns the number of blocks completed, see [`Goertzel::blocks`].
    pub const fn blocks(&self) -> u64 {
        self.goertzel.blocks()
    }

    /// Analyzes only the channel at `channel`, see [`Goertzel::with_channel`].
    pub const fn with_channel(mut self, channel: u8) -> Self {
        self.goertzel = self.goertzel.with_channel(channel);
        self
    }

    /// Forgets every frame analyzed, as if newly created.
    pub fn reset(&mut self) {
        self.goertzel.reset();
    }
}

impl Dtmf<Fixed> {
    /// Creates a detector for audio at `sample_rate`.
    pub const fn new(sample_rate: u32) -> Self {
        Self {
            goertzel: Goertzel::<Fixed, 8>::new(
                sample_rate,
                dtmf_frames(sample_rate),
                DTMF_FREQUENCIES,
            ),
        }
    }

    /// Returns the digit heard in the last block, or `None`.
    pub fn digit(&self) -> Option<char> {
        decide(self.goertzel.magnitudes().map(|magnitude| magnitude as u32))
    }
}

impl Dtmf<Float> {
    /// Creates a detector for audio at `sample_rate`.
    pub const fn new(sample_rate: u32) -> Self {
        Self {
            goertzel: Goertzel::<Float, 8>::new(
                sample_rate,
                dtmf_frames(sample_rate),
                DTMF_FREQUENCIES,
            ),
        }
    }

    /// Returns the digit heard in the last block, or `None`.
    pub fn digit(&self) -> Option<char> {
        decide(
            self.goertzel
                .magnitudes()
                .map(|magnitude| (magnitude as f64 * ONE) as u32),
        )
    }
}

impl<R: Representation> Analyzer<R> for Dtmf<R>
where
    Goertzel<R, 8>: Analyzer<R>,
{
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        self.goertzel.analyze(frame);
    }
}

const DTMF_MINIMUM: u64 = 67_908_607;
const DTMF_RELATIVE: u64 = 512;
const DTMF_NORMAL_TWIST: u64 = 643;
const DTMF_REVERSE_TWIST: u64 = 406;
const UNIT: u64 = 256;

fn strongest(magnitudes: &[u32]) -> Option<(usize, u64)> {
    let mut best = 0;

    for index in 1..magnitudes.len() {
        if magnitudes[index] > magnitudes[best] {
            best = index;
        }
    }

    let peak = magnitudes[best] as u64;
    let clear = magnitudes
        .iter()
        .enumerate()
        .all(|(index, other)| index == best || (*other as u64) * DTMF_RELATIVE <= peak * UNIT);

    (peak >= DTMF_MINIMUM && clear).then_some((best, peak))
}

fn decide(magnitudes: [u32; 8]) -> Option<char> {
    let (row, low) = strongest(&magnitudes[..4])?;
    let (column, high) = strongest(&magnitudes[4..])?;

    (low * UNIT <= high * DTMF_NORMAL_TWIST && high * UNIT <= low * DTMF_REVERSE_TWIST)
        .then_some(DTMF_DIGITS[row][column])
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::f64::consts::PI;
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

    fn reference(signal: &[f64], frequency: f64, sample_rate: f64) -> f64 {
        let angle = 2.0 * PI * frequency / sample_rate;
        let (mut real, mut imaginary) = (0.0, 0.0);

        for (index, value) in signal.iter().enumerate() {
            real += value * (angle * index as f64).cos();
            imaginary -= value * (angle * index as f64).sin();
        }

        2.0 * (real * real + imaginary * imaginary).sqrt() / signal.len() as f64
    }

    fn tones(length: usize, sample_rate: f64, tones: &[(f64, f64, f64)], noise: f64) -> Vec<i16> {
        let mut random = Random(length as u64 + 7);

        (0..length)
            .map(|index| {
                let time = index as f64 / sample_rate;
                let value: f64 = tones
                    .iter()
                    .map(|(frequency, amplitude, phase)| {
                        amplitude * (2.0 * PI * frequency * time + phase).sin()
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

    trait Measured: Representation {
        const TOLERANCE: f64;

        fn create<const F: usize>(
            sample_rate: u32,
            frames: u16,
            frequencies: [f32; F],
        ) -> Goertzel<Self, F>
        where
            Goertzel<Self, F>: Analyzer<Self>;

        fn magnitudes<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f64; F];

        fn powers<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f64; F];

        fn decibels<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f32; F];
    }

    impl Measured for Fixed {
        const TOLERANCE: f64 = 5e-6;

        fn create<const F: usize>(
            sample_rate: u32,
            frames: u16,
            frequencies: [f32; F],
        ) -> Goertzel<Self, F> {
            Goertzel::<Fixed, F>::new(sample_rate, frames, frequencies)
        }

        fn magnitudes<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f64; F] {
            goertzel
                .magnitudes()
                .map(|magnitude| magnitude as f64 / ONE)
        }

        fn powers<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f64; F] {
            goertzel.powers().map(|power| power as f64 / ONE)
        }

        fn decibels<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f32; F] {
            goertzel.decibels()
        }
    }

    impl Measured for Float {
        const TOLERANCE: f64 = 5e-5;

        fn create<const F: usize>(
            sample_rate: u32,
            frames: u16,
            frequencies: [f32; F],
        ) -> Goertzel<Self, F> {
            Goertzel::<Float, F>::new(sample_rate, frames, frequencies)
        }

        fn magnitudes<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f64; F] {
            goertzel.magnitudes().map(|magnitude| magnitude as f64)
        }

        fn powers<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f64; F] {
            goertzel.powers().map(|power| power as f64)
        }

        fn decibels<const F: usize>(goertzel: &Goertzel<Self, F>) -> [f32; F] {
            goertzel.decibels()
        }
    }

    fn check<R: Measured, const F: usize>(
        goertzel: &Goertzel<R, F>,
        signal: &[f64],
        frequencies: [f32; F],
        sample_rate: u32,
        tolerance: f64,
    ) {
        let magnitudes = R::magnitudes(goertzel);
        let powers = R::powers(goertzel);
        let decibels = R::decibels(goertzel);

        for index in 0..F {
            let expected = reference(signal, frequencies[index] as f64, sample_rate as f64);

            std::println!(
                "{} {} {}",
                frequencies[index],
                magnitudes[index] - expected,
                expected
            );
            assert!(
                (magnitudes[index] - expected).abs() <= tolerance,
                "{} Hz: {} instead of {expected}",
                frequencies[index],
                magnitudes[index]
            );
            assert!((powers[index] - expected * expected).abs() <= 2.0 * tolerance + 1e-9);

            if expected > 1e-3 {
                let level = 20.0 * expected.log10();

                assert!(
                    (decibels[index] as f64 - level).abs()
                        <= 20.0 * (1.0 + tolerance / expected).log10() + 1e-4,
                    "{} Hz: {} dB instead of {level}",
                    frequencies[index],
                    decibels[index]
                );
            }
        }
    }

    fn matches_the_reference<R: Measured>()
    where
        Goertzel<R, 4>: Analyzer<R>,
    {
        let cases: [(u32, u16); 4] = [(8000, 205), (8000, 256), (48000, 1000), (44100, 4096)];
        let cases = if cfg!(miri) { &cases[..1] } else { &cases[..] };

        for (sample_rate, frames) in cases.iter().copied() {
            let rate = sample_rate as f64;
            let bin = rate / frames as f64;
            let frequencies = [
                (12.0 * bin) as f32,
                (31.3 * bin) as f32,
                (frames as f64 / 4.0 * bin + 0.5 * bin) as f32,
                (0.47 * rate) as f32,
            ];
            let samples = tones(
                frames as usize,
                rate,
                &[
                    (frequencies[0] as f64, 0.5, 0.3),
                    (frequencies[1] as f64, 0.01, 1.0),
                    (17.0 * bin, 0.3, 2.0),
                ],
                0.001,
            );
            let signal = normalized(&samples);
            let mut goertzel = R::create(sample_rate, frames, frequencies);

            assert_eq!(
                analyze_using::<R, _, _>(&samples, 1, &mut goertzel),
                Ok(frames as usize)
            );
            assert_eq!(goertzel.blocks(), 1);
            check(&goertzel, &signal, frequencies, sample_rate, R::TOLERANCE);
        }
    }

    fn selects_or_mixes_channels<R: Measured>()
    where
        Goertzel<R, 2>: Analyzer<R> + PartialEq,
    {
        let frames = 480;
        let frequencies = [1000.0, 1234.5];
        let left = tones(frames, 48000.0, &[(1000.0, 0.6, 0.0)], 0.0);
        let right = tones(
            frames,
            48000.0,
            &[(1234.5, 0.4, 1.0), (1000.0, 0.2, 2.0)],
            0.0,
        );
        let interleaved: Vec<i16> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        let mut output = std::vec![0i16; interleaved.len()];

        for channel in [None, Some(0), Some(1)] {
            let mut tapped = R::create(48000, frames as u16, frequencies);
            let mut sliced = R::create(48000, frames as u16, frequencies);

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
                    .map(|(l, r)| (l + r) / 2.0)
                    .collect(),
                Some(0) => normalized(&left),
                _ => normalized(&right),
            };

            check(&tapped, &signal, frequencies, 48000, R::TOLERANCE);
        }

        let mut missing = R::create(48000, frames as u16, frequencies).with_channel(2);

        analyze_using::<R, _, _>(&interleaved, 2, &mut missing).unwrap();
        assert_eq!(missing.blocks(), 1);
        assert_eq!(R::magnitudes(&missing), [0.0; 2]);
    }

    fn reports_the_last_complete_block<R: Measured>()
    where
        Goertzel<R, 1>: Analyzer<R> + PartialEq,
    {
        let frequencies = [500.0];
        let mut goertzel = R::create(8000, 160, frequencies);

        assert_eq!(
            analyze_using::<R, _, _>(&[0i16; 0], 1, &mut goertzel),
            Ok(0)
        );
        assert_eq!(goertzel.blocks(), 0);
        assert_eq!(R::magnitudes(&goertzel), [0.0]);
        assert_eq!(R::decibels(&goertzel), [f32::NEG_INFINITY]);

        let loud = tones(160, 8000.0, &[(500.0, 0.5, 0.0)], 0.0);
        let quiet = tones(100, 8000.0, &[(500.0, 0.05, 0.0)], 0.0);

        analyze_using::<R, _, _>(&loud, 1, &mut goertzel).unwrap();
        analyze_using::<R, _, _>(&quiet, 1, &mut goertzel).unwrap();
        assert_eq!(goertzel.blocks(), 1);
        check(
            &goertzel,
            &normalized(&loud),
            frequencies,
            8000,
            R::TOLERANCE,
        );

        analyze_using::<R, _, _>(&quiet[..60], 1, &mut goertzel).unwrap();
        assert_eq!(goertzel.blocks(), 2);

        let mut second = quiet.clone();
        second.extend_from_slice(&quiet[..60]);
        check(
            &goertzel,
            &normalized(&second),
            frequencies,
            8000,
            R::TOLERANCE,
        );

        let fresh = R::create(8000, 160, frequencies);
        goertzel.reset();
        assert!(goertzel == fresh);

        let mut single = R::create(8000, 0, frequencies);
        analyze_using::<R, _, _>(&loud[..3], 1, &mut single).unwrap();
        assert_eq!(single.frames(), 1);
        assert_eq!(single.blocks(), 3);

        let long = R::create(8000, u16::MAX, frequencies);
        assert_eq!(long.frames(), Goertzel::<R, 1>::MAXIMUM_FRAMES);
    }

    fn runs_for_long<R: Measured>()
    where
        Goertzel<R, 1>: Analyzer<R> + PartialEq,
    {
        let blocks = if cfg!(miri) { 3 } else { 3000 };
        let samples = tones(1000, 8000.0, &[(440.0, 0.9, 0.0)], 0.0);
        let mut goertzel = R::create(8000, 1000, [440.0]);

        for _ in 0..blocks {
            analyze_using::<R, _, _>(&samples, 1, &mut goertzel).unwrap();
        }

        assert_eq!(goertzel.blocks(), blocks);
        check(
            &goertzel,
            &normalized(&samples),
            [440.0],
            8000,
            R::TOLERANCE,
        );
    }

    #[test]
    fn fixed() {
        matches_the_reference::<Fixed>();
        selects_or_mixes_channels::<Fixed>();
        reports_the_last_complete_block::<Fixed>();
        runs_for_long::<Fixed>();
    }

    #[test]
    fn float() {
        matches_the_reference::<Float>();
        selects_or_mixes_channels::<Float>();
        reports_the_last_complete_block::<Float>();
        runs_for_long::<Float>();
    }

    fn worst_case(frames: usize, frequency: f64, sample_rate: f64, polarity: bool) -> Vec<i32> {
        let cosine = (2.0 * PI * frequency / sample_rate).cos();
        let mut chebyshev = [1.0, 2.0 * cosine];
        let mut signs = std::vec![1.0; frames];

        for sign in signs.iter_mut().rev().skip(1) {
            *sign = chebyshev[1];
            chebyshev = [chebyshev[1], 2.0 * cosine * chebyshev[1] - chebyshev[0]];
        }

        signs
            .iter()
            .map(|sign| {
                if (*sign < 0.0) != polarity {
                    i32::MIN
                } else {
                    i32::MAX
                }
            })
            .collect()
    }

    #[test]
    fn fixed_does_not_overflow() {
        let frames = if cfg!(miri) {
            1024
        } else {
            Goertzel::<Fixed, 0>::MAXIMUM_FRAMES as usize
        };
        let sample_rate = 48000;
        let frequencies = [0.0, 0.5, 30.0, 1000.0, 12000.0, 23999.0, 24000.0];

        for frames in [frames, 512, 205] {
            for frequency in frequencies {
                for polarity in [false, true] {
                    let samples = worst_case(frames, frequency, sample_rate as f64, polarity);
                    let signal: Vec<f64> =
                        samples.iter().map(|sample| *sample as f64 / ONE).collect();
                    let mut goertzel =
                        Goertzel::<Fixed, 1>::new(sample_rate, frames as u16, [frequency as f32]);

                    analyze_using::<Fixed, _, _>(&samples, 1, &mut goertzel).unwrap();

                    let shift = goertzel.shifts[0] as i32;
                    let actual = (goertzel.power(0) as f64).sqrt() * 2.0f64.powi(shift + 1 - 15)
                        / frames as f64
                        / ONE;
                    let expected = reference(&signal, frequency, sample_rate as f64);

                    assert!(
                        (actual - expected).abs() <= 2.0f64.powi(shift - 29) + 1e-6 * expected,
                        "{frequency} Hz over {frames}: {actual} instead of {expected}"
                    );
                }
            }
        }

        let samples = [i32::MIN; 64];
        let mut goertzel = Goertzel::<Fixed, 1>::new(8000, 64, [0.0]);

        analyze_using::<Fixed, _, _>(&samples, 1, &mut goertzel).unwrap();
        assert_eq!(goertzel.magnitudes(), [i32::MAX]);

        let mut stereo = Goertzel::<Fixed, 1>::new(8000, 64, [4000.0]);
        let alternating: Vec<i32> = (0..256)
            .map(|index| if index % 4 < 2 { i32::MIN } else { i32::MAX })
            .collect();

        analyze_using::<Fixed, _, _>(&alternating, 2, &mut stereo).unwrap();
        assert!((stereo.magnitudes()[0] as f64 / ONE - 1.0).abs() < 1e-6);
    }

    #[test]
    fn float_propagates_invalid_samples() {
        let mut goertzel = Goertzel::<Float, 2>::new(8000, 4, [1000.0, 2000.0]);
        let samples = [0.5f32, f32::NAN, 0.0, 0.0, 0.0, 0.5, 0.0, -0.5];

        analyze_using::<Float, _, _>(&samples[..4], 1, &mut goertzel).unwrap();
        assert!(
            goertzel
                .magnitudes()
                .iter()
                .all(|magnitude| magnitude.is_nan())
        );
        assert!(goertzel.decibels().iter().all(|level| level.is_nan()));

        analyze_using::<Float, _, _>(&samples[4..], 1, &mut goertzel).unwrap();
        assert!((goertzel.magnitudes()[1] - 0.5).abs() < 1e-6);

        let infinite = [f32::INFINITY, 0.0, 0.0, 0.0];
        analyze_using::<Float, _, _>(&infinite, 1, &mut goertzel).unwrap();
        assert!(
            goertzel
                .magnitudes()
                .iter()
                .all(|magnitude| magnitude.is_nan())
        );
    }

    #[test]
    fn split_multiply_is_exact() {
        let edges = [
            i32::MIN,
            i32::MIN + 1,
            -(1 << 30),
            -65537,
            -65536,
            -1,
            0,
            1,
            65535,
            65536,
            1 << 30,
            i32::MAX,
        ];
        let mut random = Random(3);

        for coefficient in edges {
            for state in edges {
                assert_eq!(
                    multiply_split(coefficient, state),
                    ((coefficient as i64 * state as i64 + (1 << 29)) >> 30) as i32
                );
            }
        }

        for _ in 0..if cfg!(miri) { 1000 } else { 1_000_000 } {
            let coefficient = (random.next() * ONE) as i32;
            let state = (random.next() * ONE) as i32;

            assert_eq!(
                multiply_split(coefficient, state),
                ((coefficient as i64 * state as i64 + (1 << 29)) >> 30) as i32
            );
        }
    }

    const ROWS: [f64; 4] = [697.0, 770.0, 852.0, 941.0];
    const COLUMNS: [f64; 4] = [1209.0, 1336.0, 1477.0, 1633.0];

    fn dial(sample_rate: f64, row: f64, column: f64, low: f64, high: f64) -> Vec<i16> {
        tones(
            (sample_rate / 20.0) as usize,
            sample_rate,
            &[(row, low, 0.4), (column, high, 1.3)],
            0.002,
        )
    }

    fn detects<R: Representation>(
        create: impl Fn(u32) -> Dtmf<R>,
        digit: impl Fn(&Dtmf<R>) -> Option<char>,
    ) where
        Dtmf<R>: Analyzer<R>,
    {
        let sample_rates = if cfg!(miri) {
            &[8000][..]
        } else {
            &[8000, 16000, 44100][..]
        };

        for sample_rate in sample_rates.iter().copied() {
            for (row_index, row) in ROWS.iter().enumerate() {
                for (column_index, column) in COLUMNS.iter().enumerate() {
                    let mut detector = create(sample_rate);
                    let samples = dial(sample_rate as f64, *row, *column, 0.25, 0.3);
                    let frames = detector.goertzel().frames() as usize;

                    analyze_using::<R, _, _>(&samples[frames / 3..][..frames], 1, &mut detector)
                        .unwrap();
                    assert_eq!(digit(&detector), Some(DTMF_DIGITS[row_index][column_index]));
                    assert_eq!(detector.blocks(), 1);
                }
            }
        }

        let cases = [
            (ROWS[1], COLUMNS[2], 0.3, 0.0),
            (ROWS[1], COLUMNS[2], 0.5, 0.15),
            (ROWS[1], COLUMNS[2], 0.15, 0.3),
            (ROWS[1], COLUMNS[2], 0.02, 0.02),
            (ROWS[1], ROWS[3], 0.3, 0.3),
            (COLUMNS[0], COLUMNS[3], 0.3, 0.3),
            (ROWS[1], COLUMNS[2], 0.0, 0.0),
        ];

        for (low_frequency, high_frequency, low, high) in cases {
            let mut detector = create(8000);

            analyze_using::<R, _, _>(
                &dial(8000.0, low_frequency, high_frequency, low, high)[..205],
                1,
                &mut detector,
            )
            .unwrap();
            assert_eq!(
                digit(&detector),
                None,
                "{low_frequency} {high_frequency} {low} {high}"
            );
        }

        let mut accepted = create(8000);

        analyze_using::<R, _, _>(
            &dial(8000.0, ROWS[2], COLUMNS[1], 0.5, 0.25)[..205],
            1,
            &mut accepted,
        )
        .unwrap();
        assert_eq!(digit(&accepted), Some('8'));

        let stereo: Vec<i16> = dial(8000.0, ROWS[3], COLUMNS[2], 0.25, 0.25)[..205]
            .iter()
            .flat_map(|sample| [0, *sample])
            .collect();
        let mut right = create(8000).with_channel(1);

        analyze_using::<R, _, _>(&stereo, 2, &mut right).unwrap();
        assert_eq!(digit(&right), Some('#'));
        right.reset();
        assert_eq!(right.blocks(), 0);
        assert_eq!(digit(&right), None);
    }

    #[test]
    fn dtmf() {
        detects(Dtmf::<Fixed>::new, Dtmf::<Fixed>::digit);
        detects(Dtmf::<Float>::new, Dtmf::<Float>::digit);
    }
}
