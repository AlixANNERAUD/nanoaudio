use wide::{i32x4, i32x8};

use crate::{FILTER, LOW_DECISION_LEVELS};

const fn analysis_filters() -> [[[i32; 8]; 3]; 2] {
    let mut filters = [[[0; 8]; 3]; 2];
    let mut index = 0;

    while index < 24 {
        let age = 23 - index;

        filters[age % 2][index / 8][index % 8] = FILTER[age];
        index += 1;
    }

    filters
}

const fn synthesis_filters() -> [[i32; 12]; 2] {
    let mut filters = [[0; 12]; 2];
    let mut index = 0;

    while index < 12 {
        let age = 11 - index;

        filters[0][index] = FILTER[2 * age];
        filters[1][index] = FILTER[2 * age + 1];
        index += 1;
    }

    filters
}

const fn decision_levels() -> [[i32; 8]; 4] {
    let mut levels = [[0; 8]; 4];
    let mut index = 1;

    while index < LOW_DECISION_LEVELS.len() {
        levels[(index - 1) / 8][(index - 1) % 8] = LOW_DECISION_LEVELS[index];
        index += 1;
    }

    levels
}

const ANALYSIS: [[[i32; 8]; 3]; 2] = analysis_filters();
const SYNTHESIS: [[i32; 12]; 2] = synthesis_filters();
const DECISION_LEVELS: [[i32; 8]; 4] = decision_levels();

#[inline]
pub(crate) fn analysis(history: &[i32; 24]) -> (i32, i32) {
    let mut even = i32x8::ZERO;
    let mut odd = i32x8::ZERO;

    for ((samples, even_filter), odd_filter) in history
        .as_chunks::<8>()
        .0
        .iter()
        .zip(&ANALYSIS[0])
        .zip(&ANALYSIS[1])
    {
        let samples = i32x8::from(*samples);

        even += samples * i32x8::from(*even_filter);
        odd += samples * i32x8::from(*odd_filter);
    }

    (even.reduce_add(), odd.reduce_add())
}

#[inline]
fn dot(values: &[i32; 12], filter: &[i32; 12]) -> i32 {
    let head = i32x8::new(core::array::from_fn(|index| values[index]))
        * i32x8::new(core::array::from_fn(|index| filter[index]));
    let tail = i32x4::new(core::array::from_fn(|index| values[8 + index]))
        * i32x4::new(core::array::from_fn(|index| filter[8 + index]));

    head.reduce_add() + tail.reduce_add()
}

#[inline]
pub(crate) fn synthesis(differences: &[i32; 12], sums: &[i32; 12]) -> (i32, i32) {
    (dot(differences, &SYNTHESIS[0]), dot(sums, &SYNTHESIS[1]))
}

#[inline]
pub(crate) fn interval(magnitude: i32, scale: i32) -> u8 {
    let magnitude = i32x8::splat(magnitude);
    let scale = i32x8::splat(scale);
    let mut below = i32x8::ZERO;

    for levels in DECISION_LEVELS {
        below += magnitude.simd_lt((i32x8::from(levels) * scale) >> 12);
    }

    (LOW_DECISION_LEVELS.len() as i32 + below.reduce_add()) as u8
}

macro_rules! qmf_analysis {
    ($history:expr) => {
        $crate::simd::analysis(&$history)
    };
}

macro_rules! qmf_synthesis {
    ($differences:expr, $sums:expr) => {
        $crate::simd::synthesis(&$differences, &$sums)
    };
}

macro_rules! low_interval {
    ($magnitude:expr, $scale:expr) => {
        $crate::simd::interval($magnitude, $scale)
    };
}

pub(crate) use {low_interval, qmf_analysis, qmf_synthesis};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scalar;

    struct Random(u32);

    impl Random {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0
        }

        fn sample(&mut self, extremes: &[i32]) -> i32 {
            let value = self.next();

            match value % 4 {
                0 => extremes[(value >> 8) as usize % extremes.len()],
                _ => (value >> 16) as i16 as i32,
            }
        }
    }

    fn iterations() -> usize {
        if cfg!(miri) { 64 } else { 20_000 }
    }

    #[test]
    fn analysis_matches_scalar() {
        let mut random = Random(0x1234_5678);
        let extremes = [i16::MIN as i32, i16::MAX as i32, -1, 0, 1];

        for _ in 0..iterations() {
            let history = core::array::from_fn(|_| random.sample(&extremes));

            assert_eq!(analysis(&history), scalar::qmf_analysis!(history));
        }

        for (low, high) in [(i16::MIN, i16::MAX), (i16::MAX, i16::MIN)] {
            let history = core::array::from_fn(|index| {
                i32::from(if FILTER[23 - index] < 0 { low } else { high })
            });

            assert_eq!(analysis(&history), scalar::qmf_analysis!(history));
        }
    }

    #[test]
    fn synthesis_matches_scalar() {
        let mut random = Random(0x9E37_79B9);
        let extremes = [-32_768, 32_767, -1, 0, 1];

        for _ in 0..iterations() {
            let differences = core::array::from_fn(|_| random.sample(&extremes));
            let sums = core::array::from_fn(|_| random.sample(&extremes));

            assert_eq!(
                synthesis(&differences, &sums),
                scalar::qmf_synthesis!(differences, sums)
            );
        }
    }

    #[test]
    fn interval_matches_scalar() {
        let step = if cfg!(miri) { 4_099 } else { 7 };
        let scales = (0..=16_384)
            .step_by(if cfg!(miri) { 4_093 } else { 97 })
            .chain([1, 8, 32, 16_032, 16_383, 16_384, 32_767]);

        for scale in scales {
            let boundaries = LOW_DECISION_LEVELS.iter().flat_map(|level| {
                let threshold = (level * scale) >> 12;

                [threshold - 1, threshold, threshold + 1].map(|value| value.max(0))
            });

            for magnitude in (0..=32_768)
                .step_by(step)
                .chain([0, 1, 32_767, 32_768])
                .chain(boundaries)
            {
                assert_eq!(
                    interval(magnitude, scale),
                    scalar::low_interval!(magnitude, scale),
                    "{magnitude} {scale}"
                );
            }
        }
    }
}
