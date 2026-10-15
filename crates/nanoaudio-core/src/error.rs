use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Error {
    InvalidSource,
    InvalidHeader,
    UnsupportedFormat,
    Incomplete,
    TooManyChannels,
    InvalidParameter,
    BufferTooSmall,
    InvalidData,
    /// A seek needs input from this byte offset, counted like [`SeekRequest::byte_offset`](crate::codec::SeekRequest::byte_offset): drop buffered input and resume feeding from there.
    SeekTo(u64),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidSource => formatter.write_str("invalid mixer source"),
            Error::InvalidHeader => formatter.write_str("invalid header"),
            Error::UnsupportedFormat => formatter.write_str("unsupported format"),
            Error::Incomplete => formatter.write_str("incomplete data"),
            Error::TooManyChannels => formatter.write_str("too many channels"),
            Error::InvalidParameter => formatter.write_str("invalid parameter"),
            Error::BufferTooSmall => formatter.write_str("buffer too small"),
            Error::InvalidData => formatter.write_str("invalid data"),
            Error::SeekTo(offset) => write!(formatter, "seek to byte {offset}"),
        }
    }
}

impl core::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
