/// The shape of an [`Oscillator`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Waveform {
    /// A sine.
    #[default]
    Sine,
    /// A pulse wave, square at the default pulse width, band-limited with PolyBLEP.
    Square,
    /// A rising sawtooth, band-limited with PolyBLEP.
    Saw,
    /// A triangle.
    Triangle,
    /// White noise held for one period, so the frequency sets how bright it sounds.
    Noise,
}

const PHASE_SCALE: f32 = 4_294_967_296.0;

const SEMITONES: [u64; 12] = [
    1_073_741_824,
    1_137_589_835,
    1_205_234_447,
    1_276_901_417,
    1_352_829_926,
    1_433_273_380,
    1_518_500_250,
    1_608_794_974,
    1_704_458_901,
    1_805_811_301,
    1_913_190_429,
    2_026_954_652,
];

const LN2_PER_STEP: u64 = (core::f64::consts::LN_2 / 3072.0 * (1u64 << 46) as f64) as u64;

const A4: i64 = 69 << 8;

/// Returns the phase increment of `frequency` Hz at `sample_rate`, for [`Oscillator::set_increment`].
///
/// Frequencies at or above the sample rate give `u32::MAX`, and negative ones or NaN give 0. It uses floating point, so on cores without an FPU, call it when a parameter changes rather than for every sample.
pub fn frequency_increment(frequency: f32, sample_rate: u32) -> u32 {
    let ratio = frequency / sample_rate as f32;

    if ratio > 0.0 {
        (ratio * PHASE_SCALE) as u32
    } else {
        0
    }
}

/// Returns the phase increment of `pitch` at `sample_rate`, in equal temperament with A4 at 440 Hz.
///
/// `pitch` is in 1/256 semitone, so MIDI note `n` is `n << 8`, and a pitch bend of a quarter semitone adds 64. It only uses integers.
pub fn note_increment(pitch: i32, sample_rate: u32) -> u32 {
    if sample_rate == 0 {
        return u32::MAX;
    }

    let offset = pitch as i64 - A4;
    let octave = offset.div_euclid(12 << 8);
    let rest = offset.rem_euclid(12 << 8) as u64;
    let step = ((rest & 0xFF) * LN2_PER_STEP + (1 << 15)) >> 16;
    let square = (step * step) >> 30;
    let cube = (square * step) >> 30;
    let fine = (1 << 30) + step + square / 2 + cube / 6 + ((cube * step) >> 30) / 24;
    let ratio = (SEMITONES[(rest >> 8) as usize] * fine) >> 30;
    let base = (440u128 << 64) / sample_rate as u128;
    let shift = 62 - octave;

    if shift >= 128 {
        return 0;
    }

    let product = base * ratio as u128;

    if shift <= 0 {
        return u32::MAX;
    }

    ((product + (1 << (shift - 1))) >> shift).min(u32::MAX as u128) as u32
}

const fn sine_coefficient(value: f64) -> i32 {
    let scaled = value * 32_767.0;

    (scaled + if value < 0.0 { -0.5 } else { 0.5 }) as i32
}

const SINE_1: i32 = sine_coefficient(1.570_791_011_060_690_4);
const SINE_3: i32 = sine_coefficient(-0.645_892_849_429_933_4);
const SINE_5: i32 = sine_coefficient(0.079_434_344_380_822_22);
const SINE_7: i32 = sine_coefficient(-0.004_333_095_157_952_491);

#[inline(always)]
fn sine(phase: u32) -> i16 {
    let quarter = phase as i32;
    let folded = if (quarter ^ (quarter << 1)) < 0 {
        i32::MIN.wrapping_sub(quarter)
    } else {
        quarter
    };
    let z = folded >> 15;
    let square = (z * z) >> 15;
    let polynomial = SINE_5 + ((square * SINE_7) >> 15);
    let polynomial = SINE_3 + ((square * polynomial) >> 15);
    let polynomial = SINE_1 + ((square * polynomial) >> 15);

    ((z * polynomial + (1 << 14)) >> 15) as i16
}

