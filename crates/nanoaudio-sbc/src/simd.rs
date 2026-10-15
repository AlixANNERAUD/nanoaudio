use wide::{i32x4, i64x4};

use crate::{
    ANALYSIS_4, ANALYSIS_8, ANALYSIS_FRACTION, COSINE_FRACTION, MAXIMUM_SUBBANDS, PARTIAL_FRACTION,
    PROTOTYPE_4, PROTOTYPE_8, PROTOTYPE_FRACTION, SUBBAND_FRACTION, SYNTHESIS, SYNTHESIS_4,
    SYNTHESIS_8, WINDOW,
};

const fn transpose<const SIZE: usize>(matrix: &[i32; SIZE], rows: usize) -> [i32; SIZE] {
    let columns = SIZE / rows;
    let mut transposed = [0; SIZE];
    let mut index = 0;

    while index < SIZE {
        transposed[(index % columns) * rows + index / columns] = matrix[index];
        index += 1;
    }

    transposed
}

const SYNTHESIS_4_COLUMNS: [i32; 32] = transpose(&SYNTHESIS_4, 8);
const SYNTHESIS_8_COLUMNS: [i32; 128] = transpose(&SYNTHESIS_8, 16);
const ANALYSIS_4_COLUMNS: [i32; 32] = transpose(&ANALYSIS_4, 4);
const ANALYSIS_8_COLUMNS: [i32; 128] = transpose(&ANALYSIS_8, 8);

#[inline(always)]
fn load(values: &[i32], offset: usize) -> i32x4 {
    i32x4::new(*values[offset..].first_chunk().unwrap())
}

pub(crate) fn synthesize(state: &mut [i32; SYNTHESIS], samples: &[i32], output: &mut [i16]) {
    if samples.len() == 4 {
        synthesize_with::<4>(state, samples, output, &SYNTHESIS_4_COLUMNS, &PROTOTYPE_4);
    } else {
        synthesize_with::<8>(state, samples, output, &SYNTHESIS_8_COLUMNS, &PROTOTYPE_8);
    }
}

#[inline(always)]
fn synthesize_with<const SUBBANDS: usize>(
    state: &mut [i32; SYNTHESIS],
    samples: &[i32],
    output: &mut [i16],
    matrix: &[i32],
    prototype: &[i32],
) {
    let samples = &samples[..SUBBANDS];
    let output = &mut output[..SUBBANDS];

    state.copy_within(..18 * SUBBANDS, 2 * SUBBANDS);

    let mut sums = [i64x4::ZERO; MAXIMUM_SUBBANDS / 2];

    for (column, sample) in samples.iter().enumerate() {
        let sample = i32x4::splat(*sample);

        for (lane, sum) in sums[..SUBBANDS / 2].iter_mut().enumerate() {
            *sum += load(matrix, 2 * SUBBANDS * column + 4 * lane).widening_mul(sample);
        }
    }

    for (values, sum) in state
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(&sums[..SUBBANDS / 2])
    {
        *values = (*sum >> COSINE_FRACTION)
            .max(const { i64x4::splat(i32::MIN as i64) })
            .min(const { i64x4::splat(i32::MAX as i64) })
            .to_array()
            .map(|value| value as i32);
    }

    let shift = PROTOTYPE_FRACTION + SUBBAND_FRACTION - SUBBANDS.trailing_zeros();
    let rounding = const {
        i64x4::splat(1 << (PROTOTYPE_FRACTION + SUBBAND_FRACTION - SUBBANDS.trailing_zeros() - 1))
    };

    for (lane, output) in output.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let mut sum = i64x4::ZERO;

        for group in 0..5 {
            let base = 4 * SUBBANDS * group + 4 * lane;
            let position = 2 * SUBBANDS * group + 4 * lane;

            sum += load(state, base).widening_mul(load(prototype, position));
            sum +=
                load(state, base + 3 * SUBBANDS).widening_mul(load(prototype, position + SUBBANDS));
        }

        *output = ((rounding - sum) >> shift)
            .max(const { i64x4::splat(i16::MIN as i64) })
            .min(const { i64x4::splat(i16::MAX as i64) })
            .to_array()
            .map(|value| value as i16);
    }
}

pub(crate) fn analyze(
    history: &mut [i16; WINDOW],
    input: impl Iterator<Item = i16>,
    output: &mut [i32],
) {
    if output.len() == 4 {
        analyze_with::<4>(history, input, output, &PROTOTYPE_4, &ANALYSIS_4_COLUMNS);
    } else {
        analyze_with::<8>(history, input, output, &PROTOTYPE_8, &ANALYSIS_8_COLUMNS);
    }
}

