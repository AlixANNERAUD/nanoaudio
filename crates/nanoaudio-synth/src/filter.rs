use crate::Synthesis;
use crate::oscillator::sine_f32;

/// Which output of a [`Filter`] is used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FilterMode {
    /// Keeps frequencies below the cutoff.
    #[default]
    LowPass,
    /// Keeps frequencies around the cutoff.
    BandPass,
    /// Keeps frequencies above the cutoff.
    HighPass,
}

/// The settings of a [`Filter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterSettings {
    /// Which output is used.
    pub mode: FilterMode,
    /// Cutoff frequency in Hz, limited to 0.49 times the sample rate.
    pub cutoff: f32,
    /// Resonance from 0, a flat response, to 1, a peak of about 24 dB at the cutoff.
    pub resonance: f32,
}

impl FilterSettings {
    /// Creates settings.
    pub const fn new(mode: FilterMode, cutoff: f32, resonance: f32) -> Self {
        Self {
            mode,
            cutoff,
            resonance,
        }
    }

    /// Creates low-pass settings.
    pub const fn low_pass(cutoff: f32, resonance: f32) -> Self {
        Self::new(FilterMode::LowPass, cutoff, resonance)
    }
}

const MAXIMUM_RATIO: f32 = 0.49;

const MINIMUM_DAMPING: f32 = 1.0 / 16.0;

/// A resonant state-variable filter, with low-pass, band-pass and high-pass outputs.
///
/// It is the trapezoidal (zero-delay feedback) form, which stays stable at any cutoff up to near half the sample rate and when its settings change on every sample. Settings are converted to coefficients with floating point in [`new`](Self::new) and [`set`](Self::set), so on cores without an FPU, change them every few samples at most; the filter itself runs in the representation `R`, with four multiplications per sample.
#[derive(Clone)]
pub struct Filter<R: Synthesis> {
    state: [R::Value; 2],
    coefficients: [R::Coefficient; 4],
    mode: FilterMode,
}

impl<R: Synthesis> core::fmt::Debug for Filter<R> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Filter")
            .field("coefficients", &self.coefficients)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl<R: Synthesis> Default for Filter<R> {
    fn default() -> Self {
        Self {
            state: [R::SILENCE; 2],
            coefficients: [R::Coefficient::default(); 4],
            mode: FilterMode::LowPass,
        }
    }
}

impl<R: Synthesis> Filter<R> {
    /// Creates a filter for audio at `sample_rate`.
    pub fn new(settings: FilterSettings, sample_rate: u32) -> Self {
        let mut filter = Self::default();
        filter.set(settings, sample_rate);
        filter
    }

    /// Changes the settings, keeping the state so the sound does not click.
    pub fn set(&mut self, settings: FilterSettings, sample_rate: u32) {
        let ratio = settings.cutoff / sample_rate as f32;
        let ratio = if ratio > 0.0 {
            ratio.min(MAXIMUM_RATIO)
        } else {
            0.0
        };
        let resonance = if settings.resonance > 0.0 {
            settings.resonance.min(1.0)
        } else {
            0.0
        };
        let damping = 2.0 - resonance * (2.0 - MINIMUM_DAMPING);
        let gain = sine_f32(ratio * 0.5) / sine_f32(0.25 - ratio * 0.5);
        let a1 = 1.0 / (1.0 + gain * (gain + damping));
        let a2 = gain * a1;
        let a3 = gain * a2;

        self.mode = settings.mode;
        self.coefficients = [
            R::coefficient(a1),
            R::coefficient(a2),
            R::coefficient(a3),
            R::coefficient(damping * 0.5),
        ];
    }

    /// Returns the output in use.
    pub fn mode(&self) -> FilterMode {
        self.mode
    }

    /// Clears the state, as if the filter had only ever received silence.
    pub fn reset(&mut self) {
        self.state = [R::SILENCE; 2];
    }

    /// Filters one value.
    ///
    /// In [`Fixed`](nanoaudio_dsp::Fixed), the output saturates at full scale, which a strong resonance can reach.
    #[inline]
    pub fn process(&mut self, value: R::Value) -> R::Value {
        R::filter_output(R::filter(&mut self.state, &self.coefficients, value, self.mode))
    }

