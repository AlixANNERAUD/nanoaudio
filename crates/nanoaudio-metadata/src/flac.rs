use super::{
    Cursor, Dimensions, Encoding, Error, Field, Location, Picture, Result, Text, Window, grow,
    read_u24_be, read_u32_be, read_u32_le,
};

const MARKER_LENGTH: usize = 4;
const BLOCK_HEADER_LENGTH: usize = 4;
const STREAM_INFO_LENGTH: usize = 34;
const SEEK_POINT_LENGTH: usize = 18;

/// The `fLaC` marker that starts a FLAC stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Flac {
    /// Where the marker is in the file.
    pub location: Location,
}

impl Flac {
    /// Checks for the `fLaC` marker at file offset `offset`, which is also where `bytes` starts.
    pub fn parse(bytes: &[u8], offset: usize) -> Result<Self> {
        let marker = Window::new(bytes, offset).array::<MARKER_LENGTH>(offset)?;

        if &marker != b"fLaC" {
            return Err(Error::Invalid);
        }

        Ok(Self {
            location: Location::new(offset, MARKER_LENGTH),
        })
    }

    /// Returns a cursor over the metadata blocks that follow the marker.
    pub fn blocks(&self) -> FlacBlocks {
        FlacBlocks {
            position: self.location.end(),
            finished: false,
        }
    }
}

/// A [`Cursor`] over the metadata blocks of a FLAC stream, returned by [`Flac::blocks`].
///
/// It stops after the block marked as last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlacBlocks {
    position: usize,
    finished: bool,
}

impl FlacBlocks {
    /// Returns the file offset of the next block header, which is where the audio frames start once the cursor is done.
    pub const fn position(&self) -> usize {
        self.position
    }
}

impl Cursor for FlacBlocks {
    type Item<'a> = Block<'a>;

    fn advance<'a>(&mut self, bytes: &'a [u8], offset: usize) -> Result<Option<Block<'a>>> {
        if self.finished {
            return Ok(None);
        }

        let window = Window::new(bytes, offset);
        let header = window.array::<BLOCK_HEADER_LENGTH>(self.position)?;
        let block_type = BlockType::from(header[0] & 0x7F);

        if block_type == BlockType::Invalid {
            return Err(Error::Invalid);
        }

        let length = read_u24_be(&header[1..4]) as usize;
        let location = Location::new(self.position.saturating_add(BLOCK_HEADER_LENGTH), length);
        let last = header[0] & 0x80 != 0;

        self.position = location.end();
        self.finished = last;

        Ok(Some(Block {
            block_type,
            last,
            location,
            data: window.available(location),
        }))
    }
}

/// The type of a FLAC metadata block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    /// `STREAMINFO`, the stream parameters.
    StreamInfo,
    /// `PADDING`.
    Padding,
    /// `APPLICATION`, data for a third-party application.
    Application,
    /// `SEEKTABLE`.
    SeekTable,
    /// `VORBIS_COMMENT`, the tags.
    VorbisComment,
    /// `CUESHEET`.
    CueSheet,
    /// `PICTURE`.
    Picture,
    /// A reserved block type, with its number.
    Reserved(u8),
    /// The invalid block type 127.
    Invalid,
}

impl From<u8> for BlockType {
    fn from(value: u8) -> Self {
        match value {
            0 => BlockType::StreamInfo,
            1 => BlockType::Padding,
            2 => BlockType::Application,
            3 => BlockType::SeekTable,
            4 => BlockType::VorbisComment,
            5 => BlockType::CueSheet,
            6 => BlockType::Picture,
            127 => BlockType::Invalid,
            value => BlockType::Reserved(value),
        }
    }
}

/// A FLAC metadata block, with as much of its data as the buffer given to the cursor held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Block<'a> {
    /// Block type.
    pub block_type: BlockType,
    /// `true` for the last metadata block before the audio frames.
    pub last: bool,
    /// Where the block data is in the file, header excluded.
    pub location: Location,
    /// The part of the block data found in the buffer, which can be empty or partial.
    pub data: &'a [u8],
}