pub(crate) fn sine_f32(turns: f32) -> f32 {
    let z = turns * 4.0;
    let square = z * z;
    let polynomial = -0.004_333_095_f32;
    let polynomial = 0.079_434_34 + square * polynomial;
    let polynomial = -0.645_892_85 + square * polynomial;
    let polynomial = 1.570_791 + square * polynomial;

    z * polynomial
}

#[inline(always)]
fn triangle(phase: u32) -> i16 {
    let shifted = phase.wrapping_add(1 << 30) as i32;

    (((shifted ^ (shifted >> 31)) >> 15) - 32_768) as i16
}

#[inline(always)]
fn noise(state: u32) -> u32 {
    let state = state ^ (state << 13);
    let state = state ^ (state >> 17);

    state ^ (state << 5)
}

const SEED: u32 = 0x2545_F491;

const BLEP_MAXIMUM: u32 = (1 << 31) - 1;

/// A phase-accumulator oscillator producing 16-bit samples.
///
/// The phase is a `u32` covering one period, so a frequency is an increment per sample, which [`frequency_increment`] and [`note_increment`] compute. Square and saw waves are band-limited with PolyBLEP, which removes most aliasing for a few instructions per sample. Triangle and sine are not band-limited: their harmonics fall off fast enough that aliasing stays low.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oscillator {
    waveform: Waveform,
    phase: u32,
    increment: u32,
    width: u32,
    blep: u32,
    reciprocal: u32,
    noise: u32,
}

impl Default for Oscillator {
    fn default() -> Self {
        Self::new(Waveform::Sine)
    }
}

impl Oscillator {
    /// Creates a silent oscillator, at frequency 0 and phase 0.
    pub const fn new(waveform: Waveform) -> Self {
        Self {
            waveform,
            phase: 0,
            increment: 0,
            width: 1 << 31,
            blep: 0,
            reciprocal: 0,
            noise: SEED,
        }
    }

    /// Returns the waveform.
    pub fn waveform(&self) -> Waveform {
        self.waveform
    }

    /// Changes the waveform, keeping the phase.
    pub fn set_waveform(&mut self, waveform: Waveform) {
        self.waveform = waveform;
    }

    /// Returns the phase increment per sample.
    pub fn increment(&self) -> u32 {
        self.increment
    }

    /// Sets the phase increment per sample, which sets the frequency.
    ///
    /// It takes one integer division, so it fits a change every few samples, not every sample on cores without a hardware divider.
    pub fn set_increment(&mut self, increment: u32) {
        let blep = increment.min(BLEP_MAXIMUM);
        let units = blep >> 16;

        self.increment = increment;
        (self.blep, self.reciprocal) = match (1u32 << 30).checked_div(units) {
            Some(reciprocal) => (blep, reciprocal),
            None => (0, 0),
        };
    }

    /// Sets the frequency in Hz at `sample_rate`, see [`frequency_increment`].
    pub fn set_frequency(&mut self, frequency: f32, sample_rate: u32) {
        self.set_increment(frequency_increment(frequency, sample_rate));
    }

    /// Returns the pulse width of [`Waveform::Square`], as a fraction of the period out of 65 536.
    pub fn pulse_width(&self) -> u16 {
        (self.width >> 16) as u16
    }

    /// Sets the pulse width of [`Waveform::Square`], as a fraction of the period out of 65 536.
    ///
    /// The default, 32 768, gives a square wave.
    pub fn set_pulse_width(&mut self, width: u16) {
        self.width = (width as u32) << 16;
    }

    /// Returns the phase, where `u32::MAX` is one period.
    pub fn phase(&self) -> u32 {
        self.phase
    }

    /// Sets the phase, where `u32::MAX` is one period.
    pub fn set_phase(&mut self, phase: u32) {
        self.phase = phase;
    }

    /// Restarts the noise sequence from `seed`. A seed of 0 is replaced with the default one.
    pub fn set_seed(&mut self, seed: u32) {
        self.noise = if seed == 0 { SEED } else { seed };
    }

