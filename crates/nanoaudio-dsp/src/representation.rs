use nanoaudio_core::{Format, Sample};

use crate::Volume;

/// A [`Representation`] chosen at run time, for the `_with` functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepresentationKind {
    /// Fixed-point processing, see [`Fixed`].
    Fixed,
    /// Floating-point processing, see [`Float`].
    Float,
}

impl RepresentationKind {
    /// The kind of [`DefaultRepresentation`].
    pub const DEFAULT: Self = DefaultRepresentation::KIND;
}

impl Default for RepresentationKind {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How samples are held while they are processed: [`Fixed`] or [`Float`].
///
/// Every function that processes audio is generic over it, so the choice costs nothing per sample.
pub trait Representation: Copy + Default + Send + Sync + 'static {
    /// The value a sample is converted to for processing.
    type Value: Copy;
    /// The value a [`Volume`] is converted to.
    type Gain: Copy;

    /// The matching [`RepresentationKind`].
    const KIND: RepresentationKind;
    /// The value of silence.
    const SILENCE: Self::Value;

    /// Converts a sample to a value.
    fn from_sample<S: Sample>(sample: S) -> Self::Value;

    /// Converts a value back to a sample, rounding to the nearest value and saturating.
    fn to_sample<S: Sample>(value: Self::Value) -> S;

    /// Converts a volume to a gain.
    fn gain(volume: Volume) -> Self::Gain;

    /// Multiplies a value by a gain.
    fn amplify(value: Self::Value, gain: Self::Gain) -> Self::Value;

    /// Multiplies a value by a gain returned by [`gain`](Self::gain).
    ///
    /// Gives the same result as [`amplify`](Self::amplify), and can be faster since the gain is known to come from a [`Volume`].
    #[inline]
    fn amplify_volume(value: Self::Value, gain: Self::Gain) -> Self::Value {
        Self::amplify(value, gain)
    }

    /// Adds two values.
    fn add(left: Self::Value, right: Self::Value) -> Self::Value;

    /// Returns the average of `values`, or silence if it is empty.
    fn average(values: &[Self::Value]) -> Self::Value;

    /// Multiplies a sample by a gain.
    #[inline]
    fn amplify_sample<S: Sample>(sample: S, gain: Self::Gain) -> S {
        Self::to_sample(Self::amplify(Self::from_sample(sample), gain))
    }

    /// Returns `output` plus `input` multiplied by `gain`.
    #[inline]
    fn mix_sample<S: Sample>(output: S, input: S, gain: Self::Gain) -> S {
        Self::to_sample(Self::add(
            Self::from_sample(output),
            Self::amplify(Self::from_sample(input), gain),
        ))
    }
}

const ROUNDING: i32 = 1 << (Volume::FRACTIONAL_BITS - 1);

const fn padding(format: Format) -> u32 {
    match format {
        Format::U8 => 24,
        Format::I16 => 16,
        Format::I24 => 8,
        Format::I32 | Format::F32 => 0,
    }
}

const fn maximum(format: Format) -> i32 {
    i32::MAX >> padding(format)
}

const fn minimum(format: Format) -> i32 {
    -maximum(format) - 1
}

const fn product_maximum(format: Format) -> i32 {
    match format {
        Format::U8 | Format::I16 => ((maximum(format) + 1) << Volume::FRACTIONAL_BITS) - 1,
        Format::I24 | Format::I32 | Format::F32 => i32::MAX,
    }
}

#[inline]
fn round(value: i32) -> i32 {
    (value + ROUNDING + (value >> 31)) >> Volume::FRACTIONAL_BITS
}

#[inline]
fn round_wide(value: i64) -> i64 {
    (value + ROUNDING as i64 + (value >> 63)) >> Volume::FRACTIONAL_BITS
}

#[inline]
fn to_integer<S: Sample>(sample: S) -> i32 {
    sample.to_i32() >> const { padding(S::FORMAT) }
}

#[inline]
fn from_integer<S: Sample>(value: i32) -> S {
    match S::FORMAT {
        Format::U8 => S::read_le(&[value as u8 ^ 0x80]),
        Format::I16 => S::read_le(&(value as i16).to_le_bytes()),
        Format::I24 => S::read_le(&value.to_le_bytes()),
        Format::I32 | Format::F32 => S::from_i32(value << const { padding(S::FORMAT) }),
    }
}

