use crate::Format;

/// A PCM sample type: `u8`, `i16`, [`I24`], `i32` or `f32`.
///
/// Conversions go through a full-scale `i32` or an `f32` with full scale at -1.0 and 1.0. Converting to a narrower type rounds to the nearest value and saturates, and NaN becomes silence.
pub trait Sample: Copy + Default + PartialEq + Send + Sync + 'static {
    /// The [`Format`] of this type.
    const FORMAT: Format;
    /// The value of silence: `0x80` for `u8`, 0 otherwise.
    const SILENCE: Self;
    /// The size of one sample in a byte buffer, in bytes.
    const SIZE: usize = Self::FORMAT.sample_size();

    /// Scales the sample to the full `i32` range.
    fn to_i32(self) -> i32;

    /// Converts a full-scale `i32` to this type, rounding to the nearest value.
    fn from_i32(value: i32) -> Self;

    /// Converts the sample to an `f32` with full scale at -1.0 and 1.0.
    fn to_f32(self) -> f32;

    /// Converts an `f32` with full scale at -1.0 and 1.0 to this type.
    ///
    /// Integer types round to the nearest value, saturate out-of-range values and turn NaN into silence.
    fn from_f32(value: f32) -> Self;

    /// Reads a sample from the first [`SIZE`](Self::SIZE) bytes of `bytes`, little-endian, and panics if there are fewer.
    fn read_le(bytes: &[u8]) -> Self;

    /// Writes the sample to the first [`SIZE`](Self::SIZE) bytes of `bytes`, little-endian, and panics if there are fewer.
    fn write_le(self, bytes: &mut [u8]);

    /// Converts the sample to another sample type, through `f32` if either side is `f32` and through `i32` otherwise.
    fn convert<O: Sample>(self) -> O {
        if const { matches!(Self::FORMAT, Format::F32) || matches!(O::FORMAT, Format::F32) } {
            O::from_f32(self.to_f32())
        } else {
            O::from_i32(self.to_i32())
        }
    }
}

const F32_SCALE: f32 = 2_147_483_648.0;
const BELOW_HALF: f32 = 0.5 - f32::EPSILON / 4.0;

#[inline]
fn truncate(value: f32) -> i32 {
    (value * F32_SCALE) as i32
}

#[inline]
fn round(value: i32, shift: u32) -> i32 {
    (value.saturating_add(1 << (shift - 1)) + (value >> 31)) >> shift
}

impl Sample for u8 {
    const FORMAT: Format = Format::U8;
    const SILENCE: Self = 0x80;

    fn to_i32(self) -> i32 {
        (self as i32 - 0x80) << 24
    }

    fn from_i32(value: i32) -> Self {
        (round(value, 24) + 0x80) as u8
    }

    fn to_f32(self) -> f32 {
        (self as f32 - 128.0) / 128.0
    }

    fn from_f32(value: f32) -> Self {
        Self::from_i32(truncate(value))
    }

    #[inline]
    fn read_le(bytes: &[u8]) -> Self {
        bytes[0]
    }

    fn write_le(self, bytes: &mut [u8]) {
        bytes[0] = self;
    }
}

impl Sample for i16 {
    const FORMAT: Format = Format::I16;
    const SILENCE: Self = 0;

    fn to_i32(self) -> i32 {
        (self as i32) << 16
    }

    fn from_i32(value: i32) -> Self {
        round(value, 16) as i16
    }

    fn to_f32(self) -> f32 {
        self as f32 / 32_768.0
    }

    fn from_f32(value: f32) -> Self {
        Self::from_i32(truncate(value))
    }

    #[inline]
    fn read_le(bytes: &[u8]) -> Self {
        i16::from_le_bytes([bytes[0], bytes[1]])
    }

    fn write_le(self, bytes: &mut [u8]) {
        bytes[..2].copy_from_slice(&self.to_le_bytes());
    }
}

impl Sample for i32 {
    const FORMAT: Format = Format::I32;
    const SILENCE: Self = 0;

    fn to_i32(self) -> i32 {
        self
    }

    fn from_i32(value: i32) -> Self {
        value
    }

    fn to_f32(self) -> f32 {
        self as f32 / F32_SCALE
    }

