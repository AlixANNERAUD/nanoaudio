use crate::{
    ANALYSIS_4, ANALYSIS_8, ANALYSIS_FRACTION, COSINE_FRACTION, MAXIMUM_SUBBANDS, PARTIAL_FRACTION,
    PROTOTYPE_4, PROTOTYPE_8, PROTOTYPE_FRACTION, SUBBAND_FRACTION, SYNTHESIS_4, SYNTHESIS_8,
    SYNTHESIS_BLOCKS, Synthesis, WINDOW,
};

const PARTIAL_SPLIT: u32 = 14;

type Halves = [i32; 2];

const fn halves(value: i32, bits: u32) -> Halves {
    let low = (value << (32 - bits)) >> (32 - bits);

    [(value - low) >> bits, low]
}

const fn split<const SIZE: usize>(values: &[i32], bits: u32) -> [Halves; SIZE] {
    let mut result = [[0; 2]; SIZE];
    let mut index = 0;

    while index < SIZE {
        result[index] = halves(values[index], bits);
        index += 1;
    }

    result
}

const fn unique_row(row: usize, subbands: usize) -> usize {
    if row < subbands / 2 {
        row
    } else {
        row + subbands / 2 + 1
    }
}

const fn synthesis<const SIZE: usize>(matrix: &[i32]) -> [Halves; SIZE] {
    let subbands = (2 * SIZE).isqrt();
    let mut result = [[0; 2]; SIZE];
    let mut index = 0;

    while index < SIZE {
        let row = unique_row(index / (subbands / 2), subbands);

        result[index] = halves(matrix[row * subbands + index % (subbands / 2)], 16);
        index += 1;
    }

    result
}

const fn analysis<const SIZE: usize>(matrix: &[i32]) -> [Halves; SIZE] {
    let subbands = (2 * SIZE).isqrt();
    let mut result = [[0; 2]; SIZE];
    let mut index = 0;

    while index < SIZE {
        let (subband, position) = (index / subbands, index % subbands);
        let column = if position < subbands / 2 {
            2 * position
        } else {
            2 * (position - subbands / 2) + 1
        };

        result[index] = halves(matrix[2 * subbands * subband + column + subbands / 2], 16);
        index += 1;
    }

    result
}

const SYNTHESIS_4_SPLIT: [Halves; 8] = synthesis(&SYNTHESIS_4);
const SYNTHESIS_8_SPLIT: [Halves; 32] = synthesis(&SYNTHESIS_8);
const ANALYSIS_4_SPLIT: [Halves; 8] = analysis(&ANALYSIS_4);
const ANALYSIS_8_SPLIT: [Halves; 32] = analysis(&ANALYSIS_8);
const PROTOTYPE_4_SPLIT: [Halves; 40] = split(&PROTOTYPE_4, 16);
const PROTOTYPE_8_SPLIT: [Halves; 80] = split(&PROTOTYPE_8, 16);
const PROTOTYPE_4_PARTIAL: [Halves; 40] = split(&PROTOTYPE_4, PARTIAL_SPLIT);
const PROTOTYPE_8_PARTIAL: [Halves; 80] = split(&PROTOTYPE_8, PARTIAL_SPLIT);

#[derive(Default)]
struct Sum {
    high: i32,
    low: u32,
}

impl Sum {
    #[inline(always)]
    fn add(&mut self, coefficient: Halves, value: i32) {
        let (high, low) = (coefficient[0], coefficient[1]);
        let (value_high, value_low) = (value >> 16, value & 0xFFFF);
        let middle = high * value_low + low * value_high;
        let product = low * value_low;
        let (sum, first) = self.low.overflowing_add((middle << 16) as u32);
        let (sum, second) = sum.overflowing_add(product as u32);

        self.low = sum;
        self.high = self
            .high
            .wrapping_add(high * value_high)
            .wrapping_add(middle >> 16)
            .wrapping_add(product >> 31)
            .wrapping_add(first as i32)
            .wrapping_add(second as i32);
    }

    #[inline(always)]
    fn total(&self) -> i64 {
        ((self.high as i64) << 32) | self.low as i64
    }
}

