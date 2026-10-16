use core::f64::consts::{FRAC_PI_2, LN_2, LOG2_E, PI, SQRT_2};

pub const DECIBELS_PER_OCTAVE: f64 = 3.010_299_956_639_812;

pub const DECIBELS_PER_OCTAVE_F32: f32 = DECIBELS_PER_OCTAVE as f32;

pub const MIX_SHIFTS: [u8; 9] = [0, 0, 1, 2, 2, 3, 3, 3, 3];

const LN_2_HIGH: f64 = 0.693_147_180_369_123_8;
const LN_2_LOW: f64 = 1.908_214_929_270_587_7e-10;

const INVERSE_FACTORIALS: [f64; 26] = {
    let mut table = [1.0; 26];
    let mut index = 1;

    while index < table.len() {
        table[index] = table[index - 1] / index as f64;
        index += 1;
    }

    table
};

const ODD_RECIPROCALS: [f64; 10] = {
    let mut table = [0.0; 10];
    let mut index = 0;

    while index < table.len() {
        table[index] = 1.0 / (2 * index + 1) as f64;
        index += 1;
    }

    table
};

pub const fn round(value: f64) -> i64 {
    if value >= 0.0 {
        (value + 0.5) as i64
    } else {
        (value - 0.5) as i64
    }
}

const fn alternating(square: f64, first: usize) -> f64 {
    let mut index = first + 24;
    let mut sum = INVERSE_FACTORIALS[index];

    while index > first {
        index -= 2;
        sum = INVERSE_FACTORIALS[index] - square * sum;
    }

    sum
}

pub const fn cosine(angle: f64) -> f64 {
    if angle > FRAC_PI_2 {
        return -cosine(PI - angle);
    }

    alternating(angle * angle, 0)
}

pub const fn sine(angle: f64) -> f64 {
    let angle = if angle > FRAC_PI_2 { PI - angle } else { angle };

    angle * alternating(angle * angle, 1)
}

const fn exponential_series(value: f64) -> f64 {
    let mut index = 14;
    let mut sum = INVERSE_FACTORIALS[index];

    while index > 0 {
        index -= 1;
        sum = sum * value + INVERSE_FACTORIALS[index];
    }

    sum
}

const fn power_of_two(exponent: i64) -> f64 {
    if exponent < -1022 {
        f64::MIN_POSITIVE * f64::from_bits(((exponent + 2045) as u64) << 52)
    } else {
        f64::from_bits(((exponent + 1023) as u64) << 52)
    }
}

pub const fn exp2(value: f64) -> f64 {
    if value.is_nan() {
        return value;
    }

    if value < -1100.0 {
        return 0.0;
    }

    if value > 1023.0 {
        return f64::INFINITY;
    }

    let whole = round(value);

    exponential_series((value - whole as f64) * LN_2) * power_of_two(whole)
}

pub const fn exp(value: f64) -> f64 {
    if value.is_nan() {
        return value;
    }

    if value < -760.0 {
        return 0.0;
    }

    if value > 709.0 {
        return f64::INFINITY;
    }

    let whole = round(value * LOG2_E);
    let rest = (value - whole as f64 * LN_2_HIGH) - whole as f64 * LN_2_LOW;

    exponential_series(rest) * power_of_two(whole)
}

pub const fn sinh(value: f64) -> f64 {
    if value > -1.0 && value < 1.0 {
        return value * alternating(-value * value, 1);
    }

    (exp(value) - exp(-value)) / 2.0
}

#[inline(always)]
const fn pair(square: f64, index: usize) -> f64 {
    ODD_RECIPROCALS[index] + ODD_RECIPROCALS[index + 1] * square
}