/// The content of a `STREAMINFO` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamInfo {
    /// Smallest block size in the stream, in samples per channel.
    pub minimum_block_size: u16,
    /// Largest block size in the stream, in samples per channel.
    pub maximum_block_size: u16,
    /// Smallest frame size in bytes, 0 when unknown.
    pub minimum_frame_size: u32,
    /// Largest frame size in bytes, 0 when unknown.
    pub maximum_frame_size: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of channels, from 1 to 8.
    pub channels: u8,
    /// Bits per sample, from 4 to 32.
    pub bits_per_sample: u8,
    /// Total samples per channel, 0 when unknown.
    pub total_samples: u64,
    /// MD5 signature of the decoded audio, all zeros when unknown.
    pub md5: [u8; 16],
}

/// The parsed content of a FLAC metadata block, returned by [`Block::content`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BlockContent<'a> {
    /// Stream parameters.
    StreamInfo(StreamInfo),
    /// Padding.
    Padding,
    /// Application data.
    Application {
        /// Registered application identifier.
        identifier: [u8; 4],
        /// Where the application data is in the file.
        data: Location,
    },
    /// A seek table, read with [`Block::seek_point`].
    SeekTable {
        /// Number of seek points.
        points: usize,
    },
    /// Tags, walked with the [`VorbisComments`] cursor.
    VorbisComment(VorbisComments),
    /// A cue sheet, not parsed.
    CueSheet,
    /// An attached picture.
    Picture(Picture<'a>),
    /// A reserved block type.
    Reserved,
}

