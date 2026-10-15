#![no_std]
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Location {
    pub offset: usize,
    pub length: usize,
}

impl Location {
    pub const fn new(offset: usize, length: usize) -> Self {
        Self { offset, length }
    }

    pub const fn end(&self) -> usize {
        self.offset.saturating_add(self.length)
    }

    pub fn slice<'a>(&self, bytes: &'a [u8], offset: usize) -> Option<&'a [u8]> {
        let start = self.offset.checked_sub(offset)?;

        bytes.get(start..start.checked_add(self.length)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Error {
    Needs(Location),
    Invalid,
    Unsupported,
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

pub type Result<T> = core::result::Result<T, Error>;

pub trait Cursor {
    type Item<'a>;

    fn advance<'a>(&mut self, bytes: &'a [u8], offset: usize) -> Result<Option<Self::Item<'a>>>;

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

#[derive(Debug, Clone)]
pub struct Items<'a, C> {
    cursor: C,
    bytes: &'a [u8],
    offset: usize,
    finished: bool,
}

impl<'a, C> Items<'a, C> {
    pub fn cursor(&self) -> &C {
        &self.cursor
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    Id3v2(Id3v2Tag),
    Flac(Flac),
    Wav(Riff),
    Aiff(Aiff),
    Mpeg(FrameHeader),
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub colors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Picture<'a> {
    pub picture_type: u32,
    pub mime_type: &'a [u8],
    pub description: Text<'a>,
    pub dimensions: Option<Dimensions>,
    pub data: Location,
}
