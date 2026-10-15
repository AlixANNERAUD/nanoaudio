use std::fmt;

/// An error opening or controlling a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The host has no default device in that direction.
    NoDevice,
    /// The requested sample rate or channel count is 0.
    InvalidConfiguration,
    /// The device cannot run at the requested sample rate.
    ///
    /// `nearest` is the closest rate it supports, to open the stream again at that rate.
    UnsupportedSampleRate {
        /// The requested rate, in Hz.
        requested: u32,
        /// The supported rate closest to it, in Hz.
        nearest: u32,
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
            Self::UnsupportedSampleRate { requested, nearest } => write!(
                formatter,
                "the device cannot run at {requested} Hz, the nearest supported rate is {nearest} Hz"
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
            requested: 44_100,
            nearest: 48_000,
        };

        assert_eq!(
            error.to_string(),
            "the device cannot run at 44100 Hz, the nearest supported rate is 48000 Hz"
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
