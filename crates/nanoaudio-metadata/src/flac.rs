use super::{
    Cursor, Dimensions, Encoding, Error, Field, Location, Picture, Result, Text, Window, grow,
    read_u24_be, read_u32_be, read_u32_le,
};

const MARKER_LENGTH: usize = 4;
const BLOCK_HEADER_LENGTH: usize = 4;
const STREAM_INFO_LENGTH: usize = 34;
const SEEK_POINT_LENGTH: usize = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Flac {
    pub location: Location,
}

impl Flac {
    pub fn parse(bytes: &[u8], offset: usize) -> Result<Self> {
        let marker = Window::new(bytes, offset).array::<MARKER_LENGTH>(offset)?;

        if &marker != b"fLaC" {
            return Err(Error::Invalid);
        }

        Ok(Self {
            location: Location::new(offset, MARKER_LENGTH),
        })
    }

    pub fn blocks(&self) -> FlacBlocks {
        FlacBlocks {
            position: self.location.end(),
            finished: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlacBlocks {
    position: usize,
    finished: bool,
}

impl FlacBlocks {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    StreamInfo,
    Padding,
    Application,
    SeekTable,
    VorbisComment,
    CueSheet,
    Picture,
    Reserved(u8),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Block<'a> {
    pub block_type: BlockType,
    pub last: bool,
    pub location: Location,
    pub data: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamInfo {
    pub minimum_block_size: u16,
    pub maximum_block_size: u16,
    pub minimum_frame_size: u32,
    pub maximum_frame_size: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub total_samples: u64,
    pub md5: [u8; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockContent<'a> {
    StreamInfo(StreamInfo),
    Padding,
    Application { identifier: [u8; 4], data: Location },
    SeekTable { points: usize },
    VorbisComment(VorbisComments),
    CueSheet,
    Picture(Picture<'a>),
    Reserved,
}

impl<'a> Block<'a> {
    pub fn within<'b>(&self, bytes: &'b [u8], offset: usize) -> Block<'b> {
        Block {
            data: Window::new(bytes, offset).available(self.location),
            ..*self
        }
    }
    pub fn is_complete(&self) -> bool {
        self.data.len() == self.location.length
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SeekPoint {
    pub sample: u64,
    pub offset: u64,
    pub samples: u16,
}

impl SeekPoint {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VorbisComments {
    location: Location,
    position: usize,
    remaining: Option<u32>,
}

impl VorbisComments {
    pub const fn new(location: Location) -> Self {
        Self {
            location,
            position: location.offset,
            remaining: None,
        }
    }

    pub const fn location(&self) -> Location {
        self.location
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VorbisComment<'a> {
    pub location: Location,
    pub data: &'a [u8],
}

impl<'a> VorbisComment<'a> {
    pub fn within<'b>(&self, bytes: &'b [u8], offset: usize) -> VorbisComment<'b> {
        VorbisComment {
            data: Window::new(bytes, offset).available(self.location),
            ..*self
        }
    }
    pub fn is_complete(&self) -> bool {
        self.data.len() == self.location.length
    }

    pub fn key(&self) -> Result<&'a [u8]> {
        match self.data.iter().position(|byte| *byte == b'=') {
            Some(index) => Ok(&self.data[..index]),
            None if self.is_complete() => Err(Error::Invalid),
            None => Err(grow(self.location, self.data.len())),
        }
    }

    pub fn value(&self) -> Result<Text<'a>> {
        let key = self.key()?;

        if !self.is_complete() {
            return Err(Error::Needs(self.location));
        }

        Ok(Text::new(Encoding::Utf8, &self.data[key.len() + 1..]))
    }

    pub fn value_location(&self) -> Result<Location> {
        let key = self.key()?;
        let start = key.len() + 1;

        Ok(Location::new(
            self.location.offset + start,
            self.location.length - start,
        ))
    }

    pub fn field(&self) -> Option<Field> {
        Field::from_vorbis(self.key().ok()?)
    }
}
