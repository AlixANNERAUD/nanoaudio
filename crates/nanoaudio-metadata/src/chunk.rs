use super::{
    Cursor, Encoding, Error, Field, Id3v2Tag, Location, Result, Text, Window, read_u32_be,
    read_u32_le,
};

const FORM_HEADER_LENGTH: usize = 12;
const CHUNK_HEADER_LENGTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChunkFormat {
    Riff,
    RiffInfo,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Riff {
    pub location: Location,
}

impl Riff {
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

    pub fn chunks(&self) -> Chunks {
        Chunks::new(
            self.location.offset + FORM_HEADER_LENGTH,
            self.location.end(),
            ChunkFormat::Riff,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Aiff {
    pub location: Location,
    pub compressed: bool,
}

impl Aiff {
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

    pub fn chunks(&self) -> Chunks {
        Chunks::new(
            self.location.offset + FORM_HEADER_LENGTH,
            self.location.end(),
            ChunkFormat::Aiff,
        )
    }
}

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

    pub const fn position(&self) -> usize {
        self.position
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chunk<'a> {
    pub identifier: [u8; 4],
    pub format: ChunkFormat,
    pub location: Location,
    pub data: &'a [u8],
}

impl<'a> Chunk<'a> {
    pub fn within<'b>(&self, bytes: &'b [u8], offset: usize) -> Chunk<'b> {
        Chunk {
            data: Window::new(bytes, offset).available(self.location),
            ..*self
        }
    }
    pub fn is_complete(&self) -> bool {
        self.data.len() == self.location.length
    }

    pub fn field(&self) -> Option<Field> {
        match self.format {
            ChunkFormat::Riff => None,
            ChunkFormat::RiffInfo => Field::from_riff_info(&self.identifier),
            ChunkFormat::Aiff => Field::from_aiff(&self.identifier),
        }
    }

    pub fn text(&self) -> Result<Text<'a>> {
        if !self.is_complete() {
            return Err(Error::Needs(self.location));
        }

        Ok(Text::terminated(Encoding::Utf8, self.data))
    }

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

    pub fn is_id3v2(&self) -> bool {
        matches!(&self.identifier, b"id3 " | b"ID3 ")
    }

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