#[inline]
pub const fn log2(value: f64) -> f64 {
    if !(value > 0.0 && value < f64::INFINITY) {
        return if value == 0.0 {
            f64::NEG_INFINITY
        } else if value == f64::INFINITY {
            value
        } else {
            f64::NAN
        };
    }

    let (value, offset) = if value < f64::MIN_POSITIVE {
        (value * (1u128 << 64) as f64, -64.0)
    } else {
        (value, 0.0)
    };
    let bits = value.to_bits();
    let mut exponent = (bits >> 52) as i64 - 1023;
    let mut mantissa = f64::from_bits((bits & ((1 << 52) - 1)) | (1023 << 52));

    if mantissa > SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }

    let ratio = (mantissa - 1.0) / (mantissa + 1.0);
    let square = ratio * ratio;
    let fourth = square * square;
    let eighth = fourth * fourth;
    let low = pair(square, 0) + pair(square, 2) * fourth;
    let high = pair(square, 4) + pair(square, 6) * fourth;
    let sum = low + (high + pair(square, 8) * eighth) * eighth;

    exponent as f64 + offset + ratio * sum * (2.0 * LOG2_E)
}

#[inline]
pub const fn square_root(value: f64) -> f64 {
    if !(value > 0.0 && value < f64::INFINITY) {
        return if value < 0.0 { f64::NAN } else { value };
    }

    let (value, scale) = if value < f64::MIN_POSITIVE {
        (value * (1u128 << 64) as f64, 1.0 / (1u64 << 32) as f64)
    } else {
        (value, 1.0)
    };
    let mut root = f64::from_bits((value.to_bits() >> 1) + 0x1FF8_0000_0000_0000);
    let mut index = 0;

    while index < 4 {
        root = 0.5 * (root + value / root);
        index += 1;
    }

    root * scale
}

#[inline]
pub const fn log2_f32(value: f32) -> f32 {
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
    let mut exponent = (bits >> 23) as i32 - 127;
    let mut mantissa = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000);

    if mantissa > core::f32::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }

    let ratio = (mantissa - 1.0) / (mantissa + 1.0);
    let square = ratio * ratio;
    let series = ratio * (1.0 + square * (1.0 / 3.0 + square * (1.0 / 5.0 + square * (1.0 / 7.0))));

    exponent as f32 + offset + series * (2.0 / core::f32::consts::LN_2)
}

#[inline]
pub const fn square_root_f32(value: f32) -> f32 {
    if !(value > 0.0 && value < f32::INFINITY) {
        return if value < 0.0 { f32::NAN } else { value };
    }

    let (value, scale) = if value < f32::MIN_POSITIVE {
        (value * (1u64 << 32) as f32, 1.0 / (1u32 << 16) as f32)
    } else {
        (value, 1.0)
    };
    let mut root = f32::from_bits((value.to_bits() >> 1) + 0x1FBD_1DF5);
    let mut index = 0;

    while index < 3 {
        root = 0.5 * (root + value / root);
        index += 1;
    }

    root * scale
}

#[inline]
pub const fn power_decibels(power: f32) -> f32 {
    DECIBELS_PER_OCTAVE_F32 * log2_f32(power)
}