    fn from_f32(value: f32) -> Self {
        (value * F32_SCALE + BELOW_HALF.copysign(value)) as i32
    }

    fn read_le(bytes: &[u8]) -> Self {
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    fn write_le(self, bytes: &mut [u8]) {
        bytes[..4].copy_from_slice(&self.to_le_bytes());
    }
}

/// A signed 24-bit sample.
///
/// It takes 4 bytes in memory and 3 bytes in a byte buffer.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct I24(i32);

impl I24 {
    /// The smallest 24-bit value.
    pub const MIN: Self = Self(-(1 << 23));
    /// The largest 24-bit value.
    pub const MAX: Self = Self((1 << 23) - 1);

    /// Creates a sample, saturating `value` to the 24-bit range.
    pub const fn new(value: i32) -> Self {
        if value < Self::MIN.0 {
            Self::MIN
        } else if value > Self::MAX.0 {
            Self::MAX
        } else {
            Self(value)
        }
    }

    /// Returns the sample as an `i32` between [`MIN`](Self::MIN) and [`MAX`](Self::MAX).
    pub const fn value(self) -> i32 {
        self.0
    }
}

impl From<I24> for i32 {
    fn from(sample: I24) -> Self {
        sample.0
    }
}

const I24_SCALE: f32 = 8_388_608.0;

impl Sample for I24 {
    const FORMAT: Format = Format::I24;
    const SILENCE: Self = Self(0);

    fn to_i32(self) -> i32 {
        self.0 << 8
    }

    fn from_i32(value: i32) -> Self {
        Self(round(value, 8))
    }

    fn to_f32(self) -> f32 {
        self.0 as f32 / I24_SCALE
    }

    fn from_f32(value: f32) -> Self {
        Self::from_i32(truncate(value))
    }

    #[inline]
    fn read_le(bytes: &[u8]) -> Self {
        Self(i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8)
    }

    fn write_le(self, bytes: &mut [u8]) {
        bytes[..3].copy_from_slice(&self.0.to_le_bytes()[..3]);
    }
}

impl Sample for f32 {
    const FORMAT: Format = Format::F32;
    const SILENCE: Self = 0.0;

    fn to_i32(self) -> i32 {
        i32::from_f32(self)
    }

    fn from_i32(value: i32) -> Self {
        value as f32 / F32_SCALE
    }

    fn to_f32(self) -> f32 {
        self
    }

    fn from_f32(value: f32) -> Self {
        value
    }

