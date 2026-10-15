use wide::{i32x4, i64x4};

fn taps<const NEAR: usize, const BLOCKS: usize>(coefficients: &[i32]) -> [i32x4; BLOCKS] {
    core::array::from_fn(|block| {
        i32x4::new(core::array::from_fn(|lane| {
            let lag = NEAR + 4 * BLOCKS - 4 * block - lane;

            coefficients.get(lag - 1).copied().unwrap_or(0)
        }))
    })
}

fn narrow<const NEAR: usize, const BLOCKS: usize>(
    samples: &mut [i32],
    coefficients: &[i32],
    shift: u32,
) {
    let span = NEAR + 4 * BLOCKS;
    let start = span.min(samples.len());

    super::predict_narrow(&mut samples[..start], coefficients, shift);

    if start == samples.len() {
        return;
    }

    let taps = taps::<NEAR, BLOCKS>(coefficients);
    let near: [i32; NEAR] = core::array::from_fn(|lag| coefficients.get(lag).copied().unwrap_or(0));
    let mut history: [i32; NEAR] = core::array::from_fn(|lag| samples[start - 1 - lag]);

    for index in start..samples.len() {
        let (window, _) = samples[index - span..index - NEAR].as_chunks::<4>();
        let mut sum = i32x4::ZERO;

        for (chunk, tap) in window.iter().zip(&taps) {
            sum += i32x4::new(*chunk) * *tap;
        }

        let mut prediction = sum.reduce_add();

        for lag in (0..NEAR).rev() {
            prediction = prediction.wrapping_add(near[lag].wrapping_mul(history[lag]));
        }

        let value = samples[index].wrapping_add(prediction >> shift);

        samples[index] = value;
        history.copy_within(..NEAR - 1, 1);
        history[0] = value;
    }
}

fn wide<const NEAR: usize, const BLOCKS: usize>(
    samples: &mut [i32],
    coefficients: &[i32],
    shift: u32,
) {
    let span = NEAR + 4 * BLOCKS;
    let start = span.min(samples.len());

    super::predict_wide(&mut samples[..start], coefficients, shift);

    if start == samples.len() {
        return;
    }

    let taps = taps::<NEAR, BLOCKS>(coefficients);
    let near: [i64; NEAR] =
        core::array::from_fn(|lag| coefficients.get(lag).copied().unwrap_or(0).into());
    let mut history: [i64; NEAR] = core::array::from_fn(|lag| samples[start - 1 - lag].into());

    for index in start..samples.len() {
        let (window, _) = samples[index - span..index - NEAR].as_chunks::<4>();
        let mut sum = i64x4::ZERO;

        for (chunk, tap) in window.iter().zip(&taps) {
            sum += i32x4::new(*chunk).widening_mul(*tap);
        }

        let mut prediction = sum.reduce_add();

        for lag in (0..NEAR).rev() {
            prediction += near[lag] * history[lag];
        }

        let value = (i64::from(samples[index]) + (prediction >> shift)) as i32;

        samples[index] = value;
        history.copy_within(..NEAR - 1, 1);
        history[0] = value.into();
    }
}

pub(crate) fn predict_narrow(samples: &mut [i32], coefficients: &[i32], shift: u32) {
    match coefficients.len().saturating_sub(2).div_ceil(4) {
        0 => narrow::<2, 0>(samples, coefficients, shift),
        1 => narrow::<2, 1>(samples, coefficients, shift),
        2 => narrow::<2, 2>(samples, coefficients, shift),
        3 => narrow::<2, 3>(samples, coefficients, shift),
        4 => narrow::<2, 4>(samples, coefficients, shift),
        5 => narrow::<2, 5>(samples, coefficients, shift),
        6 => narrow::<2, 6>(samples, coefficients, shift),
        7 => narrow::<2, 7>(samples, coefficients, shift),
        _ => narrow::<2, 8>(samples, coefficients, shift),
    }
}