#[inline]
pub const fn amplitude_decibels(amplitude: f32) -> f32 {
    2.0 * DECIBELS_PER_OCTAVE_F32 * log2_f32(amplitude)
}

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

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn slack() -> u64 {
        if cfg!(miri) { 64 } else { 0 }
    }

    fn count() -> usize {
        if cfg!(miri) { 200 } else { 200_000 }
    }

    fn ulps(value: f64, expected: f64) -> u64 {
        let ordered = |value: f64| {
            let bits = value.to_bits() as i64;

            if bits < 0 {
                i64::MIN as i128 - bits as i128
            } else {
                bits as i128
            }
        };

        (ordered(value) - ordered(expected)).unsigned_abs() as u64
    }

    fn ulps_f32(value: f32, expected: f32) -> u64 {
        let ordered = |value: f32| {
            let bits = value.to_bits() as i32;

            if bits < 0 {
                i32::MIN as i64 - bits as i64
            } else {
                bits as i64
            }
        };

        (ordered(value) - ordered(expected)).unsigned_abs()
    }

    #[test]
    fn constants() {
        assert!((DECIBELS_PER_OCTAVE - 10.0 * 2f64.log10()).abs() < 1e-15);
        assert_eq!(DECIBELS_PER_OCTAVE_F32, 3.010_3);
        assert_eq!(round(-2.5), -3);
        assert_eq!(round(2.5), 3);
        assert_eq!(round(-0.4), 0);

        for (index, value) in INVERSE_FACTORIALS.iter().enumerate() {
            let factorial: f64 = (1..=index).map(|factor| factor as f64).product();
            assert!((value * factorial - 1.0).abs() < 1e-15);
        }
    }

    #[test]
    fn trigonometry() {
        let mut random = Random(1);

        for step in 0..=count() {
            let angle = if step % 2 == 0 {
                PI * step as f64 / count() as f64
            } else {
                PI * random.unit()
            };

            let bound = 5e-16 + slack() as f64 * 1e-15;

            assert!((cosine(angle) - angle.cos()).abs() < bound, "{angle}");
            assert!((sine(angle) - angle.sin()).abs() < bound, "{angle}");
        }

        assert_eq!(cosine(0.0), 1.0);
        assert_eq!(sine(0.0), 0.0);
        assert_eq!(cosine(PI), -1.0);
    }

    #[test]
    fn exponentials() {
        let mut random = Random(2);

        for _ in 0..count() {
            let value = random.unit() * 1400.0 - 1100.0;
            assert!(ulps(exp2(value), value.exp2()) <= 2 + slack(), "{value}");

            let value = random.unit() * 1400.0 - 760.0;
            assert!(ulps(exp(value), value.exp()) <= 2 + slack(), "{value}");

            let value = random.unit() * 40.0 - 20.0;
            assert!(ulps(sinh(value), value.sinh()) <= 4 + slack(), "{value}");
        }

        for value in [-1100.0, -1074.0, -1022.5, -0.5, 0.0, 0.5, 1.0, 10.0, 1023.0] {
            assert!(
                ulps(exp2(value), f64::exp2(value)) <= 1 + slack(),
                "{value}"
            );
        }

        for value in [-760.0, -745.0, -708.5, -1.0, 0.0, 1e-300, 1.0, 709.0] {
            assert!(ulps(exp(value), f64::exp(value)) <= 1 + slack(), "{value}");
        }

        for value in [-1.0, -1e-3, -1e-300, 0.0, 1e-9, 0.25, 1.0, 1.5] {
            assert!(
                ulps(sinh(value), f64::sinh(value)) <= 2 + slack(),
                "{value}"
            );
        }

        assert_eq!(exp2(0.0), 1.0);
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(exp2(-2000.0), 0.0);
        assert_eq!(exp(-2000.0), 0.0);
        assert_eq!(exp2(2000.0), f64::INFINITY);
        assert_eq!(exp(f64::NEG_INFINITY), 0.0);
        assert!(exp(f64::NAN).is_nan());
        assert!(exp2(f64::NAN).is_nan());
    }

    #[test]
    fn logarithms_and_roots() {
        let mut random = Random(3);

        for _ in 0..count() {
            let value = f64::from_bits(random.next() % f64::INFINITY.to_bits());
            assert!(ulps(log2(value), value.log2()) <= 4 + slack(), "{value}");
            assert!(
                ulps(square_root(value), value.sqrt()) <= 1 + slack(),
                "{value}"
            );

            let value = f32::from_bits(random.next() as u32 % f32::INFINITY.to_bits());
            let expected = (value as f64).log2();
            assert!(
                (log2_f32(value) as f64 - expected).abs() < 8e-6
                    && ulps_f32(log2_f32(value), expected as f32) <= 5 + slack(),
                "{value}"
            );
            assert!(
                ulps_f32(square_root_f32(value), value.sqrt()) <= 1 + slack(),
                "{value}"
            );
        }

        for index in 1..2000 {
            let value = (1.000_37f64.powi(index * 37) * 1e-30) as f32;
            let exact = (value as f64).log2();
            assert!((log2_f32(value) as f64 - exact).abs() < 1e-6 + exact.abs() * 6e-8);
            assert!((square_root_f32(value) as f64 / (value as f64).sqrt() - 1.0).abs() < 2e-7);
        }

        for value in [
            5e-324, 1e-310, 1e-300, 0.3, 1.0, 1.5, 2.0, 3.0, 1e10, 1.7e308,
        ] {
            assert!(ulps(log2(value), value.log2()) <= 4 + slack(), "{value}");
            assert!(
                ulps(square_root(value), value.sqrt()) <= 1 + slack(),
                "{value}"
            );
        }

        for value in [
            1e-45f32,
            1e-40,
            1e-30,
            0.1,
            1.0,
            1.42,
            2.0,
            3.7,
            1e30,
            f32::MAX,
        ] {
            assert!(
                (log2_f32(value) as f64 - (value as f64).log2()).abs() < 8e-6,
                "{value}"
            );
            assert!(
                ulps_f32(square_root_f32(value), value.sqrt()) <= 1 + slack(),
                "{value}"
            );
        }

        assert_eq!(log2(1.0), 0.0);
        assert_eq!(log2(0.0), f64::NEG_INFINITY);
        assert_eq!(log2(f64::INFINITY), f64::INFINITY);
        assert!(log2(-1.0).is_nan());
        assert!(log2(f64::NAN).is_nan());
        assert_eq!(square_root(0.0), 0.0);
        assert_eq!(square_root(-0.0), 0.0);
        assert_eq!(square_root(f64::INFINITY), f64::INFINITY);
        assert!(square_root(-1.0).is_nan());
        assert!(square_root(f64::NAN).is_nan());
        assert_eq!(log2_f32(1.0), 0.0);
        assert_eq!(log2_f32(0.0), f32::NEG_INFINITY);
        assert_eq!(log2_f32(f32::INFINITY), f32::INFINITY);
        assert!(log2_f32(-1.0).is_nan());
        assert!(log2_f32(f32::NAN).is_nan());
        assert_eq!(square_root_f32(0.0), 0.0);
        assert_eq!(square_root_f32(f32::INFINITY), f32::INFINITY);
        assert!(square_root_f32(-1.0).is_nan());
        assert!(square_root_f32(f32::NAN).is_nan());
    }

    #[test]
    fn decibels() {
        let mut value = 1.0e-40f32;

        while value < 1.0e30 {
            let expected = 20.0 * (value as f64).log10();

            assert!(
                (amplitude_decibels(value) as f64 - expected).abs()
                    < 1.0e-5 + 2.0e-7 * expected.abs(),
                "{value}"
            );
            assert!(
                (power_decibels(value) as f64 - expected / 2.0).abs()
                    < 1.0e-5 + 2.0e-7 * expected.abs(),
                "{value}"
            );
            value *= 1.37;
        }

        assert_eq!(amplitude_decibels(1.0), 0.0);
        assert_eq!(amplitude_decibels(0.0), f32::NEG_INFINITY);
        assert_eq!(power_decibels(f32::INFINITY), f32::INFINITY);
        assert!(power_decibels(f32::NAN).is_nan());
    }

    #[test]
    fn wide_multiplies() {
        let edges = [
            0,
            1,
            -1,
            i32::MAX,
            i32::MIN,
            i32::MIN + 1,
            1 << 30,
            -(1 << 30),
            0xFFFF,
            0x10000,
            -0x10000,
            -0x10001,
            0x7FFF_0000,
        ];
        let mut random = Random(4);

        for left in edges {
            for right in edges {
                assert_eq!(wide_split(left, right), left as i64 * right as i64);
                assert_eq!(wide(left, right), left as i64 * right as i64);
            }
        }

        for _ in 0..if cfg!(miri) { 1000 } else { 1_000_000 } {
            let left = random.next() as i32;
            let right = (random.next() >> 32) as i32;

            assert_eq!(wide_split(left, right), left as i64 * right as i64);
        }
    }
}
