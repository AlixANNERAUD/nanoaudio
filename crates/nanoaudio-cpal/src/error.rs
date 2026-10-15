use std::fmt;

/// An error opening or controlling a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The host has no default device in that direction.
    NoDevice,
    /// The requested sample rate or channel count is 0.
    InvalidConfiguration,
    /// The stream cannot be resampled to or from the rate the device runs at.
    ///
    /// This happens when [`Quality::Sinc`](crate::Quality::Sinc) would downsample by more than [`MAXIMUM_RATIO`](crate::MAXIMUM_RATIO), or when the stream has more than [`MAXIMUM_CHANNELS`](nanoaudio_core::MAXIMUM_CHANNELS) channels.
    UnsupportedSampleRate {
        /// The rate of the stream, in Hz.
        requested: u32,
        /// The rate of the device, in Hz.
        device: u32,
    },
    /// The device offers no sample format this crate converts to or from.
    UnsupportedFormat,
    /// An error from cpal.
    Backend(cpal::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => formatter.write_str("no default audio device"),
            Self::InvalidConfiguration => {
                formatter.write_str("the sample rate and channel count must not be 0")
            }
            Self::UnsupportedSampleRate { requested, device } => write!(
                formatter,
                "cannot resample between {requested} Hz and the device rate of {device} Hz"
            ),
            Self::UnsupportedFormat => {
                formatter.write_str("the device offers no supported sample format")
            }
            Self::Backend(error) => write!(formatter, "audio backend error: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(error) => Some(error),
            _ => None,
        }
    }
}

impl From<cpal::Error> for Error {
    fn from(error: cpal::Error) -> Self {
        Self::Backend(error)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn displays_the_sample_rates() {
        let error = Error::UnsupportedSampleRate {
            requested: 384_000,
            device: 8_000,
        };

        assert_eq!(
            error.to_string(),
            "cannot resample between 384000 Hz and the device rate of 8000 Hz"
        );
        assert!(error.source().is_none());
    }

    #[test]
    fn wraps_backend_errors() {
        let error = Error::from(cpal::Error::from(cpal::ErrorKind::DeviceNotAvailable));

        assert!(matches!(error, Error::Backend(_)));
        assert!(error.source().is_some());
        assert!(error.to_string().starts_with("audio backend error"));
        assert_eq!(Error::NoDevice.to_string(), "no default audio device");
    }
}
