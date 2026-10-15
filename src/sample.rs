use crate::Format;

pub trait Sample: Copy + Default + PartialEq + Send + Sync + 'static {
    const FORMAT: Format;
    const SILENCE: Self;
    const SIZE: usize = Self::FORMAT.sample_size();

    fn to_i32(self) -> i32;

    fn from_i32(value: i32) -> Self;

    fn to_f32(self) -> f32;

    fn from_f32(value: f32) -> Self;

    fn read_le(bytes: &[u8]) -> Self;

    fn write_le(self, bytes: &mut [u8]);

    fn convert<O: Sample>(self) -> O {
        O::from_i32(self.to_i32())
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

    fn to_f32(self) -> f32 {
        (self as f32 - 128.0) / 128.0
    }

    fn from_f32(value: f32) -> Self {
        (value * 128.0 + 128.0) as u8
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

    fn to_f32(self) -> f32 {
        self as f32 / 32_768.0
    }

    fn from_f32(value: f32) -> Self {
        (value * 32_768.0) as i16
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

    fn to_f32(self) -> f32 {
        self as f32 / F32_SCALE
    }

    fn from_f32(value: f32) -> Self {
        (value * F32_SCALE) as i32
    }

    fn read_le(bytes: &[u8]) -> Self {
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    fn write_le(self, bytes: &mut [u8]) {
        bytes[..4].copy_from_slice(&self.to_le_bytes());
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct I24(i32);

impl I24 {
    pub const MIN: Self = Self(-(1 << 23));
    pub const MAX: Self = Self((1 << 23) - 1);

    pub const fn new(value: i32) -> Self {
        if value < Self::MIN.0 {
            Self::MIN
        } else if value > Self::MAX.0 {
            Self::MAX
        } else {
            Self(value)
        }
    }

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
        Self(value >> 8)
    }

    fn to_f32(self) -> f32 {
        self.0 as f32 / I24_SCALE
    }

    fn from_f32(value: f32) -> Self {
        Self::new((value * I24_SCALE) as i32)
    }

    fn read_le(bytes: &[u8]) -> Self {
        Self(i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8)
    }

    fn write_le(self, bytes: &mut [u8]) {
        bytes[..3].copy_from_slice(&self.0.to_le_bytes()[..3]);
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

    fn to_f32(self) -> f32 {
        self
    }

    fn from_f32(value: f32) -> Self {
        value.clamp(-1.0, 1.0)
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
        assert_eq!(f32::from_f32(3.0), 1.0);
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
