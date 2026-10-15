use cpal::{
    BufferSize, SampleFormat, StreamConfig, SupportedStreamConfig, SupportedStreamConfigRange,
};
use nanoaudio_core::{Configuration, Format};

use crate::{
    Error,
    convert::{Encoding, Layout},
};

/// The configuration a device stream runs with, chosen to match a requested [`Configuration`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceConfiguration {
    /// Frames per second, in Hz.
    pub sample_rate: u32,
    /// Number of interleaved channels.
    pub channels: u16,
    /// Format of each sample, as cpal sees it.
    pub format: SampleFormat,
}

impl DeviceConfiguration {
    /// Returns the cpal stream configuration, with the backend's default buffer size.
    pub fn stream_config(&self) -> StreamConfig {
        StreamConfig {
            channels: self.channels,
            sample_rate: self.sample_rate,
            buffer_size: BufferSize::Default,
        }
    }

    pub(crate) fn layout(&self) -> Layout {
        Layout {
            encoding: Encoding::from_cpal(self.format).unwrap_or(Encoding::Packed(Format::F32)),
            channels: self.channels as u8,
        }
    }
}

pub(crate) fn stream_layout(configuration: Configuration) -> Layout {
    Layout {
        encoding: Encoding::Packed(configuration.format),
        channels: configuration.channels,
    }
}

pub(crate) fn preferred(default: SupportedStreamConfig) -> Configuration {
    let format = match Encoding::from_cpal(default.sample_format()) {
        Some(Encoding::Packed(format)) => format,
        Some(Encoding::Padded24) => Format::I24,
        None => Format::F32,
    };
    let channels = u8::try_from(default.channels()).unwrap_or(u8::MAX).max(1);

    Configuration::new(default.sample_rate(), channels, format)
}

pub(crate) fn negotiate(
    ranges: impl IntoIterator<Item = SupportedStreamConfigRange>,
    requested: Configuration,
) -> Result<DeviceConfiguration, Error> {
    if requested.sample_rate == 0 || requested.channels == 0 {
        return Err(Error::InvalidConfiguration);
    }

    let mut best: Option<((bool, u32, u32, u8), DeviceConfiguration)> = None;

    for range in ranges {
        if Encoding::from_cpal(range.sample_format()).is_none()
            || range.channels() == 0
            || range.channels() > u8::MAX as u16
            || range.min_sample_rate() > range.max_sample_rate()
        {
            continue;
        }

        let rate = requested
            .sample_rate
            .clamp(range.min_sample_rate(), range.max_sample_rate());
        let score = (
            range.channels() < requested.channels as u16,
            rate.abs_diff(requested.sample_rate),
            channel_cost(range.channels(), requested.channels),
            format_rank(range.sample_format(), requested.format),
        );

        if best.is_none_or(|(best, _)| score < best) {
            best = Some((
                score,
                DeviceConfiguration {
                    sample_rate: rate,
                    channels: range.channels(),
                    format: range.sample_format(),
                },
            ));
        }
    }

    best.map(|(_, configuration)| configuration)
        .ok_or(Error::UnsupportedFormat)
}

fn channel_cost(device: u16, requested: u8) -> u32 {
    let requested = requested as u32;
    let device = device as u32;

    if device >= requested {
        device - requested
    } else {
        0x1_0000 + requested - device
    }
}

fn format_rank(device: SampleFormat, requested: Format) -> u8 {
    match (Encoding::from_cpal(device), requested) {
        (Some(Encoding::Packed(device)), requested) if device == requested => 0,
        (Some(Encoding::Padded24), Format::I24) => 0,
        _ => match device {
            SampleFormat::F32 => 1,
            SampleFormat::I32 => 2,
            SampleFormat::I24 => 3,
            SampleFormat::I16 => 4,
            _ => 5,
        },
    }
}

#[cfg(test)]
mod tests {
    use cpal::SupportedBufferSize;

    use super::*;

    fn range(
        channels: u16,
        min: u32,
        max: u32,
        format: SampleFormat,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(channels, min, max, SupportedBufferSize::Unknown, format)
    }

    fn device(sample_rate: u32, channels: u16, format: SampleFormat) -> DeviceConfiguration {
        DeviceConfiguration {
            sample_rate,
            channels,
            format,
        }
    }