pub(crate) fn synthesize(state: &mut Synthesis, samples: &[i32], output: &mut [i16]) {
    if samples.len() == 4 {
        synthesize_with::<4>(
            state,
            samples,
            output,
            &SYNTHESIS_4_SPLIT,
            &PROTOTYPE_4_SPLIT,
        );
    } else {
        synthesize_with::<8>(
            state,
            samples,
            output,
            &SYNTHESIS_8_SPLIT,
            &PROTOTYPE_8_SPLIT,
        );
    }
}

#[inline(always)]
fn synthesize_with<const SUBBANDS: usize>(
    state: &mut Synthesis,
    samples: &[i32],
    output: &mut [i16],
    matrix: &[Halves],
    prototype: &[Halves],
) {
    let samples = &samples[..SUBBANDS];
    let output = &mut output[..SUBBANDS];
    let modulate =
        |sum: i64| (sum >> COSINE_FRACTION).clamp(i32::MIN as i64, i32::MAX as i64) as i32;

    let offsets = state.advance(SUBBANDS);
    let newest = &mut state.values[offsets[0]..][..2 * SUBBANDS];
    let mut sums = [0i32; MAXIMUM_SUBBANDS / 2];
    let mut differences = [0i32; MAXIMUM_SUBBANDS / 2];

    for index in 0..SUBBANDS / 2 {
        let (left, right) = (samples[index], samples[SUBBANDS - 1 - index]);

        sums[index] = left + right;
        differences[index] = left - right;
    }

    for (row, coefficients) in matrix.chunks_exact(SUBBANDS / 2).enumerate() {
        let row = unique_row(row, SUBBANDS);
        let values = if (row + SUBBANDS / 2).is_multiple_of(2) {
            &sums
        } else {
            &differences
        };
        let mut sum = Sum::default();

        for (coefficient, value) in coefficients.iter().zip(values) {
            sum.add(*coefficient, *value);
        }

        let sum = sum.total();

        newest[row] = modulate(sum);

        if row < SUBBANDS / 2 {
            newest[SUBBANDS - row] = modulate(-sum);
        } else {
            newest[3 * SUBBANDS - row] = newest[row];
        }
    }

    newest[SUBBANDS / 2] = 0;

    let mut window = [0i32; SYNTHESIS_BLOCKS * MAXIMUM_SUBBANDS];

    for (block, offset) in window.as_chunks_mut::<SUBBANDS>().0.iter_mut().zip(offsets) {
        block.copy_from_slice(&state.values[offset..][..SUBBANDS]);
    }

    let shift = PROTOTYPE_FRACTION + SUBBAND_FRACTION - SUBBANDS.trailing_zeros();

    for (index, output) in output.iter_mut().enumerate() {
        let value =
            (-convolve::<SUBBANDS>(&window, prototype, index) + (1 << (shift - 1))) >> shift;

        *output = value.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
    }
}

#[inline(never)]
fn convolve<const SUBBANDS: usize>(
    window: &[i32; SYNTHESIS_BLOCKS * MAXIMUM_SUBBANDS],
    prototype: &[Halves],
    index: usize,
) -> i64 {
    let mut sum = Sum::default();

    for block in 0..SYNTHESIS_BLOCKS {
        let position = block * SUBBANDS + index;

        sum.add(prototype[position], window[position]);
    }

    sum.total()
}

pub(crate) fn analyze(
    history: &mut [i16; WINDOW],
    input: impl Iterator<Item = i16>,
    output: &mut [i32],
) {
    if output.len() == 4 {
        analyze_with::<4>(
            history,
            input,
            output,
            &PROTOTYPE_4_PARTIAL,
            &ANALYSIS_4_SPLIT,
        );
    } else {
        analyze_with::<8>(
            history,
            input,
            output,
            &PROTOTYPE_8_PARTIAL,
            &ANALYSIS_8_SPLIT,
        );
    }
}

