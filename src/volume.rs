use crate::{Format, Sample};

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Volume(u16);

impl Volume {
    const FRACTIONAL_BITS: u32 = 12;

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

    pub fn apply<S: Sample>(self, sample: S) -> S {
        let scaled = (sample.to_i32() as i64 * self.0 as i64) >> Self::FRACTIONAL_BITS;

        S::from_i32(scaled.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
    }

    pub fn apply_to<S: Sample>(self, samples: &mut [S]) {
        if self.is_unity() {
            return;
        }

        if self.is_mute() {
            samples.fill(S::SILENCE);
            return;
        }

        for sample in samples {
            *sample = self.apply(*sample);
        }
    }

    pub fn apply_to_bytes(self, bytes: &mut [u8], format: Format) {
        match format {
            Format::U8 => self.apply_to_bytes_typed::<u8>(bytes),
            Format::I16 => self.apply_to_bytes_typed::<i16>(bytes),
            Format::I32 => self.apply_to_bytes_typed::<i32>(bytes),
            Format::F32 => self.apply_to_bytes_typed::<f32>(bytes),
        }
    }

    fn apply_to_bytes_typed<S: Sample>(self, bytes: &mut [u8]) {
        if self.is_unity() {
            return;
        }

        for chunk in bytes.chunks_exact_mut(S::SIZE) {
            self.apply(S::read_le(chunk)).write_le(chunk);
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
    fn apply_to_samples() {
        let mut samples = [1000i16, -1000, i16::MAX];

        Volume::from_percent(50).apply_to(&mut samples);
        assert_eq!(samples, [500, -500, i16::MAX / 2]);

        Volume::MAXIMUM.apply_to(&mut samples);
        assert_eq!(samples, [7999, -8000, i16::MAX]);

        Volume::MUTE.apply_to(&mut samples);
        assert_eq!(samples, [0, 0, 0]);
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
