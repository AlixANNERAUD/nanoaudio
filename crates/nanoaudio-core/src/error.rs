use core::fmt;

/// The errors returned by nanoaudio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Error {
    /// A mixer source index is out of range.
    InvalidSource,
    /// A file or frame header is malformed.
    InvalidHeader,
    /// The format or encoding is valid but not supported.
    UnsupportedFormat,
    /// The input ended before a complete header or value: retry with more bytes.
    Incomplete,
    /// A channel count is 0 or above [`MAXIMUM_CHANNELS`](crate::MAXIMUM_CHANNELS).
    TooManyChannels,
    /// An argument is outside its valid range.
    InvalidParameter,
    /// A buffer is smaller than the minimum the codec asks for.
    BufferTooSmall,
    /// The encoded data is corrupt.
    InvalidData,
    /// A seek needs input from this byte offset, counted like [`SeekRequest::byte_offset`](crate::codec::SeekRequest::byte_offset): drop buffered input and resume feeding from there.
    ///
    /// Only returned by `decode` after a [`Seek::seek`](crate::codec::Seek::seek), and possibly several times in a row while the decoder searches for the frame.
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

/// A result with [`Error`] as the error type.
pub type Result<T> = core::result::Result<T, Error>;