#[inline(always)]
fn analyze_with<const SUBBANDS: usize>(
    history: &mut [i16; WINDOW],
    input: impl Iterator<Item = i16>,
    output: &mut [i32],
    prototype: &[i32],
    matrix: &[i32],
) {
    let output = &mut output[..SUBBANDS];

    history.copy_within(..9 * SUBBANDS, SUBBANDS);

    for (index, sample) in input.enumerate() {
        history[SUBBANDS - 1 - index] = sample;
    }

    let mut partial = [0i32; 2 * MAXIMUM_SUBBANDS];

    for (lane, partial) in partial.as_chunks_mut::<4>().0[..SUBBANDS / 2]
        .iter_mut()
        .enumerate()
    {
        let mut sum = i64x4::ZERO;

        for group in 0..5 {
            let position = 2 * SUBBANDS * group + 4 * lane;
            let samples = history[position..].first_chunk::<4>().unwrap();

            sum += load(prototype, position).widening_mul(i32x4::new(samples.map(i32::from)));
        }

        *partial = (sum >> (PROTOTYPE_FRACTION - PARTIAL_FRACTION))
            .to_array()
            .map(|value| value as i32);
    }

    let mut sums = [i64x4::ZERO; MAXIMUM_SUBBANDS / 4];

    for (column, partial) in partial[..2 * SUBBANDS].iter().enumerate() {
        let partial = i32x4::splat(*partial);

        for (lane, sum) in sums[..SUBBANDS / 4].iter_mut().enumerate() {
            *sum += load(matrix, SUBBANDS * column + 4 * lane).widening_mul(partial);
        }
    }

    for (output, sum) in output.as_chunks_mut::<4>().0.iter_mut().zip(sums) {
        *output = (sum >> (COSINE_FRACTION + PARTIAL_FRACTION - ANALYSIS_FRACTION))
            .to_array()
            .map(|value| value as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUNDS: usize = if cfg!(miri) { 8 } else { 20_000 };

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;

            (self.0 >> 32) as u32
        }

        fn sample(&mut self) -> i16 {
            match self.next() % 8 {
                0 => i16::MIN,
                1 => i16::MAX,
                2 => (self.next() % 64) as i16 - 32,
                _ => self.next() as i16,
            }
        }

        fn subband(&mut self) -> i32 {
            const LIMIT: i32 = 1 << 28;

            match self.next() % 8 {
                0 => -LIMIT,
                1 => LIMIT,
                2 => (self.next() % 64) as i32 - 32,
                3 => (self.next() as i32) >> 8,
                _ => (self.next() as i32) >> 3,
            }
        }

        fn state(&mut self) -> i32 {
            match self.next() % 8 {
                0 => i32::MIN,
                1 => i32::MAX,
                2 => (self.next() % 64) as i32 - 32,
                _ => self.next() as i32,
            }
        }
    }

    #[test]
    fn partial_sums_fit_in_32_bits() {
        for prototype in [&PROTOTYPE_4[..], &PROTOTYPE_8[..]] {
            let subbands = prototype.len() / 10;

            for row in 0..2 * subbands {
                let sum: i64 = (row..10 * subbands)
                    .step_by(2 * subbands)
                    .map(|index| (prototype[index] as i64).abs() << 15)
                    .sum();

                assert!(sum >> (PROTOTYPE_FRACTION - PARTIAL_FRACTION) <= i32::MAX as i64);
            }
        }
    }

    #[test]
    fn analysis_matches_scalar() {
        let mut random = Random(0x9E37_79B9_7F4A_7C15);

        for subbands in [4, 8] {
            let mut history = [0i16; WINDOW];
            let mut expected_history = history;

            for _ in 0..ROUNDS {
                let mut input = [0i16; MAXIMUM_SUBBANDS];
                input
                    .iter_mut()
                    .for_each(|sample| *sample = random.sample());
                let input = &input[..subbands];

                let mut output = [0i32; MAXIMUM_SUBBANDS];
                let mut expected = [0i32; MAXIMUM_SUBBANDS];

                analyze(&mut history, input.iter().copied(), &mut output[..subbands]);
                crate::analyze(
                    &mut expected_history,
                    input.iter().copied(),
                    &mut expected[..subbands],
                );

                assert_eq!(output, expected);
                assert_eq!(history, expected_history);
            }
        }
    }

    #[test]
    fn synthesis_matches_scalar() {
        let mut random = Random(0x2545_F491_4F6C_DD1D);

        for subbands in [4, 8] {
            for round in 0..ROUNDS {
                let mut state = [0i32; SYNTHESIS];

                if round % 2 == 0 {
                    state.iter_mut().for_each(|value| *value = random.state());
                }

                let mut expected_state = state;

                for _ in 0..4 {
                    let mut samples = [0i32; MAXIMUM_SUBBANDS];
                    samples
                        .iter_mut()
                        .for_each(|sample| *sample = random.subband());

                    let mut output = [0i16; MAXIMUM_SUBBANDS];
                    let mut expected = [0i16; MAXIMUM_SUBBANDS];

                    synthesize(&mut state, &samples[..subbands], &mut output[..subbands]);
                    crate::synthesize(
                        &mut expected_state,
                        &samples[..subbands],
                        &mut expected[..subbands],
                    );

                    assert_eq!(output, expected);
                    assert_eq!(state, expected_state);
                }
            }
        }
    }
}