#[inline]
fn saturate<S: Sample>(value: i32) -> i32 {
    value.clamp(const { minimum(S::FORMAT) }, const { maximum(S::FORMAT) })
}

#[inline]
fn amplify_narrow<S: Sample>(value: i32, gain: i32) -> i32 {
    saturate::<S>(round(value * gain))
}

#[inline]
fn mix_narrow<S: Sample>(output: i32, input: i32, gain: i32) -> i32 {
    let amplified = (input * gain).clamp(
        const { -product_maximum(S::FORMAT) - 1 },
        const { product_maximum(S::FORMAT) },
    );

    saturate::<S>(round((output << Volume::FRACTIONAL_BITS) + amplified))
}

#[inline]
fn amplify_wide<S: Sample>(value: i32, gain: i32) -> i32 {
    round_wide(value as i64 * gain as i64).clamp(
        const { minimum(S::FORMAT) as i64 },
        const { maximum(S::FORMAT) as i64 },
    ) as i32
}

#[inline]
fn mix_wide<S: Sample>(output: i32, input: i32, gain: i32) -> i32 {
    saturate::<S>(output.saturating_add(amplify_wide::<S>(input, gain)))
}

const LOW_MASK: i32 = (1 << Volume::FRACTIONAL_BITS) - 1;

#[inline]
fn amplify_split<S: Sample>(value: i32, gain: i32) -> i32 {
    let high = (value >> Volume::FRACTIONAL_BITS) * gain;
    let low = ((value & LOW_MASK) * gain + ROUNDING + (value >> 31)) >> Volume::FRACTIONAL_BITS;

    saturate::<S>(high + low)
}

#[inline]
fn mix_split<S: Sample>(output: i32, input: i32, gain: i32) -> i32 {
    saturate::<S>(output + amplify_split::<S>(input, gain))
}

const HALF_BITS: u32 = 16;

const HALF_MASK: i32 = (1 << HALF_BITS) - 1;

const UNIT_BITS: u32 = HALF_BITS - Volume::FRACTIONAL_BITS;

#[inline]
fn amplify_halves(value: i32, gain: i32) -> i32 {
    let high = (value >> HALF_BITS) * gain;
    let low = ((value & HALF_MASK) as u32 * gain as u32)
        .wrapping_add(ROUNDING as u32)
        .wrapping_add((high >> 31) as u32)
        >> Volume::FRACTIONAL_BITS;
    let units = high + (low >> UNIT_BITS) as i32;

    if units > i32::MAX >> UNIT_BITS {
        i32::MAX
    } else if units < i32::MIN >> UNIT_BITS {
        i32::MIN
    } else {
        (units << UNIT_BITS) | (low & ((1 << UNIT_BITS) - 1)) as i32
    }
}

/// Fixed-point processing, for cores without a floating-point unit.
///
/// Values are full-scale `i32` (Q31) and gains are Q4.12. Every operation saturates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Fixed;

impl Representation for Fixed {
    type Value = i32;
    type Gain = i32;

    const KIND: RepresentationKind = RepresentationKind::Fixed;
    const SILENCE: i32 = 0;

    #[inline]
    fn from_sample<S: Sample>(sample: S) -> i32 {
        sample.to_i32()
    }

    #[inline]
    fn to_sample<S: Sample>(value: i32) -> S {
        S::from_i32(value)
    }

    #[inline]
    fn gain(volume: Volume) -> i32 {
        volume.raw() as i32
    }

    #[inline]
    fn amplify(value: i32, gain: i32) -> i32 {
        amplify_wide::<i32>(value, gain)
    }

    #[inline]
    fn amplify_volume(value: i32, gain: i32) -> i32 {
        if cfg!(nanoaudio_narrow_multiply) {
            amplify_halves(value, gain)
        } else {
            Self::amplify(value, gain)
        }
    }

    #[inline]
    fn add(left: i32, right: i32) -> i32 {
        left.saturating_add(right)
    }

    #[inline]
    fn average(values: &[i32]) -> i32 {
        if values.is_empty() {
            return 0;
        }

        let sum: i64 = values.iter().map(|value| *value as i64).sum();

        (sum / values.len() as i64) as i32
    }

