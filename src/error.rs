use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Error {
    InvalidSource,
    InvalidHeader,
    UnsupportedFormat,
    Incomplete,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidSource => formatter.write_str("invalid mixer source"),
            Error::InvalidHeader => formatter.write_str("invalid header"),
            Error::UnsupportedFormat => formatter.write_str("unsupported format"),
            Error::Incomplete => formatter.write_str("incomplete data"),
        }
    }
}

impl core::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
