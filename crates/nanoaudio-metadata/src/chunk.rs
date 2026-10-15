use super::{
    Cursor, Encoding, Error, Field, Id3v2Tag, Location, Result, Text, Window, read_u32_be,
    read_u32_le,
};

const FORM_HEADER_LENGTH: usize = 12;
const CHUNK_HEADER_LENGTH: usize = 8;

/// The kind of chunk list a [`Chunk`] comes from, which sets the byte order of chunk sizes and the meaning of identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChunkFormat {
    /// Top-level chunks of a RIFF WAVE file, with little-endian sizes.
    Riff,
    /// Entries of a RIFF `LIST` `INFO` chunk, with little-endian sizes.
    RiffInfo,
    /// Chunks of an AIFF or AIFF-C file, with big-endian sizes.
    Aiff,
}

impl ChunkFormat {
    const fn read_size(self, bytes: &[u8]) -> u32 {
        let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];

        match self {
            ChunkFormat::Riff | ChunkFormat::RiffInfo => u32::from_le_bytes(bytes),
            ChunkFormat::Aiff => u32::from_be_bytes(bytes),
        }
    }
}

/// The header of a RIFF WAVE file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Riff {
    /// The whole `RIFF` chunk, header included.
    pub location: Location,
}

impl Riff {
    /// Parses the 12-byte `RIFF` header at file offset `offset`, which is also where `bytes` starts.
    pub fn parse(bytes: &[u8], offset: usize) -> Result<Self> {
        let header = Window::new(bytes, offset).array::<FORM_HEADER_LENGTH>(offset)?;

        if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
            return Err(Error::Invalid);
        }

        let size = read_u32_le(&header[4..8]) as usize;

        Ok(Self {
            location: Location::new(offset, size.saturating_add(CHUNK_HEADER_LENGTH)),
        })
    }

    /// Returns a cursor over the top-level chunks, such as `fmt `, `LIST`, `data` and `id3 `.
    pub fn chunks(&self) -> Chunks {
        Chunks::new(
            self.location.offset + FORM_HEADER_LENGTH,
            self.location.end(),
            ChunkFormat::Riff,
        )
    }
}

/// The header of an AIFF or AIFF-C file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Aiff {
    /// The whole `FORM` chunk, header included.
    pub location: Location,
    /// `true` for AIFF-C.
    pub compressed: bool,
}

impl Aiff {
    /// Parses the 12-byte `FORM` header at file offset `offset`, which is also where `bytes` starts.
    pub fn parse(bytes: &[u8], offset: usize) -> Result<Self> {
        let header = Window::new(bytes, offset).array::<FORM_HEADER_LENGTH>(offset)?;

        let compressed = match (&header[0..4], &header[8..12]) {
            (b"FORM", b"AIFF") => false,
            (b"FORM", b"AIFC") => true,
            _ => return Err(Error::Invalid),
        };
        let size = read_u32_be(&header[4..8]) as usize;

        Ok(Self {
            location: Location::new(offset, size.saturating_add(CHUNK_HEADER_LENGTH)),
            compressed,
        })
    }

    /// Returns a cursor over the chunks, such as `COMM`, `SSND`, `NAME` and `ID3 `.
    pub fn chunks(&self) -> Chunks {
        Chunks::new(
            self.location.offset + FORM_HEADER_LENGTH,
            self.location.end(),
            ChunkFormat::Aiff,
        )
    }
}

/// A [`Cursor`] over a list of chunks, returned by [`Riff::chunks`], [`Aiff::chunks`] and [`Chunk::info`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chunks {
    position: usize,
    end: usize,
    format: ChunkFormat,
}

impl Chunks {
    const fn new(position: usize, end: usize, format: ChunkFormat) -> Self {
        Self {
            position,
            end,
            format,
        }
    }

    /// Returns the file offset of the next chunk header.
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Returns the kind of chunk list being walked.
    pub const fn format(&self) -> ChunkFormat {
        self.format
    }
}

impl Cursor for Chunks {
    type Item<'a> = Chunk<'a>;