    #[inline]
    fn amplify_sample<S: Sample>(sample: S, gain: i32) -> S {
        match S::FORMAT {
            Format::U8 | Format::I16 => from_integer(amplify_narrow::<S>(to_integer(sample), gain)),
            Format::I24 => from_integer(amplify_split::<S>(to_integer(sample), gain)),
            Format::I32 => from_integer(amplify_wide::<S>(to_integer(sample), gain)),
            Format::F32 => Self::to_sample(Self::amplify(Self::from_sample(sample), gain)),
        }
    }

    #[inline]
    fn mix_sample<S: Sample>(output: S, input: S, gain: i32) -> S {
        match S::FORMAT {
            Format::U8 | Format::I16 => {
                from_integer(mix_narrow::<S>(to_integer(output), to_integer(input), gain))
            }
            Format::I24 => {
                from_integer(mix_split::<S>(to_integer(output), to_integer(input), gain))
            }
            Format::I32 => from_integer(mix_wide::<S>(to_integer(output), to_integer(input), gain)),
            Format::F32 => Self::to_sample(Self::add(
                Self::from_sample(output),
                Self::amplify(Self::from_sample(input), gain),
            )),
        }
    }
}

/// Floating-point processing, for cores with a single-precision floating-point unit.
///
/// Values are `f32` with full scale at -1.0 and 1.0. They can go beyond it while processing, and are clipped when converted back to samples.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Float;

impl Representation for Float {
    type Value = f32;
    type Gain = f32;

    const KIND: RepresentationKind = RepresentationKind::Float;
    const SILENCE: f32 = 0.0;

    #[inline]
    fn from_sample<S: Sample>(sample: S) -> f32 {
        sample.to_f32()
    }

    #[inline]
    fn to_sample<S: Sample>(value: f32) -> S {
        S::from_f32(value)
    }

    #[inline]
    fn gain(volume: Volume) -> f32 {
        volume.raw() as f32 / Volume::UNITY.raw() as f32
    }

    #[inline]
    fn amplify(value: f32, gain: f32) -> f32 {
        value * gain
    }

    #[inline]
    fn add(left: f32, right: f32) -> f32 {
        left + right
    }

    #[inline]
    fn average(values: &[f32]) -> f32 {
        if values.is_empty() {
            return 0.0;
        }

        values.iter().sum::<f32>() / values.len() as f32
    }
}

/// The representation used when none is given, chosen when the crate is built.
///
/// It is [`Float`] on targets with a single-precision floating-point unit (x86, AArch64, WebAssembly, `eabihf` ARM, RISC-V with the `F` extension, ESP32 and ESP32-S3) and [`Fixed`] elsewhere. The `fixed` and `float` features override it, and `fixed` takes precedence when both are enabled.
#[cfg(nanoaudio_float)]
pub type DefaultRepresentation = Float;

/// The representation used when none is given, chosen when the crate is built.
///
/// It is [`Float`] on targets with a single-precision floating-point unit (x86, AArch64, WebAssembly, `eabihf` ARM, RISC-V with the `F` extension, ESP32 and ESP32-S3) and [`Fixed`] elsewhere. The `fixed` and `float` features override it, and `fixed` takes precedence when both are enabled.
#[cfg(not(nanoaudio_float))]
pub type DefaultRepresentation = Fixed;

#[cfg(test)]
mod tests {
    use nanoaudio_core::I24;

    use super::*;

    fn round_trip<R: Representation>() {
        for sample in [i16::MIN, -1000, 0, 1000, i16::MAX] {
            assert_eq!(R::to_sample::<i16>(R::from_sample(sample)), sample);
        }

        for sample in [0u8, 0x7F, 0x80, 0xFF] {
            assert_eq!(R::to_sample::<u8>(R::from_sample(sample)), sample);
        }

        assert_eq!(R::to_sample::<f32>(R::from_sample(-0.5f32)), -0.5);
    }

    fn saturates<R: Representation>() {
        let maximum = R::from_sample(i16::MAX);

        assert_eq!(R::to_sample::<i16>(R::add(maximum, maximum)), i16::MAX);
        assert_eq!(
            R::to_sample::<i16>(R::amplify(maximum, R::gain(Volume::MAXIMUM))),
            i16::MAX
        );
        assert_eq!(
            R::to_sample::<i16>(R::add(R::from_sample(1.0f32), R::from_sample(1.0f32))),
            i16::MAX
        );
    }