    #[inline(always)]
    pub(crate) fn process_scaled(&mut self, value: R::Value, amplitude: i32) -> R::Value {
        R::filter_output_scaled(
            R::filter(&mut self.state, &self.coefficients, value, self.mode),
            amplitude,
        )
    }
}

#[cfg(test)]
mod tests {
    use nanoaudio_dsp::{Fixed, Float, Representation};

    use super::*;
    use crate::{Oscillator, Waveform, note_increment};

    extern crate std;

    fn gain<R: Synthesis>(settings: FilterSettings, frequency: f32) -> f64 {
        let sample_rate = 48_000;
        let mut filter = Filter::<R>::new(settings, sample_rate);
        let mut oscillator = Oscillator::new(Waveform::Sine);
        oscillator.set_frequency(frequency, sample_rate);
        let samples = if cfg!(miri) { 2_000 } else { 48_000 };
        let mut peak = 0.0f64;

        for index in 0..samples {
            let input = R::from_sample(oscillator.next_sample() / 32);
            let output = R::to_sample::<f32>(filter.process(input)) as f64;

            if index >= samples / 2 {
                peak = peak.max(output.abs());
            }
        }

        20.0 * (peak * 32_768.0 / 1_023.0).log10()
    }

    fn responses<R: Synthesis>() {
        let low = FilterSettings::low_pass(1_000.0, 0.0);
        let high = FilterSettings::new(FilterMode::HighPass, 1_000.0, 0.0);
        let band = FilterSettings::new(FilterMode::BandPass, 1_000.0, 0.0);
        let resonant = FilterSettings::low_pass(1_000.0, 1.0);

        assert!(gain::<R>(low, 100.0).abs() < 0.2);
        assert!((gain::<R>(low, 1_000.0) + 6.0).abs() < 0.3);
        assert!((gain::<R>(low, 10_000.0) + 42.8).abs() < 0.5);
        assert!(gain::<R>(high, 100.0) < -38.0);
        assert!(gain::<R>(high, 10_000.0).abs() < 0.2);
        assert!(gain::<R>(band, 100.0) < -18.0);
        assert!((gain::<R>(band, 1_000.0) + 6.0).abs() < 0.3);
        assert!((gain::<R>(resonant, 1_000.0) - 24.1).abs() < 0.3);
    }

    #[test]
    fn fixed_responses() {
        responses::<Fixed>();
    }

    #[test]
    fn float_responses() {
        responses::<Float>();
    }

    #[test]
    fn fixed_does_not_overflow_at_extremes() {
        let sample_rate = 44_100;
        let cutoffs = [0.0, 1.0, 20.0, 200.0, 2_000.0, 11_025.0, 21_000.0, 40_000.0, f32::NAN];
        let samples = if cfg!(miri) { 64 } else { 8_192 };

        for mode in [FilterMode::LowPass, FilterMode::BandPass, FilterMode::HighPass] {
            for cutoff in cutoffs {
                for resonance in [-1.0, 0.0, 0.5, 1.0, 2.0] {
                    for waveform in [Waveform::Square, Waveform::Saw, Waveform::Noise] {
                        let settings = FilterSettings::new(mode, cutoff, resonance);
                        let mut filter = Filter::<Fixed>::new(settings, sample_rate);
                        let mut oscillator = Oscillator::new(waveform);
                        let frequency = if cutoff > 0.0 { cutoff.min(20_000.0) } else { 50.0 };
                        oscillator.set_frequency(frequency, sample_rate);

                        for _ in 0..samples {
                            let sample = oscillator.next_sample().saturating_mul(2);
                            filter.process(Fixed::from_sample(sample));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fixed_survives_fast_modulation() {
        let sample_rate = 44_100;
        let mut filter = Filter::<Fixed>::new(FilterSettings::low_pass(100.0, 1.0), sample_rate);
        let mut oscillator = Oscillator::new(Waveform::Square);
        oscillator.set_increment(note_increment(40 << 8, sample_rate));
        let mut state = 0x1234_5678u32;
        let blocks = if cfg!(miri) { 16 } else { 4_096 };

        for _ in 0..blocks {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let cutoff = 20.0 + (state % 21_000) as f32;
            filter.set(FilterSettings::low_pass(cutoff, 1.0), sample_rate);

            for _ in 0..4 {
                filter.process(Fixed::from_sample(oscillator.next_sample()));
            }
        }
    }
}
