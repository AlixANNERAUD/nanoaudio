#![no_std]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod chunk;
mod field;
mod flac;
mod id3v1;
mod id3v2;
mod mpeg;
mod text;

pub use chunk::*;
pub use field::*;
pub use flac::*;
pub use id3v1::*;
pub use id3v2::*;
pub use mpeg::*;
pub use text::*;

use core::fmt;

/// A range of bytes in the file, as an offset from the start of the file and a length.
///
/// Parsers return locations instead of copies, for example for picture data, and [`Error::Needs`] uses one to say which bytes to read next.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Location {
    /// Offset in bytes from the start of the file.
    pub offset: usize,
    /// Length in bytes.
    pub length: usize,
}

impl Location {
    /// Creates a location from an offset and a length in bytes.
    pub const fn new(offset: usize, length: usize) -> Self {
        Self { offset, length }
    }

    /// Returns the offset of the first byte after the location, saturating at `usize::MAX`.
    pub const fn end(&self) -> usize {
        self.offset.saturating_add(self.length)
    }

    /// Returns the bytes of this location from `bytes`, a part of the file that starts at file offset `offset`.
    ///
    /// Returns `None` when `bytes` does not cover the whole location.
    pub fn slice<'a>(&self, bytes: &'a [u8], offset: usize) -> Option<&'a [u8]> {
        let start = self.offset.checked_sub(offset)?;

        bytes.get(start..start.checked_add(self.length)?)
    }
}

/// Errors returned by the parsers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Error {
    /// The bytes given do not cover this location of the file: read it and call again with a part of the file that holds it.
    Needs(Location),
    /// The metadata is malformed.
    Invalid,
    /// The file is not a known container, or uses a version or feature that is not supported, such as compressed ID3v2.2 tags.
    Unsupported,
    /// The output buffer is too small for the decoded data.
    BufferTooSmall,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Needs(location) => write!(
                formatter,
                "needs {} bytes at offset {}",
                location.length, location.offset
            ),
            Error::Invalid => formatter.write_str("invalid metadata"),
            Error::Unsupported => formatter.write_str("unsupported metadata"),
            Error::BufferTooSmall => formatter.write_str("buffer too small"),
        }
    }
}

impl core::error::Error for Error {}

#[cfg(feature = "core")]
impl From<Error> for nanoaudio_core::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Needs(_) => nanoaudio_core::Error::Incomplete,
            Error::Invalid => nanoaudio_core::Error::InvalidData,
            Error::Unsupported => nanoaudio_core::Error::UnsupportedFormat,
            Error::BufferTooSmall => nanoaudio_core::Error::BufferTooSmall,
        }
    }
}

/// A `Result` with [`Error`] as the error type.
pub type Result<T> = core::result::Result<T, Error>;

/// A position in a list of entries, such as ID3v2 frames, FLAC blocks or RIFF chunks, that is walked one entry at a time.
///
/// A cursor holds no bytes, only where it is in the file, so the bytes can come from a different buffer at each call. Every call takes `bytes`, a part of the file, and `offset`, the position of `bytes[0]` in the file. When the next entry header is not in `bytes`, [`advance`](Self::advance) returns [`Error::Needs`] without moving, and the caller reads the location it names and calls again.
///
/// # Examples
///
/// Walking the chunks of a WAV file through a buffer of 8 bytes:
///
/// ```
/// use nanoaudio_metadata::{Cursor, Error, Riff};
///
/// let file: &[u8] = b"RIFF\x1c\0\0\0WAVEfmt \x04\0\0\0abcddata\x04\0\0\0\0\0\0\0";
/// let read = |offset: usize, length: usize| &file[offset..(offset + length).min(file.len())];
///
/// let mut chunks = Riff::parse(read(0, 12), 0)?.chunks();
/// let mut offset = 12;
/// let mut bytes = read(offset, 8);
/// let mut found = 0;
///
/// loop {
///     match chunks.advance(bytes, offset) {
///         Ok(Some(chunk)) => {
///             assert_eq!(chunk.identifier, [*b"fmt ", *b"data"][found]);
///             found += 1;
///         }
///         Ok(None) => break,
///         Err(Error::Needs(location)) => {
///             offset = location.offset;
///             bytes = read(location.offset, location.length);
///         }
///         Err(error) => return Err(error),
///     }
/// }
///
/// assert_eq!(found, 2);
/// # Ok::<(), Error>(())
/// ```
pub trait Cursor {
    /// The entry type, borrowing from the bytes given to [`advance`](Self::advance).
    type Item<'a>;

    /// Returns the next entry, or `None` at the end of the list.
    ///
    /// The entry's data is the part of it found in `bytes`, possibly empty or partial. When `bytes` does not hold the next entry header, it returns [`Error::Needs`] and the cursor stays where it was.
    fn advance<'a>(&mut self, bytes: &'a [u8], offset: usize) -> Result<Option<Self::Item<'a>>>;

    /// Turns the cursor into an iterator over the entries found in `bytes`, which starts at file offset `offset`.
    ///
    /// The iterator stops after the first error, which can be [`Error::Needs`] when `bytes` does not hold the whole list.
    fn items(self, bytes: &[u8], offset: usize) -> Items<'_, Self>
    where
        Self: Sized,
    {
        Items {
            cursor: self,
            bytes,
            offset,
            finished: false,
        }
    }
}