    fn read_le(bytes: &[u8]) -> Self {
        f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    fn write_le(self, bytes: &mut [u8]) {
        bytes[..4].copy_from_slice(&self.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_conversions() {
        assert_eq!(u8::SILENCE.convert::<i16>(), 0);
        assert_eq!(0xFFu8.convert::<i16>(), 0x7F00);
        assert_eq!(0x00u8.convert::<i16>(), i16::MIN);
        assert_eq!(i16::MAX.convert::<u8>(), 0xFF);
        assert_eq!(i16::MIN.convert::<u8>(), 0x00);
        assert_eq!(i16::MIN.convert::<i32>(), i32::MIN);
        assert_eq!(1.0f32.convert::<i16>(), i16::MAX);
        assert_eq!((-1.0f32).convert::<i16>(), i16::MIN);
        assert_eq!(i16::MIN.convert::<f32>(), -1.0);
        assert_eq!(f32::NAN.convert::<i16>(), 0);
        assert_eq!(4.0f32.convert::<i32>(), i32::MAX);
    }

    #[test]
    fn float_conversions() {
        assert_eq!(i16::MIN.to_f32(), -1.0);
        assert_eq!(i16::from_f32(1.0), i16::MAX);
        assert_eq!(i16::from_f32(f32::NAN), 0);
        assert_eq!(u8::from_f32(-1.0), 0);
        assert_eq!(u8::from_f32(1.0), u8::MAX);
        assert_eq!(u8::SILENCE.to_f32(), 0.0);
        assert_eq!(i32::from_f32(-2.0), i32::MIN);
        assert_eq!(f32::from_f32(3.0), 3.0);
        assert_eq!(u8::from_f32(f32::NAN), u8::SILENCE);
        assert_eq!(i32::from_f32(f32::NAN), 0);
        assert_eq!(f32::NAN.to_i32(), 0);
    }

    fn nearest(value: f32, scale: f64, min: i64, max: i64) -> i64 {
        if value.is_nan() {
            return 0;
        }
        let scaled = value as f64 * scale;
        if scaled >= max as f64 + 1.0 {
            return max;
        }
        if scaled <= min as f64 - 1.0 {
            return min;
        }
        let truncated = scaled as i64;
        let fraction = scaled - truncated as f64;
        let rounded = if fraction >= 0.5 {
            truncated + 1
        } else if fraction <= -0.5 {
            truncated - 1
        } else {
            truncated
        };
        rounded.clamp(min, max)
    }

    fn floats() -> impl Iterator<Item = f32> {
        let mut state = 0x1234_5678u32;
        let special = [
            0.0,
            -0.0,
            f32::MIN_POSITIVE,
            -f32::MIN_POSITIVE,
            f32::from_bits(1),
            -f32::from_bits(1),
            f32::from_bits(0x007F_FFFF),
            1.0,
            -1.0,
            1.0 + f32::EPSILON,
            -1.0 - f32::EPSILON,
            1.0 - f32::EPSILON / 2.0,
            -1.0 + f32::EPSILON / 2.0,
            0.5,
            -0.5,
            BELOW_HALF,
            -BELOW_HALF,
            3.0,
            -3.0,
            f32::MAX,
            f32::MIN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::EPSILON,
        ];
        let halves = (-40i32..40).flat_map(|value| {
            let half = value as f32 + 0.5;
            [
                half / 128.0,
                half / 32_768.0,
                half / 8_388_608.0,
                half / 2_147_483_648.0,
                (half - 1.0 / 64.0) / 32_768.0,
                (half + 1.0 / 64.0) / 32_768.0,
            ]
        });
        let random = (0..4_096).map(move |_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let bits = state;
            let value = f32::from_bits(bits);
            if value.is_nan() { 0.25 } else { value }
        });
        let unit = (0..4_096).map(move |index| (index as f32 / 2_048.0 - 1.0) * 0.999_9);
        special.into_iter().chain(halves).chain(random).chain(unit)
    }

    #[test]
    fn float_to_integer_rounds_to_nearest() {
        for value in floats() {
            assert_eq!(
                i16::from_f32(value) as i64,
                nearest(value, 32_768.0, i16::MIN as i64, i16::MAX as i64),
                "{value:e}"
            );
            assert_eq!(
                u8::from_f32(value) as i64 - 128,
                nearest(value, 128.0, -128, 127),
                "{value:e}"
            );
            assert_eq!(
                I24::from_f32(value).value() as i64,
                nearest(
                    value,
                    8_388_608.0,
                    I24::MIN.value() as i64,
                    I24::MAX.value() as i64
                ),
                "{value:e}"
            );
            assert_eq!(
                i32::from_f32(value) as i64,
                nearest(value, 2_147_483_648.0, i32::MIN as i64, i32::MAX as i64),
                "{value:e}"
            );
            assert_eq!(value.to_i32(), i32::from_f32(value));
        }
    }

    #[test]
    fn float_halfway_cases_round_away_from_zero() {
        assert_eq!(i16::from_f32(0.5 / 32_768.0), 1);
        assert_eq!(i16::from_f32(-0.5 / 32_768.0), -1);
        assert_eq!(i16::from_f32(1.5 / 32_768.0), 2);
        assert_eq!(i16::from_f32(2.5 / 32_768.0), 3);
        assert_eq!(i16::from_f32(-2.5 / 32_768.0), -3);
        assert_eq!(i16::from_f32(BELOW_HALF / 32_768.0), 0);
        assert_eq!(i16::from_f32(-BELOW_HALF / 32_768.0), 0);
        assert_eq!(u8::from_f32(0.5 / 128.0), 0x81);
        assert_eq!(u8::from_f32(-0.5 / 128.0), 0x7F);
        assert_eq!(I24::from_f32(0.5 / 8_388_608.0).value(), 1);
        assert_eq!(I24::from_f32(-0.5 / 8_388_608.0).value(), -1);
        assert_eq!(i32::from_f32(0.5 / 2_147_483_648.0), 1);
        assert_eq!(i32::from_f32(-0.5 / 2_147_483_648.0), -1);
        assert_eq!(i32::from_f32(BELOW_HALF / 2_147_483_648.0), 0);
        assert_eq!(i32::from_f32(8_388_609.0 / 2_147_483_648.0), 8_388_609);
        assert_eq!(i32::from_f32(-8_388_609.0 / 2_147_483_648.0), -8_388_609);
    }

    #[test]
    fn float_saturation() {
        for value in [1.0, 1.0 + f32::EPSILON, 3.0, f32::MAX, f32::INFINITY] {
            assert_eq!(i16::from_f32(value), i16::MAX);
            assert_eq!(u8::from_f32(value), u8::MAX);
            assert_eq!(I24::from_f32(value), I24::MAX);
            assert_eq!(i32::from_f32(value), i32::MAX);
            assert_eq!(i16::from_f32(-value), i16::MIN);
            assert_eq!(u8::from_f32(-value), u8::MIN);
            assert_eq!(I24::from_f32(-value), I24::MIN);
            assert_eq!(i32::from_f32(-value), i32::MIN);
        }
        assert_eq!(i16::from_f32(1.0 - 0.5 / 32_768.0), i16::MAX);
        assert_eq!(i16::from_f32(-1.0 + 0.5 / 32_768.0), i16::MIN);
        assert_eq!(i16::from_f32(-1.0 + 1.0 / 32_768.0), i16::MIN + 1);
    }

    #[test]
    fn float_to_float_is_identity() {
        for value in floats() {
            assert_eq!(value.convert::<f32>().to_bits(), value.to_bits());
            assert_eq!(f32::from_f32(value).to_bits(), value.to_bits());
        }
        assert!(f32::NAN.convert::<f32>().is_nan());
    }

    #[test]
    fn integer_float_round_trips_are_exact() {
        for value in i16::MIN..=i16::MAX {
            assert_eq!(i16::from_f32(value.to_f32()), value);
            assert_eq!(value.convert::<f32>().convert::<i16>(), value);
            assert_eq!(value.convert::<i32>().convert::<i16>(), value);
            assert_eq!(value.convert::<I24>().convert::<i16>(), value);
        }
        for value in u8::MIN..=u8::MAX {
            assert_eq!(value.convert::<f32>().convert::<u8>(), value);
            assert_eq!(value.convert::<i16>().convert::<u8>(), value);
            assert_eq!(value.convert::<I24>().convert::<u8>(), value);
        }
        for value in (I24::MIN.value()..=I24::MAX.value())
            .step_by(4_099)
            .chain(I24::MIN.value()..I24::MIN.value() + 256)
            .chain(-256..256)
            .chain(I24::MAX.value() - 256..=I24::MAX.value())
        {
            let sample = I24::new(value);
            assert_eq!(sample.convert::<f32>().convert::<I24>(), sample);
            assert_eq!(sample.convert::<i32>().convert::<I24>(), sample);
        }
        for value in [i32::MIN, -0x4000_0080, -1, 0, 1, 1 << 24, i32::MAX - 127] {
            assert_eq!(value.convert::<f32>().convert::<i32>(), value);
        }
    }

    #[test]
    fn integer_narrowing_rounds_to_nearest() {
        assert_eq!(i16::from_i32(0x7FFF), 0);
        assert_eq!(i16::from_i32(0x8000), 1);
        assert_eq!(i16::from_i32(-0x7FFF), 0);
        assert_eq!(i16::from_i32(-0x8000), -1);
        assert_eq!(i16::from_i32(-0x8001), -1);
        assert_eq!(i16::from_i32(0x1_8000), 2);
        assert_eq!(i16::from_i32(-0x1_8000), -2);
        assert_eq!(i16::from_i32(0x1_7FFF), 1);
        assert_eq!(i16::from_i32(-0x1_7FFF), -1);
        assert_eq!(u8::from_i32(0x7F_FFFF), 0x80);
        assert_eq!(u8::from_i32(0x80_0000), 0x81);
        assert_eq!(u8::from_i32(-0x80_0000), 0x7F);
        assert_eq!(u8::from_i32(-0x7F_FFFF), 0x80);
        assert_eq!(I24::from_i32(0x7F).value(), 0);
        assert_eq!(I24::from_i32(0x80).value(), 1);
        assert_eq!(I24::from_i32(-0x80).value(), -1);
        assert_eq!(I24::from_i32(-0x7F).value(), 0);
        assert_eq!(0x0080i16.convert::<u8>(), 0x81);
        assert_eq!((-0x0080i16).convert::<u8>(), 0x7F);
        assert_eq!(I24::new(0x80).convert::<i16>(), 1);
        assert_eq!(I24::new(-0x80).convert::<i16>(), -1);
    }

    #[test]
    fn integer_narrowing_saturates() {
        for value in [i32::MAX, i32::MAX - 1, 0x7FFF_8000, 0x7FFF_7FFF] {
            assert_eq!(i16::from_i32(value), i16::MAX);
        }
        for value in [i32::MAX, 0x7F80_0000, 0x7F7F_FFFF] {
            assert_eq!(u8::from_i32(value), u8::MAX);
        }
        for value in [i32::MAX, 0x7FFF_FF80, 0x7FFF_FF7F] {
            assert_eq!(I24::from_i32(value), I24::MAX);
        }
        assert_eq!(i16::from_i32(i32::MIN), i16::MIN);
        assert_eq!(u8::from_i32(i32::MIN), u8::MIN);
        assert_eq!(I24::from_i32(i32::MIN), I24::MIN);
        assert_eq!(i16::from_i32(i32::MIN + 1), i16::MIN);
    }

    #[test]
    fn little_endian_round_trip() {
        let mut bytes = [0u8; 4];

        0x1234i16.write_le(&mut bytes);
        assert_eq!(bytes[..2], [0x34, 0x12]);
        assert_eq!(i16::read_le(&bytes), 0x1234);

        (-0.5f32).write_le(&mut bytes);
        assert_eq!(f32::read_le(&bytes), -0.5);
    }

    #[test]
    fn twenty_four_bit() {
        assert_eq!(I24::SIZE, 3);
        assert_eq!(I24::new(1 << 23), I24::MAX);
        assert_eq!(I24::new(i32::MIN), I24::MIN);
        assert_eq!(i32::from(I24::new(-5)), -5);

        assert_eq!(I24::MAX.convert::<i32>(), 0x7FFF_FF00);
        assert_eq!(I24::MIN.convert::<i32>(), i32::MIN);
        assert_eq!(I24::MIN.convert::<i16>(), i16::MIN);
        assert_eq!(I24::MAX.convert::<u8>(), 0xFF);
        assert_eq!(i16::MIN.convert::<I24>(), I24::MIN);
        assert_eq!(0x1234i16.convert::<I24>().value(), 0x12_3400);
        assert_eq!(0x7FFF_FFFFi32.convert::<I24>(), I24::MAX);
        assert_eq!(0x80u8.convert::<I24>(), I24::SILENCE);

        assert_eq!(I24::MIN.to_f32(), -1.0);
        assert_eq!(I24::from_f32(1.0), I24::MAX);
        assert_eq!(I24::from_f32(-2.0), I24::MIN);
        assert_eq!(I24::from_f32(f32::NAN), I24::SILENCE);
        assert_eq!(I24::from_f32(0.5).value(), 0x40_0000);

        for value in [
            I24::MIN.value(),
            -0x12_3456,
            -1,
            0,
            1,
            0x12_3456,
            I24::MAX.value(),
        ] {
            let sample = I24::new(value);

            assert_eq!(I24::from_i32(sample.to_i32()), sample);
            assert_eq!(I24::from_f32(sample.to_f32()), sample);
        }
    }

    #[test]
    fn twenty_four_bit_little_endian() {
        let mut bytes = [0x11u8; 4];

        I24::new(-0x12_3456).write_le(&mut bytes);
        assert_eq!(bytes, [0xAA, 0xCB, 0xED, 0x11]);
        assert_eq!(I24::read_le(&bytes).value(), -0x12_3456);

        I24::new(0x12_3456).write_le(&mut bytes);
        assert_eq!(bytes[..3], [0x56, 0x34, 0x12]);
        assert_eq!(I24::read_le(&bytes).value(), 0x12_3456);

        assert_eq!(I24::read_le(&[0xFF, 0xFF, 0x7F]), I24::MAX);
        assert_eq!(I24::read_le(&[0x00, 0x00, 0x80]), I24::MIN);
    }
}