    fn amplifies<R: Representation>() {
        let half = R::gain(Volume::from_percent(50));

        assert_eq!(
            R::to_sample::<i16>(R::amplify(R::from_sample(1000i16), half)),
            500
        );
        assert_eq!(
            R::to_sample::<i16>(R::amplify(R::from_sample(-1000i16), half)),
            -500
        );
    }

    fn averages<R: Representation>() {
        let values = [
            R::from_sample(i16::MAX),
            R::from_sample(i16::MAX),
            R::from_sample(0i16),
        ];

        assert_eq!(R::to_sample::<i16>(R::average(&values)), 21_845);
        assert_eq!(R::to_sample::<i16>(R::average(&[])), 0);
    }

    #[test]
    fn fixed() {
        averages::<Fixed>();
        round_trip::<Fixed>();
        saturates::<Fixed>();
        amplifies::<Fixed>();
    }

    const GAINS: [u16; 11] = [
        0,
        1,
        2047,
        2048,
        3072,
        4095,
        4096,
        4097,
        12_345,
        40_000,
        u16::MAX,
    ];

    fn round(value: i64) -> i64 {
        value.signum() * ((value.abs() + 2048) >> 12)
    }

    fn saturating(value: i64, bits: u32) -> i64 {
        let maximum = (1i64 << (bits - 1)) - 1;

        value.clamp(-maximum - 1, maximum)
    }

    fn exact(sample: i64, raw: u16, bits: u32) -> i64 {
        saturating(round(sample * raw as i64), bits)
    }

    fn generic_amplify<S: Sample>(sample: S, gain: i32) -> S {
        Fixed::to_sample(Fixed::amplify(Fixed::from_sample(sample), gain))
    }

    fn generic_mix<S: Sample>(output: S, input: S, gain: i32) -> S {
        Fixed::to_sample(Fixed::add(
            Fixed::from_sample(output),
            Fixed::amplify(Fixed::from_sample(input), gain),
        ))
    }

    #[test]
    fn fixed_rounds_to_nearest() {
        let three_quarters = Fixed::gain(Volume::from_raw(3072));
        let half = Fixed::gain(Volume::from_percent(50));

        assert_eq!(Fixed::amplify_sample(5i16, three_quarters), 4);
        assert_eq!(Fixed::amplify_sample(-3i16, three_quarters), -2);
        assert_eq!(Fixed::amplify_sample(-5i16, three_quarters), -4);
        assert_eq!(Fixed::amplify_sample(1i16, half), 1);
        assert_eq!(Fixed::amplify_sample(-1i16, half), -1);
        assert_eq!(Fixed::amplify_sample(-3i16, half), -2);
        assert_eq!(Fixed::amplify_sample(0x85u8, three_quarters), 0x84);
        assert_eq!(Fixed::amplify_sample(0x7Du8, three_quarters), 0x7E);
        assert_eq!(
            Fixed::amplify_sample(I24::new(-3), three_quarters),
            I24::new(-2)
        );
        assert_eq!(Fixed::amplify_sample(5i32, three_quarters), 4);
        assert_eq!(Fixed::amplify_sample(-1i32, half), -1);
        assert_eq!(Fixed::mix_sample(10i16, 5, three_quarters), 14);
        assert_eq!(Fixed::mix_sample(10i32, -3, three_quarters), 8);
    }

    #[test]
    fn fixed_narrow_path_matches_generic_path() {
        let outputs = [i16::MIN, -12_345, -1, 0, 1, 12_345, i16::MAX];
        let stride = if cfg!(miri) { 997 } else { 61 };

        for raw in GAINS {
            let gain = Fixed::gain(Volume::from_raw(raw));
            let samples =
                (i16::MIN..=i16::MAX)
                    .step_by(stride)
                    .chain([i16::MIN, -1, 0, 1, i16::MAX]);

            for sample in samples {
                assert_eq!(
                    Fixed::amplify_sample(sample, gain),
                    generic_amplify(sample, gain)
                );
                assert_eq!(
                    Fixed::amplify_sample(sample, gain) as i64,
                    exact(sample as i64, raw, 16)
                );

                for output in outputs {
                    assert_eq!(
                        Fixed::mix_sample(output, sample, gain),
                        generic_mix(output, sample, gain)
                    );
                }
            }

            for sample in (0..=u8::MAX).step_by(stride / 60) {
                assert_eq!(
                    Fixed::amplify_sample(sample, gain),
                    generic_amplify(sample, gain)
                );

                for output in [0, 0x01, 0x7F, 0x80, 0x81, 0xFF] {
                    assert_eq!(
                        Fixed::mix_sample(output, sample, gain),
                        generic_mix(output, sample, gain)
                    );
                }
            }
        }
    }