impl<'a> Block<'a> {
    /// Returns the same block with its data taken from `bytes`, a part of the file that starts at file offset `offset`.
    ///
    /// Use it after reading [`location`](Self::location) into a buffer of its own.
    pub fn within<'b>(&self, bytes: &'b [u8], offset: usize) -> Block<'b> {
        Block {
            data: Window::new(bytes, offset).available(self.location),
            ..*self
        }
    }
    /// Returns `true` when [`data`](Self::data) holds the whole block.
    pub fn is_complete(&self) -> bool {
        self.data.len() == self.location.length
    }

    /// Parses the block content from [`data`](Self::data), reading only what each type needs.
    ///
    /// Returns [`Error::Needs`] when the part needed is not in `data`.
    pub fn content(&self) -> Result<BlockContent<'a>> {
        match self.block_type {
            BlockType::StreamInfo => self.stream_info().map(BlockContent::StreamInfo),
            BlockType::Padding => Ok(BlockContent::Padding),
            BlockType::Application => {
                let identifier =
                    self.data
                        .first_chunk::<4>()
                        .ok_or(if self.location.length < 4 {
                            Error::Invalid
                        } else {
                            Error::Needs(Location::new(self.location.offset, 4))
                        })?;

                Ok(BlockContent::Application {
                    identifier: *identifier,
                    data: Location::new(self.location.offset + 4, self.location.length - 4),
                })
            }
            BlockType::SeekTable => Ok(BlockContent::SeekTable {
                points: self.location.length / SEEK_POINT_LENGTH,
            }),
            BlockType::VorbisComment => Ok(BlockContent::VorbisComment(VorbisComments::new(
                self.location,
            ))),
            BlockType::CueSheet => Ok(BlockContent::CueSheet),
            BlockType::Picture => self.picture().map(BlockContent::Picture),
            BlockType::Reserved(_) | BlockType::Invalid => Ok(BlockContent::Reserved),
        }
    }

    /// Parses a `STREAMINFO` block.
    pub fn stream_info(&self) -> Result<StreamInfo> {
        if self.block_type != BlockType::StreamInfo || self.location.length < STREAM_INFO_LENGTH {
            return Err(Error::Invalid);
        }

        let bytes = self
            .data
            .get(..STREAM_INFO_LENGTH)
            .ok_or(Error::Needs(Location::new(
                self.location.offset,
                STREAM_INFO_LENGTH,
            )))?;
        let packed = u64::from_be_bytes([
            bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17],
        ]);
        let mut md5 = [0; 16];
        md5.copy_from_slice(&bytes[18..34]);

        Ok(StreamInfo {
            minimum_block_size: u16::from_be_bytes([bytes[0], bytes[1]]),
            maximum_block_size: u16::from_be_bytes([bytes[2], bytes[3]]),
            minimum_frame_size: read_u24_be(&bytes[4..7]),
            maximum_frame_size: read_u24_be(&bytes[7..10]),
            sample_rate: (packed >> 44) as u32,
            channels: ((packed >> 41) & 0x07) as u8 + 1,
            bits_per_sample: ((packed >> 36) & 0x1F) as u8 + 1,
            total_samples: packed & 0x000F_FFFF_FFFF,
            md5,
        })
    }

    /// Parses a `PICTURE` block. Only the picture header needs to be in [`data`](Self::data), not the image.
    pub fn picture(&self) -> Result<Picture<'a>> {
        if self.block_type != BlockType::Picture {
            return Err(Error::Invalid);
        }

        let mut reader = Reader {
            data: self.data,
            position: 0,
        };
        let parsed = (|| {
            let picture_type = reader.u32()?;
            let mime_length = reader.u32()? as usize;
            let mime_type = reader.bytes(mime_length)?;
            let description_length = reader.u32()? as usize;
            let description = reader.bytes(description_length)?;
            let width = reader.u32()?;
            let height = reader.u32()?;
            let depth = reader.u32()?;
            let colors = reader.u32()?;
            let data_length = reader.u32()? as usize;

            Some((
                picture_type,
                mime_type,
                description,
                Dimensions {
                    width,
                    height,
                    depth,
                    colors,
                },
                data_length,
            ))
        })();

        let Some((picture_type, mime_type, description, dimensions, data_length)) = parsed else {
            return Err(if self.is_complete() {
                Error::Invalid
            } else {
                grow(self.location, self.data.len())
            });
        };

        let header = reader.position;

        if data_length > self.location.length - header {
            return Err(Error::Invalid);
        }

        Ok(Picture {
            picture_type,
            mime_type,
            description: Text::new(Encoding::Utf8, description),
            dimensions: Some(dimensions),
            data: Location::new(self.location.offset + header, data_length),
        })
    }

    /// Returns the seek point at `index` in a `SEEKTABLE` block, or [`Error::Invalid`] when `index` is past the end of the table.
    pub fn seek_point(&self, index: usize) -> Result<SeekPoint> {
        if self.block_type != BlockType::SeekTable {
            return Err(Error::Invalid);
        }

        let start = index
            .checked_mul(SEEK_POINT_LENGTH)
            .filter(|start| {
                start
                    .checked_add(SEEK_POINT_LENGTH)
                    .is_some_and(|end| end <= self.location.length)
            })
            .ok_or(Error::Invalid)?;
        let location = Location::new(self.location.offset + start, SEEK_POINT_LENGTH);
        let bytes = self
            .data
            .get(start..start + SEEK_POINT_LENGTH)
            .ok_or(Error::Needs(location))?;
        let read_u64 = |bytes: &[u8]| {
            u64::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])
        };

        Ok(SeekPoint {
            sample: read_u64(&bytes[0..8]),
            offset: read_u64(&bytes[8..16]),
            samples: u16::from_be_bytes([bytes[16], bytes[17]]),
        })
    }
}

/// A point of a FLAC seek table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SeekPoint {
    /// Number of the first sample in the target frame, `u64::MAX` for a placeholder.
    pub sample: u64,
    /// Offset in bytes of the target frame, from the first audio frame.
    pub offset: u64,
    /// Number of samples in the target frame.
    pub samples: u16,
}

impl SeekPoint {
    /// Returns `true` for a placeholder point, which holds no position.
    pub const fn is_placeholder(&self) -> bool {
        self.sample == u64::MAX
    }
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, length: usize) -> Option<&'a [u8]> {
        let bytes = self
            .data
            .get(self.position..self.position.checked_add(length)?)?;
        self.position += length;

        Some(bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        self.bytes(4).map(read_u32_be)
    }
}

/// A [`Cursor`] over the comments of a `VORBIS_COMMENT` block, each a `KEY=value` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VorbisComments {
    location: Location,
    position: usize,
    remaining: Option<u32>,
}

