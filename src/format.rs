#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    U8,
    I16,
    I32,
    F32,
}

impl Format {
    pub const fn sample_size(self) -> usize {
        match self {
            Format::U8 => 1,
            Format::I16 => 2,
            Format::I32 | Format::F32 => 4,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Configuration {
    pub sample_rate: u32,
    pub channels: u8,
    pub format: Format,
}

impl Configuration {
    pub const fn new(sample_rate: u32, channels: u8, format: Format) -> Self {
        Self {
            sample_rate,
            channels,
            format,
        }
    }

    pub const fn frame_size(&self) -> usize {
        self.channels as usize * self.format.sample_size()
    }

    pub const fn bytes_per_second(&self) -> usize {
        self.sample_rate as usize * self.frame_size()
    }

    pub const fn frames_to_bytes(&self, frames: usize) -> usize {
        frames * self.frame_size()
    }

    pub const fn bytes_to_frames(&self, bytes: usize) -> usize {
        match self.frame_size() {
            0 => 0,
            size => bytes / size,
        }
    }

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
    fn empty_configuration() {
        let configuration = Configuration::new(0, 0, Format::U8);

        assert_eq!(configuration.bytes_to_frames(16), 0);
        assert_eq!(configuration.frames_to_microseconds(16), 0);
    }
}