    #[test]
    fn fixed_halves_match_wide_path() {
        let gain_stride = if cfg!(miri) { 4_099 } else { 1 };
        let value_count = if cfg!(miri) { 16 } else { 96 };
        let mut state = 0x9E37_79B9u32;

        for raw in (0..=u16::MAX).step_by(gain_stride).chain(GAINS) {
            let gain = Fixed::gain(Volume::from_raw(raw));
            let edges = [
                i32::MIN,
                i32::MIN + 1,
                -0x0001_0000,
                -0xFFFF,
                -1,
                0,
                1,
                0xFFFF,
                0x0001_0000,
                i32::MAX - 1,
                i32::MAX,
            ];
            let random = (0..value_count).map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as i32
            });

            for value in edges.into_iter().chain(random) {
                assert_eq!(
                    amplify_halves(value, gain),
                    Fixed::amplify(value, gain),
                    "{value} {raw}"
                );
            }
        }
    }

    #[test]
    fn fixed_wide_path_matches_exact_arithmetic() {
        for raw in GAINS {
            let gain = Fixed::gain(Volume::from_raw(raw));

            for sample in [
                i32::MIN,
                i32::MIN + 1,
                -12_345_678,
                -1,
                0,
                1,
                12_345_678,
                i32::MAX,
            ] {
                let expected = exact(sample as i64, raw, 32);

                assert_eq!(Fixed::amplify_sample(sample, gain) as i64, expected);
                assert_eq!(Fixed::amplify(sample, gain) as i64, expected);

                for output in [i32::MIN, 0, i32::MAX] {
                    assert_eq!(
                        Fixed::mix_sample(output, sample, gain) as i64,
                        saturating(output as i64 + expected, 32)
                    );
                }
            }

            for sample in [I24::MIN, I24::new(-1), I24::SILENCE, I24::new(1), I24::MAX] {
                let expected = exact(sample.value() as i64, raw, 24);

                assert_eq!(Fixed::amplify_sample(sample, gain).value() as i64, expected);

                for output in [I24::MIN, I24::SILENCE, I24::MAX] {
                    assert_eq!(
                        Fixed::mix_sample(output, sample, gain).value() as i64,
                        saturating(output.value() as i64 + expected, 24)
                    );
                }
            }
        }
    }

    #[test]
    fn fixed_i24_split_matches_wide_arithmetic() {
        let stride = if cfg!(miri) { 4_093 } else { 1 };
        let outputs = [I24::MIN, I24::new(-1), I24::SILENCE, I24::new(1), I24::MAX];

        for raw in GAINS {
            let gain = Fixed::gain(Volume::from_raw(raw));
            let samples = (I24::MIN.value()..=I24::MAX.value())
                .step_by(stride)
                .chain([I24::MIN.value(), -1, 0, 1, I24::MAX.value()]);

            for (index, sample) in samples.enumerate() {
                let input = I24::new(sample);
                let output = outputs[index % outputs.len()];

                assert_eq!(
                    Fixed::amplify_sample(input, gain),
                    from_integer::<I24>(amplify_wide::<I24>(sample, gain))
                );
                assert_eq!(
                    Fixed::mix_sample(output, input, gain),
                    from_integer::<I24>(mix_wide::<I24>(output.value(), sample, gain))
                );
            }
        }
    }

    #[test]
    fn float() {
        averages::<Float>();
        round_trip::<Float>();
        saturates::<Float>();
        amplifies::<Float>();
    }

    #[test]
    #[cfg(not(any(feature = "fixed", feature = "float")))]
    fn default_matches_host_capabilities() {
        assert_eq!(RepresentationKind::default(), RepresentationKind::Float);
    }

    #[test]
    #[cfg(feature = "fixed")]
    fn fixed_feature_overrides_default() {
        assert_eq!(RepresentationKind::default(), RepresentationKind::Fixed);
    }

    #[test]
    #[cfg(all(feature = "float", not(feature = "fixed")))]
    fn float_feature_overrides_default() {
        assert_eq!(RepresentationKind::default(), RepresentationKind::Float);
    }
}
