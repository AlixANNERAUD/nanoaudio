use nanoaudio_dsp::{Fixed, Float, Representation};

use crate::FilterMode;

/// A [`Representation`] the synthesizer runs in: [`Fixed`] or [`Float`].
///
/// Oscillators and envelopes work in integers for both, so the two only differ in the filter and the voice gain.
pub trait Synthesis: Representation + sealed::Sealed {}

impl Synthesis for Fixed {}

impl Synthesis for Float {}

pub(crate) mod sealed {
    use super::*;

    pub trait Sealed: Representation {
        type Coefficient: Copy + Default + core::fmt::Debug + PartialEq;

        fn coefficient(value: f32) -> Self::Coefficient;

        fn amplify_oscillator(sample: i16, amplitude: i32) -> Self::Value;

        fn from_oscillator(sample: i16) -> Self::Value;

        fn filter(
            state: &mut [Self::Value; 2],
            coefficients: &[Self::Coefficient; 4],
            input: Self::Value,
            mode: FilterMode,
        ) -> Self::Value;

        fn filter_output(raw: Self::Value) -> Self::Value;

        fn filter_output_scaled(raw: Self::Value, amplitude: i32) -> Self::Value;
    }
}

const FILTER_HEADROOM: u32 = 8;

const FILTER_MAXIMUM: i32 = (1 << (31 - FILTER_HEADROOM)) - 1;

#[cfg(not(nanoaudio_synth_split))]
#[inline(always)]
pub(crate) fn multiply_q15(value: i32, coefficient: i32) -> i32 {
    ((value as i64 * coefficient as i64) >> 15) as i32
}

#[cfg(nanoaudio_synth_split)]
#[inline(always)]
pub(crate) fn multiply_q15(value: i32, coefficient: i32) -> i32 {
    (value >> 15) * coefficient + (((value & 0x7FFF) * coefficient) >> 15)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scaled {
    mantissa: i32,
    shift: u32,
}

impl Scaled {
    const MAXIMUM_SHIFT: u32 = 24;

    fn new(value: f32) -> Self {
        if !(value > 0.0) {
            return Self::default();
        }

        let mut scaled = value * 32_768.0;
        let mut shift = 0;

        while scaled < 16_384.0 && shift < Self::MAXIMUM_SHIFT {
            scaled *= 2.0;
            shift += 1;
        }

        Self {
            mantissa: (scaled + 0.5).min(32_767.0) as i32,
            shift,
        }
    }

    #[inline(always)]
    fn apply(self, value: i32) -> i32 {
        multiply_q15(value, self.mantissa) >> self.shift
    }
}

impl sealed::Sealed for Fixed {
    type Coefficient = Scaled;

    fn coefficient(value: f32) -> Scaled {
        Scaled::new(value)
    }

    #[inline(always)]
    fn amplify_oscillator(sample: i16, amplitude: i32) -> i32 {
        (sample as i32 * amplitude) << 1
    }

    #[inline(always)]
    fn from_oscillator(sample: i16) -> i32 {
        (sample as i32) << 16
    }

    #[inline(always)]
    fn filter(
        state: &mut [i32; 2],
        coefficients: &[Scaled; 4],
        input: i32,
        mode: FilterMode,
    ) -> i32 {
        let [a1, a2, a3, damping] = *coefficients;
        let [first, second] = *state;
        let input = input >> FILTER_HEADROOM;
        let difference = input - second;
        let band = a1.apply(first) + a2.apply(difference);
        let low = second + a2.apply(first) + a3.apply(difference);
        state[0] = 2 * band - first;
        state[1] = 2 * low - second;

        match mode {
            FilterMode::LowPass => low,
            FilterMode::BandPass => band,
            FilterMode::HighPass => input - 2 * damping.apply(band) - low,
        }
    }

    #[inline(always)]
    fn filter_output(raw: i32) -> i32 {
        raw.clamp(-FILTER_MAXIMUM - 1, FILTER_MAXIMUM) << FILTER_HEADROOM
    }

    #[inline(always)]
    fn filter_output_scaled(raw: i32, amplitude: i32) -> i32 {
        Self::filter_output(multiply_q15(raw, amplitude))
    }
}

const Q15: f32 = 1.0 / 32_768.0;

const Q30: f32 = 1.0 / 1_073_741_824.0;

impl sealed::Sealed for Float {
    type Coefficient = f32;

    fn coefficient(value: f32) -> f32 {
        value
    }

    #[inline(always)]
    fn amplify_oscillator(sample: i16, amplitude: i32) -> f32 {
        (sample as i32 * amplitude) as f32 * Q30
    }

    #[inline(always)]
    fn from_oscillator(sample: i16) -> f32 {
        sample as f32 * Q15
    }

    #[inline(always)]
    fn filter(state: &mut [f32; 2], coefficients: &[f32; 4], input: f32, mode: FilterMode) -> f32 {
        let [a1, a2, a3, damping] = *coefficients;
        let [first, second] = *state;
        let difference = input - second;
        let band = a1 * first + a2 * difference;
        let low = second + a2 * first + a3 * difference;
        state[0] = 2.0 * band - first;
        state[1] = 2.0 * low - second;

        match mode {
            FilterMode::LowPass => low,
            FilterMode::BandPass => band,
            FilterMode::HighPass => input - 2.0 * damping * band - low,
        }
    }

    #[inline(always)]
    fn filter_output(raw: f32) -> f32 {
        raw
    }

    #[inline(always)]
    fn filter_output_scaled(raw: f32, amplitude: i32) -> f32 {
        raw * (amplitude as f32 * Q15)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_multiply_matches_wide_multiply() {
        let values = [i32::MIN, i32::MIN + 1, -65_537, -32_768, -1, 0, 1, 32_767, 65_536, i32::MAX];
        let coefficients = [-32_767, -16_384, -1, 0, 1, 2, 16_383, 32_767];

        for value in values {
            for coefficient in coefficients {
                let split = (value >> 15) * coefficient + (((value & 0x7FFF) * coefficient) >> 15);
                assert_eq!(multiply_q15(value, coefficient), split);
                assert_eq!(
                    split as i64,
                    (value as i64 * coefficient as i64) >> 15,
                    "{value} {coefficient}"
                );
            }
        }
    }

    #[test]
    fn scaled_coefficients_keep_their_precision() {
        for value in [1.0e-6f32, 3.0e-4, 0.01, 0.2, 0.5, 0.75, 0.999] {
            let scaled = Scaled::new(value);
            let actual = scaled.mantissa as f32 / (1u64 << (15 + scaled.shift)) as f32;

            assert!((actual - value).abs() <= value / 16_384.0, "{value} {actual}");
        }

        assert_eq!(Scaled::new(0.0), Scaled::default());
        assert_eq!(Scaled::new(f32::NAN), Scaled::default());
        assert_eq!(Scaled::new(1.0).mantissa, 32_767);
    }
}
