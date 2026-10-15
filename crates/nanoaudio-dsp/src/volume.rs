use nanoaudio_core::{Format, I24, Sample};

use crate::bytes::{Bytes, each};
use crate::{DefaultRepresentation, Fixed, Float, Representation, RepresentationKind};

/// A volume, as an unsigned Q4.12 gain.
///
/// [`UNITY`](Self::UNITY) is 4096 and leaves samples unchanged, and [`MAXIMUM`](Self::MAXIMUM) is slightly under 16. Applying a volume rounds to the nearest value and saturates. The default is unity.
///
/// ```
/// use nanoaudio_dsp::Volume;
///
/// let volume = Volume::from_percent(50);
/// let mut samples = [1000i16, -1000];
///
/// volume.apply_to(&mut samples);
///
/// assert_eq!(volume.raw(), 2048);
/// assert_eq!(samples, [500, -500]);
/// ```
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Volume(u16);

impl Volume {
    /// The number of fractional bits.
    pub const FRACTIONAL_BITS: u32 = 12;

    /// A gain of 0.
    pub const MUTE: Self = Self(0);
    /// A gain of 1, 4096 in Q4.12.
    pub const UNITY: Self = Self(1 << Self::FRACTIONAL_BITS);
    /// The largest gain, slightly under 16.
    pub const MAXIMUM: Self = Self(u16::MAX);

    /// Creates a volume from a raw Q4.12 value.
    #[inline]
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    /// Returns the raw Q4.12 value.
    #[inline]
    pub const fn raw(self) -> u16 {
        self.0
    }

    /// Creates a volume from a percentage, 100 being unity, rounding down.
    #[inline]
    pub const fn from_percent(percent: u8) -> Self {
        Self(((percent as u32 * Self::UNITY.0 as u32) / 100) as u16)
    }

    /// Returns the volume as a percentage, 100 being unity, rounded to the nearest integer.
    #[inline]
    pub const fn percent(self) -> u16 {
        ((self.0 as u32 * 100 + Self::UNITY.0 as u32 / 2) / Self::UNITY.0 as u32) as u16
    }

    /// Returns `true` if the volume is [`MUTE`](Self::MUTE).
    #[inline]
    pub const fn is_mute(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` if the volume is [`UNITY`](Self::UNITY).
    #[inline]
    pub const fn is_unity(self) -> bool {
        self.0 == Self::UNITY.0
    }

    /// Multiplies two volumes, rounding down and saturating at [`MAXIMUM`](Self::MAXIMUM).
    #[inline]
    pub const fn combine(self, other: Self) -> Self {
        let combined = (self.0 as u32 * other.0 as u32) >> Self::FRACTIONAL_BITS;

        if combined > u16::MAX as u32 {
            Self::MAXIMUM
        } else {
            Self(combined as u16)
        }
    }

    /// Applies the volume to one sample, with the [`DefaultRepresentation`].
    #[inline]
    pub fn apply<S: Sample>(self, sample: S) -> S {
        self.apply_using::<DefaultRepresentation, S>(sample)
    }

    /// Applies the volume to one sample, with the representation `R`.
    #[inline]
    pub fn apply_using<R: Representation, S: Sample>(self, sample: S) -> S {
        R::amplify_sample(sample, R::gain(self))
    }

    /// Applies the volume to `samples` in place, with the [`DefaultRepresentation`].
    pub fn apply_to<S: Sample>(self, samples: &mut [S]) {
        self.apply_to_using::<DefaultRepresentation, S>(samples)
    }

    /// Applies the volume to `samples` in place, with the representation `R`.
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
            *sample = R::amplify_sample(*sample, gain);
        }
    }

    /// Applies the volume in place to little-endian samples in `format`, with the [`DefaultRepresentation`].
    ///
    /// Trailing bytes that do not form a whole sample are left unchanged.
    pub fn apply_to_bytes(self, bytes: &mut [u8], format: Format) {
        self.apply_to_bytes_using::<DefaultRepresentation>(bytes, format)
    }

    /// Like [`apply_to_bytes`](Self::apply_to_bytes), with the representation chosen at run time.
    pub fn apply_to_bytes_with(self, kind: RepresentationKind, bytes: &mut [u8], format: Format) {
        match kind {
            RepresentationKind::Fixed => self.apply_to_bytes_using::<Fixed>(bytes, format),
            RepresentationKind::Float => self.apply_to_bytes_using::<Float>(bytes, format),
        }
    }

    /// Like [`apply_to_bytes`](Self::apply_to_bytes), with the representation `R`.
    pub fn apply_to_bytes_using<R: Representation>(self, bytes: &mut [u8], format: Format) {
        match format {
            Format::U8 => self.apply_to_bytes_typed::<R, u8, 1>(bytes),
            Format::I16 => self.apply_to_bytes_typed::<R, i16, 2>(bytes),
            Format::I24 => self.apply_to_bytes_typed::<R, I24, 3>(bytes),
            Format::I32 => self.apply_to_bytes_typed::<R, i32, 4>(bytes),
            Format::F32 => self.apply_to_bytes_typed::<R, f32, 4>(bytes),
        }
    }

    fn apply_to_bytes_typed<R: Representation, S: Bytes<SIZE>, const SIZE: usize>(
        self,
        bytes: &mut [u8],
    ) {
        if self.is_unity() {
            return;
        }

        let gain = R::gain(self);

        each::<SIZE>(bytes, |chunk| {
            R::amplify_sample(S::from_bytes(chunk), gain).store(chunk);
        });
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
    use crate::bytes::testing::{FORMATS, GAINS, LENGTH, ROUNDS, fill, length};

    fn reference<R: Representation>(volume: Volume, bytes: &mut [u8], format: Format) {
        match format {
            Format::U8 => reference_typed::<R, u8>(volume, bytes),
            Format::I16 => reference_typed::<R, i16>(volume, bytes),
            Format::I24 => reference_typed::<R, I24>(volume, bytes),
            Format::I32 => reference_typed::<R, i32>(volume, bytes),
            Format::F32 => reference_typed::<R, f32>(volume, bytes),
        }
    }

    fn reference_typed<R: Representation, S: Sample>(volume: Volume, bytes: &mut [u8]) {
        if volume.is_unity() {
            return;
        }

        let gain = R::gain(volume);

        for chunk in bytes.chunks_exact_mut(S::SIZE) {
            R::amplify_sample(S::read_le(chunk), gain).write_le(chunk);
        }
    }

    fn bytes_match_reference<R: Representation>() {
        let mut expected = [0u8; LENGTH];

        for round in 0..ROUNDS {
            for format in FORMATS {
                for raw in GAINS {
                    let volume = Volume::from_raw(raw);
                    let length = length(round, raw as u64);
                    fill(&mut expected, format, round, raw as u64);
                    let mut actual = expected;

                    volume.apply_to_bytes_using::<R>(&mut actual[..length], format);
                    reference::<R>(volume, &mut expected[..length], format);

                    assert_eq!(actual, expected);
                }
            }
        }
    }

    #[test]
    fn fixed_bytes_match_reference() {
        bytes_match_reference::<Fixed>();
    }

    #[test]
    fn float_bytes_match_reference() {
        bytes_match_reference::<Float>();
    }

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
        assert_eq!(samples, [500, -500, 16_384]);

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