/// An iterator over the entries of a [`Cursor`] found in one buffer, returned by [`Cursor::items`].
#[derive(Debug, Clone)]
pub struct Items<'a, C> {
    cursor: C,
    bytes: &'a [u8],
    offset: usize,
    finished: bool,
}

impl<'a, C> Items<'a, C> {
    /// Returns the cursor, positioned after the last entry returned.
    pub fn cursor(&self) -> &C {
        &self.cursor
    }

    /// Returns the cursor, to resume with other bytes after an [`Error::Needs`].
    pub fn into_cursor(self) -> C {
        self.cursor
    }
}

impl<'a, C: Cursor> Iterator for Items<'a, C> {
    type Item = Result<C::Item<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        match self.cursor.advance(self.bytes, self.offset) {
            Ok(Some(item)) => Some(Ok(item)),
            Ok(None) => {
                self.finished = true;
                None
            }
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }
}

/// The kind of file found by [`detect`], with its parsed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Container {
    /// An ID3v2 tag, usually followed by MPEG audio.
    Id3v2(Id3v2Tag),
    /// A FLAC stream.
    Flac(Flac),
    /// A RIFF WAVE file.
    Wav(Riff),
    /// An AIFF or AIFF-C file.
    Aiff(Aiff),
    /// MPEG audio without an ID3v2 tag, starting with this frame header.
    Mpeg(FrameHeader),
}

/// Detects the kind of file from its first bytes, and parses its header.
///
/// `bytes` starts at file offset `offset`, normally 0. After an ID3v2 tag, call it again at [`Id3v2Tag::end`] to find what the tag precedes.
///
/// # Errors
///
/// [`Error::Needs`] when `bytes` is too short to tell or to parse the header, and [`Error::Unsupported`] when the file is none of the known kinds.
pub fn detect(bytes: &[u8], offset: usize) -> Result<Container> {
    if bytes.len() < 4 {
        return Err(Error::Needs(Location::new(offset, 4)));
    }

    let starts_with = |prefix: &[u8]| bytes.starts_with(prefix);

    if starts_with(b"ID3") {
        return Id3v2Tag::parse(bytes, offset).map(Container::Id3v2);
    }

    if starts_with(b"fLaC") {
        return Flac::parse(bytes, offset).map(Container::Flac);
    }

    if starts_with(b"RIFF") {
        return Riff::parse(bytes, offset).map(Container::Wav);
    }

    if starts_with(b"FORM") {
        return Aiff::parse(bytes, offset).map(Container::Aiff);
    }

    match bytes {
        [0xFF, second, ..] if second & 0xE0 == 0xE0 => {
            FrameHeader::parse(bytes, offset).map(Container::Mpeg)
        }
        _ => Err(Error::Unsupported),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Window<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Window<'a> {
    pub(crate) const fn new(bytes: &'a [u8], offset: usize) -> Self {
        Self { bytes, offset }
    }

    pub(crate) fn get(&self, location: Location) -> Result<&'a [u8]> {
        location
            .slice(self.bytes, self.offset)
            .ok_or(Error::Needs(location))
    }

    pub(crate) fn array<const N: usize>(&self, position: usize) -> Result<[u8; N]> {
        let bytes = self.get(Location::new(position, N))?;
        let mut array = [0; N];
        array.copy_from_slice(bytes);

        Ok(array)
    }

    pub(crate) fn available(&self, location: Location) -> &'a [u8] {
        let Some(start) = location.offset.checked_sub(self.offset) else {
            return &[];
        };
        let Some(rest) = self.bytes.get(start..) else {
            return &[];
        };

        &rest[..rest.len().min(location.length)]
    }
}

pub(crate) fn grow(location: Location, available: usize) -> Error {
    let length = available.saturating_mul(2).max(256).min(location.length);

    Error::Needs(Location::new(location.offset, length))
}

pub(crate) fn read_u16_be(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

pub(crate) fn read_u24_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]])
}

pub(crate) fn read_u32_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

pub(crate) fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// The size and colour depth of a picture, as declared in a FLAC picture block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dimensions {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bits per pixel.
    pub depth: u32,
    /// Number of colours for indexed pictures, 0 otherwise.
    pub colors: u32,
}

/// An attached picture, such as a cover, from an ID3v2 `APIC` frame or a FLAC picture block.
///
/// The image itself is not read: [`data`](Self::data) says where it is in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Picture<'a> {
    /// The picture type from the ID3v2 list, for example 3 for a front cover.
    pub picture_type: u32,
    /// The MIME type, such as `image/jpeg`, or a three-letter format such as `JPG` in ID3v2.2.
    pub mime_type: &'a [u8],
    /// The description.
    pub description: Text<'a>,
    /// The size and colour depth, only given by FLAC.
    pub dimensions: Option<Dimensions>,
    /// Where the image data is in the file.
    pub data: Location,
}
