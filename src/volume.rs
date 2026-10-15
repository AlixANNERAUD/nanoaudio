use crate::{
    DefaultRepresentation, Fixed, Float, Format, Representation, RepresentationKind, Sample,
};

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Volume(u16);

impl Volume {
    pub const FRACTIONAL_BITS: u32 = 12;

    pub const MUTE: Self = Self(0);
    pub const UNITY: Self = Self(1 << Self::FRACTIONAL_BITS);
    pub const MAXIMUM: Self = Self(u16::MAX);

    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }

    pub const fn from_percent(percent: u8) -> Self {
        Self(((percent as u32 * Self::UNITY.0 as u32) / 100) as u16)
    }

    pub const fn percent(self) -> u16 {
        ((self.0 as u32 * 100 + Self::UNITY.0 as u32 / 2) / Self::UNITY.0 as u32) as u16
    }

    pub const fn is_mute(self) -> bool {
        self.0 == 0
    }

    pub const fn is_unity(self) -> bool {
        self.0 == Self::UNITY.0
    }

    pub const fn combine(self, other: Self) -> Self {
        let combined = (self.0 as u32 * other.0 as u32) >> Self::FRACTIONAL_BITS;

        if combined > u16::MAX as u32 {
            Self::MAXIMUM
        } else {
            Self(combined as u16)
        }
    }

    pub fn apply<S: Sample>(self, sample: S) -> S {
        self.apply_using::<DefaultRepresentation, S>(sample)
    }

    pub fn apply_using<R: Representation, S: Sample>(self, sample: S) -> S {
        R::to_sample(R::amplify(R::from_sample(sample), R::gain(self)))
    }

    pub fn apply_to<S: Sample>(self, samples: &mut [S]) {
        self.apply_to_using::<DefaultRepresentation, S>(samples)
    }

    pub fn apply_to_using<R: Representation, S: Sample>(self, samples: &mut [S]) {
        if self.is_unity() {
            return;
        }

        if self.is_mute() {
            samples.fill(S::SILENCE);
            return;
        }

        let gain = R::gain(self);

        for sample in samples {
            *sample = R::to_sample(R::amplify(R::from_sample(*sample), gain));
        }
    }

    pub fn apply_to_bytes(self, bytes: &mut [u8], format: Format) {
        self.apply_to_bytes_using::<DefaultRepresentation>(bytes, format)
    }

    pub fn apply_to_bytes_with(self, kind: RepresentationKind, bytes: &mut [u8], format: Format) {
        match kind {
            RepresentationKind::Fixed => self.apply_to_bytes_using::<Fixed>(bytes, format),
            RepresentationKind::Float => self.apply_to_bytes_using::<Float>(bytes, format),
        }
    }

    pub fn apply_to_bytes_using<R: Representation>(self, bytes: &mut [u8], format: Format) {
        match format {
            Format::U8 => self.apply_to_bytes_typed::<R, u8>(bytes),
            Format::I16 => self.apply_to_bytes_typed::<R, i16>(bytes),
            Format::I32 => self.apply_to_bytes_typed::<R, i32>(bytes),
            Format::F32 => self.apply_to_bytes_typed::<R, f32>(bytes),
        }
    }

    fn apply_to_bytes_typed<R: Representation, S: Sample>(self, bytes: &mut [u8]) {
        if self.is_unity() {
            return;
        }

        let gain = R::gain(self);

        for chunk in bytes.chunks_exact_mut(S::SIZE) {
            R::to_sample::<S>(R::amplify(R::from_sample(S::read_le(chunk)), gain)).write_le(chunk);
        }
    }
}

impl Default for Volume {
    fn default() -> Self {
        Self::UNITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent() {
        assert_eq!(Volume::from_percent(100), Volume::UNITY);
        assert_eq!(Volume::from_percent(0), Volume::MUTE);
        assert_eq!(Volume::from_percent(50).percent(), 50);
        assert_eq!(Volume::from_percent(200).percent(), 200);
    }

    #[test]
    fn combine() {
        assert_eq!(
            Volume::UNITY.combine(Volume::from_percent(40)),
            Volume::from_percent(40)
        );
        assert_eq!(
            Volume::from_percent(50)
                .combine(Volume::from_percent(50))
                .percent(),
            25
        );
        assert_eq!(Volume::MAXIMUM.combine(Volume::MAXIMUM), Volume::MAXIMUM);
        assert_eq!(Volume::MUTE.combine(Volume::MAXIMUM), Volume::MUTE);
    }

    fn apply_to_samples<R: Representation>() {
        let mut samples = [1000i16, -1000, i16::MAX];

        Volume::from_percent(50).apply_to_using::<R, _>(&mut samples);
        assert_eq!(samples, [500, -500, i16::MAX / 2]);

        Volume::MAXIMUM.apply_to_using::<R, _>(&mut samples);
        assert!((7999..=8000).contains(&samples[0]));
        assert!((-8000..=-7999).contains(&samples[1]));
        assert_eq!(samples[2], i16::MAX);

        Volume::MUTE.apply_to_using::<R, _>(&mut samples);
        assert_eq!(samples, [0, 0, 0]);
    }

    #[test]
    fn apply_to_samples_fixed() {
        apply_to_samples::<Fixed>();
    }

    #[test]
    fn apply_to_samples_float() {
        apply_to_samples::<Float>();
    }

    #[test]
    fn apply_to_bytes_at_runtime() {
        for kind in [RepresentationKind::Fixed, RepresentationKind::Float] {
            let mut bytes = [0u8; 4];
            0.5f32.write_le(&mut bytes);

            Volume::from_percent(50).apply_to_bytes_with(kind, &mut bytes, Format::F32);

            assert_eq!(f32::read_le(&bytes), 0.25);
        }
    }

    #[test]
    fn apply_to_unsigned_samples() {
        let mut samples = [0xFFu8, 0x00, 0x80];

        Volume::MUTE.apply_to(&mut samples);
        assert_eq!(samples, [0x80; 3]);
    }

    #[test]
    fn apply_to_bytes() {
        let mut bytes = [0u8; 4];
        1000i16.write_le(&mut bytes[..2]);
        (-1000i16).write_le(&mut bytes[2..]);

        Volume::from_percent(25).apply_to_bytes(&mut bytes, Format::I16);

        assert_eq!(i16::read_le(&bytes[..2]), 250);
        assert_eq!(i16::read_le(&bytes[2..]), -250);
    }
}