impl VorbisComments {
    /// Creates a cursor over the Vorbis comment block found at `location`.
    pub const fn new(location: Location) -> Self {
        Self {
            location,
            position: location.offset,
            remaining: None,
        }
    }

    /// Returns where the block data is in the file.
    pub const fn location(&self) -> Location {
        self.location
    }

    /// Returns the vendor string, read from `bytes`, a part of the file that starts at file offset `offset`.
    pub fn vendor<'a>(&self, bytes: &'a [u8], offset: usize) -> Result<Text<'a>> {
        let window = Window::new(bytes, offset);
        let start = self.location.offset;
        let length = read_u32_le(&window.array::<4>(start)?) as usize;

        if length > self.location.length.saturating_sub(4) {
            return Err(Error::Invalid);
        }

        let vendor = window.get(Location::new(start + 4, length))?;

        Ok(Text::new(Encoding::Utf8, vendor))
    }

    fn length(&self, window: Window<'_>) -> Result<usize> {
        let length = read_u32_le(&window.array::<4>(self.position)?) as usize;

        if length > self.location.end().saturating_sub(self.position + 4) {
            return Err(Error::Invalid);
        }

        Ok(length)
    }
}

impl Cursor for VorbisComments {
    type Item<'a> = VorbisComment<'a>;

    fn advance<'a>(&mut self, bytes: &'a [u8], offset: usize) -> Result<Option<VorbisComment<'a>>> {
        let window = Window::new(bytes, offset);

        let remaining = match self.remaining {
            Some(remaining) => remaining,
            None => {
                let vendor = self.length(window)?;
                let position = self.position + 4 + vendor;

                if position + 4 > self.location.end() {
                    return Err(Error::Invalid);
                }

                let count = read_u32_le(&window.array::<4>(position)?);

                self.position = position + 4;
                self.remaining = Some(count);

                count
            }
        };

        if remaining == 0 {
            return Ok(None);
        }

        let length = self.length(window)?;
        let location = Location::new(self.position + 4, length);

        self.position = location.end();
        self.remaining = Some(remaining - 1);

        Ok(Some(VorbisComment {
            location,
            data: window.available(location),
        }))
    }
}

/// A Vorbis comment, with as much of its data as the buffer given to the cursor held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VorbisComment<'a> {
    /// Where the comment is in the file, length prefix excluded.
    pub location: Location,
    /// The part of the comment found in the buffer, which can be empty or partial.
    pub data: &'a [u8],
}

impl<'a> VorbisComment<'a> {
    /// Returns the same comment with its data taken from `bytes`, a part of the file that starts at file offset `offset`.
    pub fn within<'b>(&self, bytes: &'b [u8], offset: usize) -> VorbisComment<'b> {
        VorbisComment {
            data: Window::new(bytes, offset).available(self.location),
            ..*self
        }
    }
    /// Returns `true` when [`data`](Self::data) holds the whole comment.
    pub fn is_complete(&self) -> bool {
        self.data.len() == self.location.length
    }

    /// Returns the key, the part before `=`, such as `TITLE`.
    pub fn key(&self) -> Result<&'a [u8]> {
        match self.data.iter().position(|byte| *byte == b'=') {
            Some(index) => Ok(&self.data[..index]),
            None if self.is_complete() => Err(Error::Invalid),
            None => Err(grow(self.location, self.data.len())),
        }
    }

    /// Returns the value, the UTF-8 text after `=`.
    pub fn value(&self) -> Result<Text<'a>> {
        let key = self.key()?;

        if !self.is_complete() {
            return Err(Error::Needs(self.location));
        }

        Ok(Text::new(Encoding::Utf8, &self.data[key.len() + 1..]))
    }

    /// Returns where the value is in the file, to read a long value without the key.
    pub fn value_location(&self) -> Result<Location> {
        let key = self.key()?;
        let start = key.len() + 1;

        Ok(Location::new(
            self.location.offset + start,
            self.location.length - start,
        ))
    }

    /// Returns the common field for the key, if it maps to one.
    pub fn field(&self) -> Option<Field> {
        Field::from_vorbis(self.key().ok()?)
    }
}