    #[test]
    fn prefers_an_exact_match() {
        let ranges = [
            range(2, 8_000, 192_000, SampleFormat::F32),
            range(2, 8_000, 192_000, SampleFormat::I16),
            range(1, 8_000, 192_000, SampleFormat::I16),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(44_100, 2, Format::I16)),
            Ok(device(44_100, 2, SampleFormat::I16))
        );
    }

    #[test]
    fn prefers_the_widest_format_when_none_matches() {
        let ranges = [
            range(2, 48_000, 48_000, SampleFormat::I16),
            range(2, 48_000, 48_000, SampleFormat::F32),
            range(2, 48_000, 48_000, SampleFormat::I32),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(48_000, 2, Format::U8)),
            Ok(device(48_000, 2, SampleFormat::F32))
        );
    }

    #[test]
    fn matches_24_bit_with_the_padded_format() {
        let ranges = [
            range(2, 48_000, 48_000, SampleFormat::F32),
            range(2, 48_000, 48_000, SampleFormat::I24),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(48_000, 2, Format::I24)),
            Ok(device(48_000, 2, SampleFormat::I24))
        );
    }

    #[test]
    fn channels_come_before_the_format() {
        let ranges = [
            range(1, 48_000, 48_000, SampleFormat::I16),
            range(2, 48_000, 48_000, SampleFormat::F32),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(48_000, 2, Format::I16)),
            Ok(device(48_000, 2, SampleFormat::F32))
        );
    }

    #[test]
    fn prefers_more_channels_over_fewer() {
        let ranges = [
            range(1, 48_000, 48_000, SampleFormat::F32),
            range(8, 48_000, 48_000, SampleFormat::F32),
            range(4, 48_000, 48_000, SampleFormat::F32),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(48_000, 2, Format::F32)),
            Ok(device(48_000, 4, SampleFormat::F32))
        );

        let ranges = [range(1, 48_000, 48_000, SampleFormat::F32)];

        assert_eq!(
            negotiate(ranges, Configuration::new(48_000, 2, Format::F32)),
            Ok(device(48_000, 1, SampleFormat::F32))
        );
    }

    #[test]
    fn falls_back_to_the_nearest_sample_rate() {
        let ranges = [
            range(2, 48_000, 48_000, SampleFormat::F32),
            range(2, 88_200, 96_000, SampleFormat::F32),
            range(2, 44_000, 44_000, SampleFormat::F64),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(44_100, 2, Format::I16)),
            Ok(device(48_000, 2, SampleFormat::F32))
        );

        let ranges = [
            range(2, 44_100, 48_000, SampleFormat::F32),
            range(2, 96_000, 96_000, SampleFormat::I16),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(22_050, 2, Format::I16)),
            Ok(device(44_100, 2, SampleFormat::F32))
        );
    }

    #[test]
    fn prefers_the_requested_rate_over_the_format() {
        let ranges = [
            range(2, 48_000, 48_000, SampleFormat::I16),
            range(2, 44_100, 44_100, SampleFormat::F32),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(44_100, 2, Format::I16)),
            Ok(device(44_100, 2, SampleFormat::F32))
        );
    }

    #[test]
    fn keeps_the_channels_before_the_rate() {
        let ranges = [
            range(1, 44_100, 44_100, SampleFormat::I16),
            range(2, 48_000, 48_000, SampleFormat::I16),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(44_100, 2, Format::I16)),
            Ok(device(48_000, 2, SampleFormat::I16))
        );
    }

    #[test]
    fn skips_unsupported_formats() {
        let ranges = [
            range(2, 8_000, 192_000, SampleFormat::F64),
            range(2, 8_000, 192_000, SampleFormat::U16),
        ];

        assert_eq!(
            negotiate(ranges, Configuration::new(44_100, 2, Format::I16)),
            Err(Error::UnsupportedFormat)
        );
        assert_eq!(
            negotiate([], Configuration::new(44_100, 2, Format::I16)),
            Err(Error::UnsupportedFormat)
        );
    }

    #[test]
    fn rejects_an_empty_configuration() {
        let ranges = [range(2, 8_000, 192_000, SampleFormat::F32)];

        assert_eq!(
            negotiate(ranges, Configuration::new(0, 2, Format::I16)),
            Err(Error::InvalidConfiguration)
        );
        assert_eq!(
            negotiate(ranges, Configuration::new(44_100, 0, Format::I16)),
            Err(Error::InvalidConfiguration)
        );
    }

    #[test]
    fn preferred_follows_the_device_default() {
        let default = |channels, format| {
            SupportedStreamConfig::new(channels, 48_000, SupportedBufferSize::Unknown, format)
        };

        assert_eq!(
            preferred(default(2, SampleFormat::F32)),
            Configuration::new(48_000, 2, Format::F32)
        );
        assert_eq!(
            preferred(default(2, SampleFormat::I24)),
            Configuration::new(48_000, 2, Format::I24)
        );
        assert_eq!(
            preferred(default(1, SampleFormat::F64)),
            Configuration::new(48_000, 1, Format::F32)
        );
    }

    #[test]
    fn device_configuration_maps_to_cpal() {
        let configuration = device(44_100, 2, SampleFormat::I24);

        assert_eq!(configuration.stream_config().sample_rate, 44_100);
        assert_eq!(configuration.stream_config().channels, 2);
        assert_eq!(
            configuration.layout(),
            Layout {
                encoding: Encoding::Padded24,
                channels: 2
            }
        );
    }
}