pub(crate) fn predict_wide(samples: &mut [i32], coefficients: &[i32], shift: u32) {
    match coefficients.len() {
        0..=4 => wide::<4, 0>(samples, coefficients, shift),
        order => match (order - 2).div_ceil(4) {
            1 => wide::<2, 1>(samples, coefficients, shift),
            2 => wide::<2, 2>(samples, coefficients, shift),
            3 => wide::<2, 3>(samples, coefficients, shift),
            4 => wide::<2, 4>(samples, coefficients, shift),
            5 => wide::<2, 5>(samples, coefficients, shift),
            6 => wide::<2, 6>(samples, coefficients, shift),
            7 => wide::<2, 7>(samples, coefficients, shift),
            _ => wide::<2, 8>(samples, coefficients, shift),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);

            (self.0 >> 32) as u32
        }

        fn below(&mut self, bound: u32) -> u32 {
            self.next() % bound
        }

        fn signed(&mut self, bits: u32) -> i32 {
            (self.next() as i32) >> (32 - bits)
        }

        fn sample(&mut self) -> i32 {
            match self.below(16) {
                0 => i32::MIN,
                1 => i32::MAX,
                2 => 0,
                3 => -1,
                _ => {
                    let bits = 1 + self.below(32);
                    self.signed(bits)
                }
            }
        }
    }

    fn check(
        random: &mut Random,
        prediction: fn(&mut [i32], &[i32], u32),
        reference: fn(&mut [i32], &[i32], u32),
    ) {
        let mut coefficients = [0i32; crate::MAXIMUM_ORDER];
        let mut expected = [0i32; 96];
        let mut actual = [0i32; 96];

        let order = 1 + random.below(crate::MAXIMUM_ORDER as u32) as usize;
        let precision = 1 + random.below(15);
        let shift = random.below(16);
        let length = order + random.below(64) as usize;

        for coefficient in &mut coefficients[..order] {
            *coefficient = match random.below(8) {
                0 => -(1 << (precision - 1)),
                1 => (1 << (precision - 1)) - 1,
                _ => random.signed(precision),
            };
        }

        for sample in &mut expected[..length] {
            *sample = random.sample();
        }

        actual[..length].copy_from_slice(&expected[..length]);
        reference(&mut expected[..length], &coefficients[..order], shift);
        prediction(&mut actual[..length], &coefficients[..order], shift);

        assert_eq!(
            actual[..length],
            expected[..length],
            "order {order} shift {shift}"
        );
    }

    const ROUNDS: usize = if cfg!(miri) { 16 } else { 20_000 };

    #[test]
    fn narrow_matches_scalar() {
        let mut random = Random(1);

        for _ in 0..ROUNDS {
            check(&mut random, predict_narrow, crate::predict_narrow);
        }
    }

    #[test]
    fn wide_matches_scalar() {
        let mut random = Random(2);

        for _ in 0..ROUNDS {
            check(&mut random, predict_wide, crate::predict_wide);
        }
    }

    #[test]
    fn every_order_matches_scalar() {
        let mut random = Random(3);
        let mut samples = [0i32; 80];

        for sample in &mut samples {
            *sample = random.sample();
        }

        let step = if cfg!(miri) { 7 } else { 1 };

        for order in (1..=crate::MAXIMUM_ORDER).step_by(step) {
            let coefficients: [i32; crate::MAXIMUM_ORDER] =
                core::array::from_fn(|_| random.signed(15));
            let coefficients = &coefficients[..order];

            for length in [order, order + 1, order + 5, samples.len()] {
                for shift in [0, 15] {
                    let mut expected = samples;
                    let mut actual = samples;

                    crate::predict_narrow(&mut expected[..length], coefficients, shift);
                    predict_narrow(&mut actual[..length], coefficients, shift);
                    assert_eq!(actual, expected);

                    let mut expected = samples;
                    let mut actual = samples;

                    crate::predict_wide(&mut expected[..length], coefficients, shift);
                    predict_wide(&mut actual[..length], coefficients, shift);
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}