#[inline(always)]
fn analyze_with<const SUBBANDS: usize>(
    history: &mut [i16; WINDOW],
    input: impl Iterator<Item = i16>,
    output: &mut [i32],
    prototype: &[Halves],
    matrix: &[Halves],
) {
    let output = &mut output[..SUBBANDS];

    history.copy_within(..9 * SUBBANDS, SUBBANDS);

    for (index, sample) in input.enumerate() {
        history[SUBBANDS - 1 - index] = sample;
    }

    let mut partial = [0i32; 2 * MAXIMUM_SUBBANDS];

    for (row, partial) in partial[..2 * SUBBANDS].iter_mut().enumerate() {
        let (mut high, mut low) = (0i32, 0i32);

        for index in (row..10 * SUBBANDS).step_by(2 * SUBBANDS) {
            let sample = history[index] as i32;

            high += prototype[index][0] * sample;
            low += prototype[index][1] * sample;
        }

        let shift = PROTOTYPE_FRACTION - PARTIAL_FRACTION;

        *partial = (high >> (shift - PARTIAL_SPLIT))
            + ((((high & ((1 << (shift - PARTIAL_SPLIT)) - 1)) << PARTIAL_SPLIT) + low) >> shift);
    }

    let mut folded = [[0i32; MAXIMUM_SUBBANDS / 2]; 2];

    for column in 0..SUBBANDS {
        folded[column % 2][column / 2] = if column == 0 {
            partial[SUBBANDS / 2]
        } else if column <= SUBBANDS / 2 {
            partial[SUBBANDS / 2 + column] + partial[SUBBANDS / 2 - column]
        } else {
            partial[SUBBANDS / 2 + column] - partial[5 * SUBBANDS / 2 - column]
        };
    }

    let shift = COSINE_FRACTION + PARTIAL_FRACTION - ANALYSIS_FRACTION;

    for (subband, coefficients) in matrix.as_chunks::<SUBBANDS>().0.iter().enumerate() {
        let (even_coefficients, odd_coefficients) = coefficients.split_at(SUBBANDS / 2);
        let (mut even_sum, mut odd_sum) = (Sum::default(), Sum::default());

        for (coefficient, value) in even_coefficients.iter().zip(&folded[0]) {
            even_sum.add(*coefficient, *value);
        }

        for (coefficient, value) in odd_coefficients.iter().zip(&folded[1]) {
            odd_sum.add(*coefficient, *value);
        }

        let (even_sum, odd_sum) = (even_sum.total(), odd_sum.total());

        output[subband] = ((even_sum + odd_sum) >> shift) as i32;
        output[SUBBANDS - 1 - subband] = ((even_sum - odd_sum) >> shift) as i32;
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
    fn split_products_fit_in_32_bits() {
        for table in [
            &SYNTHESIS_4_SPLIT[..],
            &SYNTHESIS_8_SPLIT,
            &ANALYSIS_4_SPLIT,
            &ANALYSIS_8_SPLIT,
            &PROTOTYPE_4_SPLIT,
            &PROTOTYPE_8_SPLIT,
        ] {
            for [high, low] in table {
                let (high, low) = (*high as i64, *low as i64);

                assert!(high.abs() * 0xFFFF + low.abs() * 0x8000 <= i32::MAX as i64);
                assert!(low.abs() * 0xFFFF <= i32::MAX as i64);
            }
        }

        for (prototype, split) in [
            (&PROTOTYPE_4[..], &PROTOTYPE_4_PARTIAL[..]),
            (&PROTOTYPE_8, &PROTOTYPE_8_PARTIAL),
        ] {
            let subbands = prototype.len() / 10;

            for (value, [high, low]) in prototype.iter().zip(split) {
                assert_eq!((high << PARTIAL_SPLIT) + low, *value);
            }

            for row in 0..2 * subbands {
                let (high, low) = (row..10 * subbands)
                    .step_by(2 * subbands)
                    .map(|index| split[index])
                    .fold((0i64, 0i64), |(high, low), [h, l]| {
                        (high + (h as i64).abs(), low + (l as i64).abs())
                    });

                assert!(high << 15 <= i32::MAX as i64);
                assert!(low << 15 <= i32::MAX as i64);
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
                let mut state = Synthesis::new();

                if round % 2 == 0 {
                    state
                        .values
                        .iter_mut()
                        .for_each(|value| *value = random.state());
                    state.newest = round / 2 % SYNTHESIS_BLOCKS;
                }

                let mut expected_state = state.clone();

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