    #[inline(always)]
    fn fraction(&self, distance: u32) -> i32 {
        (((distance >> 16) * self.reciprocal) >> 15) as i32
    }

    #[inline(always)]
    fn blep(&self, phase: u32) -> i32 {
        if phase < self.blep {
            let remaining = 32_768 - self.fraction(phase);
            -((remaining * remaining) >> 15)
        } else if phase.wrapping_neg() < self.blep {
            let remaining = 32_768 - self.fraction(phase.wrapping_neg());
            (remaining * remaining) >> 15
        } else {
            0
        }
    }

    /// Returns the next sample and advances the phase.
    #[inline]
    pub fn next_sample(&mut self) -> i16 {
        let phase = self.phase;
        let (next, wrapped) = phase.overflowing_add(self.increment);
        self.phase = next;

        match self.waveform {
            Waveform::Sine => sine(phase),
            Waveform::Square => {
                let naive = if phase < self.width { 32_767 } else { -32_767 };
                let value = naive + self.blep(phase) - self.blep(phase.wrapping_sub(self.width));

                value.clamp(-32_768, 32_767) as i16
            }
            Waveform::Saw => (((phase >> 16) as i32 - 32_768) - self.blep(phase)) as i16,
            Waveform::Triangle => triangle(phase),
            Waveform::Noise => {
                let value = (self.noise >> 16) as i16;

                if wrapped {
                    self.noise = noise(self.noise);
                }

                value
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;

    use std::vec::Vec;

    fn exact_increment(pitch: i32, sample_rate: u32) -> f64 {
        440.0 * 2f64.powf((pitch as f64 / 256.0 - 69.0) / 12.0) * 4_294_967_296.0
            / sample_rate as f64
    }

    #[test]
    fn note_increments_are_accurate() {
        for sample_rate in [8_000, 22_050, 44_100, 48_000, 96_000] {
            for pitch in (0..=(127 << 8)).step_by(if cfg!(miri) { 257 } else { 7 }) {
                let expected = exact_increment(pitch, sample_rate);

                if expected >= 4_294_967_295.0 {
                    assert_eq!(note_increment(pitch, sample_rate), u32::MAX);
                    continue;
                }

                let actual = note_increment(pitch, sample_rate) as f64;
                let error = (actual - expected).abs();

                assert!(
                    error <= 1.0 || error / expected < 2.0e-7,
                    "{pitch} {sample_rate} {actual} {expected}"
                );
            }
        }
    }

    #[test]
    fn note_increments_saturate() {
        assert_eq!(note_increment(i32::MIN, 44_100), 0);
        assert_eq!(note_increment(i32::MAX, 44_100), u32::MAX);
        assert_eq!(note_increment(300 << 8, 44_100), u32::MAX);
        assert_eq!(note_increment(69 << 8, 0), u32::MAX);
        assert_eq!(note_increment(69 << 8, 1), u32::MAX);
        assert_eq!(note_increment(0, u32::MAX), 8);
    }

    #[test]
    fn frequency_increments_match_notes() {
        assert!(frequency_increment(440.0, 44_100).abs_diff(note_increment(69 << 8, 44_100)) <= 4);
        assert_eq!(frequency_increment(0.0, 44_100), 0);
        assert_eq!(frequency_increment(-5.0, 44_100), 0);
        assert_eq!(frequency_increment(f32::NAN, 44_100), 0);
        assert_eq!(frequency_increment(44_100.0, 44_100), u32::MAX);
        assert_eq!(frequency_increment(1.0, 0), u32::MAX);
    }

    #[test]
    fn sine_is_accurate() {
        let mut worst = 0.0f64;
        let count = if cfg!(miri) { 1 << 10 } else { 1 << 18 };

        for index in 0..count {
            let phase = (index as u32).wrapping_mul(0x9E37_79B9);
            let expected =
                (phase as f64 / 4_294_967_296.0 * core::f64::consts::TAU).sin() * 32_767.0;
            worst = worst.max((sine(phase) as f64 - expected).abs());
        }

        assert!(worst < 3.0, "{worst}");
        assert_eq!(sine(0), 0);
        assert_eq!(sine(1 << 30), 32_767);
        assert_eq!(sine(3 << 30), -32_767);
    }

    #[test]
    fn float_sine_is_accurate() {
        for index in 0..=1000 {
            let turns = index as f32 / 4000.0;
            let expected = (turns as f64 * core::f64::consts::TAU).sin();

            assert!(
                (sine_f32(turns) as f64 - expected).abs() < 2.0e-6,
                "{turns}"
            );
        }
    }

    #[test]
    fn triangle_has_the_right_shape() {
        assert_eq!(triangle(0), 0);
        assert_eq!(triangle(1 << 30), 32_767);
        assert_eq!(triangle(2 << 30), -1);
        assert_eq!(triangle(3 << 30), -32_768);
    }

    fn crossings(oscillator: &mut Oscillator, samples: usize) -> (usize, usize, usize) {
        let mut previous = oscillator.next_sample();
        let mut count = 0;
        let mut first = 0;
        let mut last = 0;

        for index in 1..samples {
            let sample = oscillator.next_sample();

            if previous < 0 && sample >= 0 {
                if count == 0 {
                    first = index;
                }
                last = index;
                count += 1;
            }

            previous = sample;
        }

        (count, first, last)
    }

    #[test]
    fn frequencies_are_exact() {
        let sample_rate = 48_000;
        let samples = if cfg!(miri) { 4_800 } else { 480_000 };

        for waveform in [
            Waveform::Sine,
            Waveform::Square,
            Waveform::Saw,
            Waveform::Triangle,
        ] {
            for note in [21u8, 45, 60, 69, 93, 105] {
                let mut oscillator = Oscillator::new(waveform);
                oscillator.set_increment(note_increment((note as i32) << 8, sample_rate));
                oscillator.set_phase(1 << 31);
                let (count, first, last) = crossings(&mut oscillator, samples);
                let measured = (count - 1) as f64 * sample_rate as f64 / (last - first) as f64;
                let expected = 440.0 * 2f64.powf((note as f64 - 69.0) / 12.0);
                let tolerance = 2.0 / (last - first) as f64 + 1.0e-6;

                assert!(
                    (measured - expected).abs() / expected <= tolerance,
                    "{waveform:?} {note} {measured} {expected}"
                );
            }
        }
    }

    #[test]
    fn noise_holds_for_one_period() {
        let mut oscillator = Oscillator::new(Waveform::Noise);
        oscillator.set_increment(1 << 30);
        let samples: Vec<i16> = (0..16).map(|_| oscillator.next_sample()).collect();

        for chunk in samples.chunks(4) {
            assert!(chunk.iter().all(|sample| *sample == chunk[0]));
        }

        assert_ne!(samples[0], samples[4]);

        oscillator.set_increment(u32::MAX);
        let samples: Vec<i16> = (0..64).map(|_| oscillator.next_sample()).collect();

        assert!(samples[1..].windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn extremes_stay_in_range() {
        let increments = [
            0,
            1,
            65_535,
            65_536,
            1 << 24,
            (1 << 31) - 1,
            1 << 31,
            u32::MAX - 1,
            u32::MAX,
        ];
        let widths = [0, 1, 1 << 15, u16::MAX - 1, u16::MAX];
        let samples = if cfg!(miri) { 64 } else { 4_096 };

        for waveform in [
            Waveform::Sine,
            Waveform::Square,
            Waveform::Saw,
            Waveform::Triangle,
            Waveform::Noise,
        ] {
            for increment in increments {
                for width in widths {
                    let mut oscillator = Oscillator::new(waveform);
                    oscillator.set_increment(increment);
                    oscillator.set_pulse_width(width);
                    oscillator.set_phase(increment.wrapping_mul(3));

                    for _ in 0..samples {
                        oscillator.next_sample();
                    }
                }
            }
        }
    }
}