    fn advance<'a>(&mut self, bytes: &'a [u8], offset: usize) -> Result<Option<Chunk<'a>>> {
        if self.end.saturating_sub(self.position) < CHUNK_HEADER_LENGTH {
            return Ok(None);
        }

        let window = Window::new(bytes, offset);
        let header = window.array::<CHUNK_HEADER_LENGTH>(self.position)?;
        let size = self.format.read_size(&header[4..8]) as usize;
        let location = Location::new(self.position + CHUNK_HEADER_LENGTH, size);

        if self.format == ChunkFormat::RiffInfo && location.end() > self.end {
            return Err(Error::Invalid);
        }

        self.position = location.end().saturating_add(size & 1);

        Ok(Some(Chunk {
            identifier: [header[0], header[1], header[2], header[3]],
            format: self.format,
            location,
            data: window.available(location),
        }))
    }
}

/// A chunk, with as much of its data as the buffer given to the cursor held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chunk<'a> {
    /// The four-character identifier, such as `fmt ` or `NAME`.
    pub identifier: [u8; 4],
    /// The kind of chunk list it comes from.
    pub format: ChunkFormat,
    /// Where the chunk data is in the file, header excluded.
    pub location: Location,
    /// The part of the chunk data found in the buffer, which can be empty or partial.
    pub data: &'a [u8],
}

impl<'a> Chunk<'a> {
    /// Returns the same chunk with its data taken from `bytes`, a part of the file that starts at file offset `offset`.
    ///
    /// Use it after reading [`location`](Self::location) into a buffer of its own.
    pub fn within<'b>(&self, bytes: &'b [u8], offset: usize) -> Chunk<'b> {
        Chunk {
            data: Window::new(bytes, offset).available(self.location),
            ..*self
        }
    }
    /// Returns `true` when [`data`](Self::data) holds the whole chunk.
    pub fn is_complete(&self) -> bool {
        self.data.len() == self.location.length
    }

    /// Returns the common field for a RIFF `INFO` entry or an AIFF text chunk.
    pub fn field(&self) -> Option<Field> {
        match self.format {
            ChunkFormat::Riff => None,
            ChunkFormat::RiffInfo => Field::from_riff_info(&self.identifier),
            ChunkFormat::Aiff => Field::from_aiff(&self.identifier),
        }
    }

    /// Returns the chunk data as text, for `INFO` entries and AIFF text chunks.
    ///
    /// The text is read as UTF-8, which also covers ASCII, the usual content.
    pub fn text(&self) -> Result<Text<'a>> {
        if !self.is_complete() {
            return Err(Error::Needs(self.location));
        }

        Ok(Text::terminated(Encoding::Utf8, self.data))
    }

    /// Returns a cursor over the entries of a RIFF `LIST` `INFO` chunk, or `None` for any other chunk.
    pub fn info(&self) -> Result<Option<Chunks>> {
        if self.format != ChunkFormat::Riff || &self.identifier != b"LIST" {
            return Ok(None);
        }

        if self.location.length < 4 {
            return Err(Error::Invalid);
        }

        let kind = self
            .data
            .get(..4)
            .ok_or(Error::Needs(Location::new(self.location.offset, 4)))?;

        if kind != b"INFO" {
            return Ok(None);
        }

        Ok(Some(Chunks::new(
            self.location.offset + 4,
            self.location.end(),
            ChunkFormat::RiffInfo,
        )))
    }

    /// Returns `true` for an `id3 ` or `ID3 ` chunk, which holds an ID3v2 tag.
    pub fn is_id3v2(&self) -> bool {
        matches!(&self.identifier, b"id3 " | b"ID3 ")
    }

    /// Parses the ID3v2 tag header of an `id3 ` or `ID3 ` chunk, or returns `None` for any other chunk.
    pub fn id3v2(&self) -> Result<Option<Id3v2Tag>> {
        if !self.is_id3v2() {
            return Ok(None);
        }

        let tag = Id3v2Tag::parse(self.data, self.location.offset)?;

        if tag.size() > self.location.length {
            return Err(Error::Invalid);
        }

        Ok(Some(tag))
    }

    /// Returns the frame count from a RIFF `fact` chunk, or `None` for any other chunk.
    pub fn sample_length(&self) -> Result<Option<u32>> {
        if self.format != ChunkFormat::Riff || &self.identifier != b"fact" {
            return Ok(None);
        }

        if self.location.length < 4 {
            return Err(Error::Invalid);
        }

        let bytes = self
            .data
            .get(..4)
            .ok_or(Error::Needs(Location::new(self.location.offset, 4)))?;

        Ok(Some(read_u32_le(bytes)))
    }
}
