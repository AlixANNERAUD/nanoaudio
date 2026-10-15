use crate::{Sample, Volume};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepresentationKind {
    Fixed,
    Float,
}

impl RepresentationKind {
    pub const DEFAULT: Self = DefaultRepresentation::KIND;
}

impl Default for RepresentationKind {
    fn default() -> Self {
        Self::DEFAULT
    }
}

pub trait Representation: Copy + Default + Send + Sync + 'static {
    type Value: Copy;
    type Gain: Copy;

    const KIND: RepresentationKind;

    fn from_sample<S: Sample>(sample: S) -> Self::Value;

    fn to_sample<S: Sample>(value: Self::Value) -> S;

    fn gain(volume: Volume) -> Self::Gain;

    fn amplify(value: Self::Value, gain: Self::Gain) -> Self::Value;

    fn add(left: Self::Value, right: Self::Value) -> Self::Value;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Fixed;

impl Representation for Fixed {
    type Value = i32;
    type Gain = i32;

    const KIND: RepresentationKind = RepresentationKind::Fixed;

    fn from_sample<S: Sample>(sample: S) -> i32 {
        sample.to_i32()
    }

    fn to_sample<S: Sample>(value: i32) -> S {
        S::from_i32(value)
    }

    fn gain(volume: Volume) -> i32 {
        volume.raw() as i32
    }

    fn amplify(value: i32, gain: i32) -> i32 {
        let scaled = (value as i64 * gain as i64) >> Volume::FRACTIONAL_BITS;

        scaled.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    fn add(left: i32, right: i32) -> i32 {
        left.saturating_add(right)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Float;

impl Representation for Float {
    type Value = f32;
    type Gain = f32;

    const KIND: RepresentationKind = RepresentationKind::Float;

    fn from_sample<S: Sample>(sample: S) -> f32 {
        sample.to_f32()
    }

    fn to_sample<S: Sample>(value: f32) -> S {
        S::from_f32(value)
    }

    fn gain(volume: Volume) -> f32 {
        volume.raw() as f32 / Volume::UNITY.raw() as f32
    }

    fn amplify(value: f32, gain: f32) -> f32 {
        value * gain
    }

    fn add(left: f32, right: f32) -> f32 {
        left + right
    }
}

#[cfg(all(feature = "fixed", feature = "float"))]
compile_error!("the `fixed` and `float` features are mutually exclusive");

#[cfg(nanoaudio_float)]
pub type DefaultRepresentation = Float;

#[cfg(not(nanoaudio_float))]
pub type DefaultRepresentation = Fixed;

#[cfg(test)]
mod tests {
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
            R::to_sample::<f32>(R::add(R::from_sample(1.0f32), R::from_sample(1.0f32))),
            1.0
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

    #[test]
    fn fixed() {
        round_trip::<Fixed>();
        saturates::<Fixed>();
        amplifies::<Fixed>();
    }

    #[test]
    fn float() {
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
    #[cfg(feature = "float")]
    fn float_feature_overrides_default() {
        assert_eq!(RepresentationKind::default(), RepresentationKind::Float);
    }
}
