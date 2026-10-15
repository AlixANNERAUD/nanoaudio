use crate::Format;

pub trait Sample: Copy + Default + PartialEq + Send + Sync + 'static {
    const FORMAT: Format;
    const SILENCE: Self;
    const SIZE: usize = Self::FORMAT.sample_size();

    fn to_i32(self) -> i32;

    fn from_i32(value: i32) -> Self;

    fn read_le(bytes: &[u8]) -> Self;

    fn write_le(self, bytes: &mut [u8]);

    fn convert<O: Sample>(self) -> O {
        O::from_i32(self.to_i32())
    }

    fn mix(self, other: Self) -> Self {
        Self::from_i32(self.to_i32().saturating_add(other.to_i32()))
    }
}

impl Sample for u8 {
    const FORMAT: Format = Format::U8;
    const SILENCE: Self = 0x80;

    fn to_i32(self) -> i32 {
        (self as i32 - 0x80) << 24
    }

    fn from_i32(value: i32) -> Self {
        ((value >> 24) + 0x80) as u8
    }

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
        (value >> 16) as i16
    }

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

    fn read_le(bytes: &[u8]) -> Self {
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    fn write_le(self, bytes: &mut [u8]) {
        bytes[..4].copy_from_slice(&self.to_le_bytes());
    }
}

const F32_SCALE: f32 = 2_147_483_648.0;

impl Sample for f32 {
    const FORMAT: Format = Format::F32;
    const SILENCE: Self = 0.0;

    fn to_i32(self) -> i32 {
        (self * F32_SCALE) as i32
    }

    fn from_i32(value: i32) -> Self {
        value as f32 / F32_SCALE
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
    fn mixing_saturates() {
        assert_eq!(i16::MAX.mix(i16::MAX), i16::MAX);
        assert_eq!(i16::MIN.mix(-1), i16::MIN);
        assert_eq!(100i16.mix(-40), 60);
        assert_eq!(0xF0u8.mix(0xF0), 0xFF);
        assert_eq!(u8::SILENCE.mix(u8::SILENCE), u8::SILENCE);
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
}
