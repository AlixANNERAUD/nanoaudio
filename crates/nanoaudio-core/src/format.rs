use crate::{Error, Result};

/// A PCM sample format.
///
/// In byte buffers, samples are little-endian. The `u8` value of each variant is stable, to cross an ABI.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Unsigned 8-bit samples, with silence at `0x80`.
    U8,
    /// Signed 16-bit samples.
    I16,
    /// Signed 32-bit samples.
    I32,
    /// 32-bit floating-point samples, with full scale at -1.0 and 1.0.
    F32,
    /// Signed 24-bit samples, stored in 3 bytes.
    I24,
}

impl Format {
    /// Returns the size of one sample in bytes.
    pub const fn sample_size(self) -> usize {
        match self {
            Format::U8 => 1,
            Format::I16 => 2,
            Format::I24 => 3,
            Format::I32 | Format::F32 => 4,
        }
    }
}

impl TryFrom<u8> for Format {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Format::U8),
            1 => Ok(Format::I16),
            2 => Ok(Format::I32),
            3 => Ok(Format::F32),
            4 => Ok(Format::I24),
            _ => Err(Error::UnsupportedFormat),
        }
    }
}

impl From<Format> for u8 {
    fn from(format: Format) -> Self {
        format as u8
    }
}

/// The largest number of channels a stream or codec supports.
pub const MAXIMUM_CHANNELS: usize = 8;

/// The sample rate, channel count and sample format of a stream.
///
/// It is `#[repr(C)]`, to cross an ABI.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Configuration {
    /// Frames per second, in Hz.
    pub sample_rate: u32,
    /// Number of interleaved channels.
    pub channels: u8,
    /// Format of each sample.
    pub format: Format,
}

impl Configuration {
    /// Creates a configuration.
    pub const fn new(sample_rate: u32, channels: u8, format: Format) -> Self {
        Self {
            sample_rate,
            channels,
            format,
        }
    }

    /// Returns the size of one frame in bytes, a frame being one sample per channel.
    pub const fn frame_size(&self) -> usize {
        self.channels as usize * self.format.sample_size()
    }

    /// Returns the data rate in bytes per second.
    pub const fn bytes_per_second(&self) -> usize {
        self.sample_rate as usize * self.frame_size()
    }

    /// Converts a number of frames to bytes.
    pub const fn frames_to_bytes(&self, frames: usize) -> usize {
        frames * self.frame_size()
    }

    /// Converts a number of bytes to whole frames, rounding down, or 0 if the frame size is 0.
    pub const fn bytes_to_frames(&self, bytes: usize) -> usize {
        match self.frame_size() {
            0 => 0,
            size => bytes / size,
        }
    }

    /// Converts a number of frames to a duration in microseconds, rounding down, or 0 if the sample rate is 0.
    pub const fn frames_to_microseconds(&self, frames: usize) -> u64 {
        match self.sample_rate {
            0 => 0,
            rate => frames as u64 * 1_000_000 / rate as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        let configuration = Configuration::new(44_100, 2, Format::I16);

        assert_eq!(configuration.frame_size(), 4);
        assert_eq!(configuration.bytes_per_second(), 176_400);
        assert_eq!(configuration.frames_to_bytes(10), 40);
        assert_eq!(configuration.bytes_to_frames(41), 10);
        assert_eq!(configuration.frames_to_microseconds(44_100), 1_000_000);
    }

    #[test]
    fn format_from_u8() {
        for format in [
            Format::U8,
            Format::I16,
            Format::I24,
            Format::I32,
            Format::F32,
        ] {
            assert_eq!(Format::try_from(u8::from(format)), Ok(format));
        }

        assert_eq!(Format::try_from(5), Err(Error::UnsupportedFormat));
    }

    #[test]
    fn empty_configuration() {
        let configuration = Configuration::new(0, 0, Format::U8);

        assert_eq!(configuration.bytes_to_frames(16), 0);
        assert_eq!(configuration.frames_to_microseconds(16), 0);
    }
}
