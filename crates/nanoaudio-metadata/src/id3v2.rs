use super::{
    Cursor, Encoding, Error, Field, Location, Picture, Result, Text, Window, grow, read_u24_be,
    read_u32_be,
};

const HEADER_LENGTH: usize = 10;
const FOOTER_LENGTH: usize = 10;

/// The flags of an ID3v2 tag header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Id3v2Flags {
    /// Unsynchronisation is applied to the whole tag (ID3v2.2 and ID3v2.3) or to every frame (ID3v2.4).
    pub unsynchronisation: bool,
    /// An extended header follows the tag header; the frame cursor skips it.
    pub extended_header: bool,
    /// The tag is marked as experimental.
    pub experimental: bool,
    /// A 10-byte footer ends the tag (ID3v2.4).
    pub footer: bool,
    /// The whole tag is compressed (ID3v2.2), which is not supported.
    pub compression: bool,
}

/// The header of an ID3v2.2, ID3v2.3 or ID3v2.4 tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id3v2Tag {
    /// Major version: 2, 3 or 4.
    pub major_version: u8,
    /// Revision number.
    pub revision: u8,
    /// Tag flags.
    pub flags: Id3v2Flags,
    /// The whole tag in the file, header and footer included.
    pub location: Location,
}

impl Id3v2Tag {
    /// Length of the tag header in bytes.
    pub const HEADER_LENGTH: usize = HEADER_LENGTH;
    /// Length of the ID3v2.4 footer in bytes.
    pub const FOOTER_LENGTH: usize = FOOTER_LENGTH;

    /// Parses the 10-byte tag header at file offset `offset`, which is also where `bytes` starts.
    pub fn parse(bytes: &[u8], offset: usize) -> Result<Self> {
        let header = Window::new(bytes, offset).array::<HEADER_LENGTH>(offset)?;

        if &header[..3] != b"ID3" {
            return Err(Error::Invalid);
        }

        Self::from_header(&header, offset)
    }

    /// Parses an ID3v2.4 footer at file offset `offset`, to find a tag appended at the end of a file.
    ///
    /// `bytes` starts at `offset`, and the returned [`location`](Self::location) points to the start of the tag.
    pub fn parse_footer(bytes: &[u8], offset: usize) -> Result<Self> {
        let footer = Window::new(bytes, offset).array::<FOOTER_LENGTH>(offset)?;

        if &footer[..3] != b"3DI" || footer[3] != 4 {
            return Err(Error::Invalid);
        }

        let size = syncsafe(&footer[6..10]).ok_or(Error::Invalid)? as usize;
        let start = (offset + FOOTER_LENGTH)
            .checked_sub(HEADER_LENGTH + size + FOOTER_LENGTH)
            .ok_or(Error::Invalid)?;

        Self::from_header(&footer, start)
    }

    fn from_header(header: &[u8; HEADER_LENGTH], offset: usize) -> Result<Self> {
        let major_version = header[3];
        let revision = header[4];
        let flags = header[5];

        if !(2..=4).contains(&major_version) || revision == 0xFF {
            return Err(Error::Unsupported);
        }

        let size = syncsafe(&header[6..10]).ok_or(Error::Invalid)? as usize;
        let flags = Id3v2Flags {
            unsynchronisation: flags & 0x80 != 0,
            extended_header: major_version > 2 && flags & 0x40 != 0,
            compression: major_version == 2 && flags & 0x40 != 0,
            experimental: major_version > 2 && flags & 0x20 != 0,
            footer: major_version == 4 && flags & 0x10 != 0,
        };
        let footer = if flags.footer { FOOTER_LENGTH } else { 0 };

        Ok(Self {
            major_version,
            revision,
            flags,
            location: Location::new(offset, HEADER_LENGTH + size + footer),
        })
    }

    /// Returns the length of the whole tag in bytes, header and footer included.
    pub const fn size(&self) -> usize {
        self.location.length
    }

    /// Returns the file offset of the first byte after the tag, where the audio usually starts.
    pub const fn end(&self) -> usize {
        self.location.end()
    }

    /// Returns where the frames are in the file, between the header and the footer, extended header and padding included.
    pub fn frames_location(&self) -> Location {
        let footer = if self.flags.footer { FOOTER_LENGTH } else { 0 };

        Location::new(
            self.location.offset.saturating_add(HEADER_LENGTH),
            self.location
                .length
                .saturating_sub(HEADER_LENGTH)
                .saturating_sub(footer),
        )
    }

    /// Returns a cursor over the frames of the tag.
    pub fn frames(&self) -> Id3v2Frames {
        let frames = self.frames_location();

        Id3v2Frames {
            tag: *self,
            position: frames.offset,
            end: frames.end(),
            started: false,
        }
    }
}

fn syncsafe(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0u32, |value, byte| {
        (byte & 0x80 == 0).then_some((value << 7) | *byte as u32)
    })
}

/// A [`Cursor`] over the frames of an ID3v2 tag, returned by [`Id3v2Tag::frames`].
///
/// It skips the extended header and padding, and with whole-tag unsynchronisation (ID3v2.2 and ID3v2.3) it reads frame headers through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id3v2Frames {
    tag: Id3v2Tag,
    position: usize,
    end: usize,
    started: bool,
}

impl Id3v2Frames {
    /// Returns the file offset of the next frame header.
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Returns the tag being walked.
    pub const fn tag(&self) -> &Id3v2Tag {
        &self.tag
    }

    fn whole_tag_unsynchronised(&self) -> bool {
        self.tag.flags.unsynchronisation && self.tag.major_version < 4
    }

    fn skip_extended_header(&mut self, window: Window<'_>) -> Result<()> {
        if !self.tag.flags.extended_header {
            return Ok(());
        }

        let size = window.array::<4>(self.position)?;
        let length = match self.tag.major_version {
            3 => (read_u32_be(&size) as usize).saturating_add(4),
            _ => syncsafe(&size).ok_or(Error::Invalid)? as usize,
        };

        if length < 4 || length > self.end.saturating_sub(self.position) {
            return Err(Error::Invalid);
        }

        self.position += length;

        Ok(())
    }

    fn header(&self, window: Window<'_>, length: usize) -> Result<([u8; HEADER_LENGTH], usize)> {
        let available = self.end.saturating_sub(self.position);
        let mut header = [0; HEADER_LENGTH];

        if !self.whole_tag_unsynchronised() {
            let bytes = window.get(Location::new(self.position, length))?;
            header[..length].copy_from_slice(bytes);

            return Ok((header, length));
        }

        let raw = window.available(Location::new(self.position, available));
        let mut previous = 0;
        let mut written = 0;
        let mut consumed = 0;

        while written < length {
            let Some(byte) = raw.get(consumed) else {
                if raw.len() == available {
                    return Err(Error::Invalid);
                }

                let needed = (2 * length + 1).min(available);

                return Err(Error::Needs(Location::new(self.position, needed)));
            };

            consumed += 1;

            if !(previous == 0xFF && *byte == 0) {
                header[written] = *byte;
                written += 1;
            }

            previous = *byte;
        }

        if previous == 0xFF && raw.get(consumed) == Some(&0) {
            consumed += 1;
        }

        Ok((header, consumed))
    }

    fn raw_length(&self, window: Window<'_>, start: usize, size: usize) -> Result<usize> {
        let available = self.end - start;

        if !self.whole_tag_unsynchronised() {
            return if size > available {
                Err(Error::Invalid)
            } else {
                Ok(size)
            };
        }

        let raw = window.available(Location::new(start, available));
        let mut previous = 0;
        let mut written = 0;
        let mut consumed = 0;

        while written < size {
            let Some(byte) = raw.get(consumed) else {
                return if raw.len() == available {
                    Err(Error::Invalid)
                } else {
                    Err(Error::Needs(Location::new(
                        self.position,
                        self.end - self.position,
                    )))
                };
            };

            consumed += 1;

            if !(previous == 0xFF && *byte == 0) {
                written += 1;
            }

            previous = *byte;
        }

        if previous == 0xFF && raw.get(consumed) == Some(&0) {
            consumed += 1;
        }

        Ok(consumed)
    }
}

impl Cursor for Id3v2Frames {
    type Item<'a> = Frame<'a>;

    fn advance<'a>(&mut self, bytes: &'a [u8], offset: usize) -> Result<Option<Frame<'a>>> {
        let window = Window::new(bytes, offset);

        if self.tag.flags.compression {
            return Err(Error::Unsupported);
        }

        if !self.started {
            self.skip_extended_header(window)?;
            self.started = true;
        }

        let version = self.tag.major_version;
        let header_length = if version == 2 { 6 } else { 10 };

        loop {
            if self.end.saturating_sub(self.position) < header_length {
                return Ok(None);
            }

            let (header, consumed) = self.header(window, header_length)?;

            if header[0] == 0 {
                return Ok(None);
            }

            let identifier_length = if version == 2 { 3 } else { 4 };
            let identifier = &header[..identifier_length];

            if !identifier
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            {
                return Err(Error::Invalid);
            }

            let (size, flags) = match version {
                2 => (read_u24_be(&header[3..6]) as usize, [0, 0]),
                3 => (read_u32_be(&header[4..8]) as usize, [header[8], header[9]]),
                _ => (
                    syncsafe(&header[4..8]).ok_or(Error::Invalid)? as usize,
                    [header[8], header[9]],
                ),
            };

            let start = self.position + consumed;
            let length = self.raw_length(window, start, size)?;
            let location = Location::new(start, length);
            self.position = start + length;

            if size == 0 {
                continue;
            }

            let mut array = [0; 4];
            array[..identifier_length].copy_from_slice(identifier);

            return Ok(Some(Frame {
                identifier: array,
                identifier_length: identifier_length as u8,
                major_version: version,
                flags: FrameFlags::new(version, flags, self.tag.flags.unsynchronisation),
                location,
                data: window.available(location),
            }));
        }
    }
}

/// The flags of an ID3v2 frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FrameFlags {
    /// The frame data is compressed with zlib, which is not supported.
    pub compressed: bool,
    /// The frame data is encrypted, which is not supported.
    pub encrypted: bool,
    /// The frame belongs to a group, and a group identifier byte precedes the data.
    pub grouped: bool,
    /// Unsynchronisation is applied to the frame, by the frame flag or the tag flag; read the frame with [`Frame::decode`].
    pub unsynchronised: bool,
    /// A 4-byte data length precedes the data (ID3v2.4).
    pub data_length_indicator: bool,
    /// The two flag bytes as stored, zero in ID3v2.2.
    pub raw: [u8; 2],
}

impl FrameFlags {
    fn new(version: u8, raw: [u8; 2], tag_unsynchronised: bool) -> Self {
        let format = raw[1];

        match version {
            3 => Self {
                compressed: format & 0x80 != 0,
                encrypted: format & 0x40 != 0,
                grouped: format & 0x20 != 0,
                unsynchronised: tag_unsynchronised,
                data_length_indicator: false,
                raw,
            },
            4 => Self {
                grouped: format & 0x40 != 0,
                compressed: format & 0x08 != 0,
                encrypted: format & 0x04 != 0,
                unsynchronised: tag_unsynchronised || format & 0x02 != 0,
                data_length_indicator: format & 0x01 != 0,
                raw,
            },
            _ => Self {
                unsynchronised: tag_unsynchronised,
                raw,
                ..Self::default()
            },
        }
    }
}

/// An ID3v2 frame, with as much of its data as the buffer given to the cursor held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Frame<'a> {
    identifier: [u8; 4],
    identifier_length: u8,
    major_version: u8,
    /// Frame flags.
    pub flags: FrameFlags,
    /// Where the frame data is in the file, frame header excluded, as stored (still unsynchronised if it is).
    pub location: Location,
    /// The part of the frame data found in the buffer, which can be empty or partial.
    pub data: &'a [u8],
}

/// The parsed content of an ID3v2 frame, returned by [`Frame::content`] and [`Frame::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Content<'a> {
    /// A text frame (`T***`, such as `TIT2`), possibly holding several values; see [`Text::values`].
    Text(Text<'a>),
    /// A user-defined text frame (`TXXX`).
    UserText {
        /// What the value is.
        description: Text<'a>,
        /// The value.
        value: Text<'a>,
    },
    /// A comment (`COMM`) or unsynchronised lyrics (`USLT`) frame.
    Comment {
        /// ISO 639-2 language code, such as `eng`.
        language: [u8; 3],
        /// Short description.
        description: Text<'a>,
        /// The comment or lyrics.
        text: Text<'a>,
    },
    /// A URL frame (`W***`, such as `WOAR`).
    Url(Text<'a>),
    /// A user-defined URL frame (`WXXX`).
    UserUrl {
        /// What the URL is.
        description: Text<'a>,
        /// The URL.
        url: Text<'a>,
    },
    /// An attached picture (`APIC`, or `PIC` in ID3v2.2).
    Picture(Picture<'a>),
    /// Any other frame, as raw data.
    Other(&'a [u8]),
    /// The frame is unsynchronised: decode it with [`Frame::decode`], then parse the result with [`Frame::parse`].
    Unsynchronised,
    /// The frame is compressed, which is not supported.
    Compressed,
    /// The frame is encrypted, which is not supported.
    Encrypted,
}

impl<'a> Frame<'a> {
    /// Returns the same frame with its data taken from `bytes`, a part of the file that starts at file offset `offset`.
    ///
    /// Use it after reading [`location`](Self::location) into a buffer of its own.
    pub fn within<'b>(&self, bytes: &'b [u8], offset: usize) -> Frame<'b> {
        Frame {
            data: Window::new(bytes, offset).available(self.location),
            ..*self
        }
    }
    /// Returns the frame identifier: three characters in ID3v2.2, four otherwise.
    pub fn identifier(&self) -> &[u8] {
        &self.identifier[..self.identifier_length as usize]
    }

    /// Returns the major version of the tag holding the frame.
    pub const fn major_version(&self) -> u8 {
        self.major_version
    }

    /// Returns the common field for this frame, if it maps to one.
    pub fn field(&self) -> Option<Field> {
        Field::from_id3v2(self.identifier())
    }

    /// Returns `true` when [`data`](Self::data) holds the whole frame.
    pub fn is_complete(&self) -> bool {
        self.data.len() == self.location.length
    }

    fn prefix_length(&self) -> usize {
        let flags = self.flags;

        match self.major_version {
            3 => 4 * flags.compressed as usize + flags.encrypted as usize + flags.grouped as usize,
            4 => {
                flags.grouped as usize
                    + flags.encrypted as usize
                    + 4 * flags.data_length_indicator as usize
            }
            _ => 0,
        }
    }

    /// Returns where the frame payload is in the file, after the group identifier, encryption method and data length when present.
    ///
    /// Returns [`Error::Unsupported`] for an ID3v2.2 or ID3v2.3 frame under whole-tag unsynchronisation, where file offsets do not match payload offsets.
    pub fn payload_location(&self) -> Result<Location> {
        let prefix = self.prefix_length();

        if self.flags.unsynchronised && self.major_version < 4 {
            return Err(Error::Unsupported);
        }

        if prefix > self.location.length {
            return Err(Error::Invalid);
        }

        Ok(Location::new(
            self.location.offset + prefix,
            self.location.length - prefix,
        ))
    }

    /// Parses the frame content from [`data`](Self::data).
    ///
    /// Unsynchronised, compressed and encrypted frames return the matching [`Content`] variant instead. A picture only needs its header to be in `data`, not the image.
    ///
    /// # Errors
    ///
    /// [`Error::Needs`] when `data` is partial, and [`Error::Invalid`] when the frame is malformed.
    pub fn content(&self) -> Result<Content<'a>> {
        if self.flags.encrypted {
            return Ok(Content::Encrypted);
        }

        if self.flags.compressed {
            return Ok(Content::Compressed);
        }

        if self.flags.unsynchronised {
            return Ok(Content::Unsynchronised);
        }

        let location = self.payload_location()?;
        let start = self.prefix_length().min(self.data.len());

        parse(
            self.identifier(),
            self.major_version,
            &self.data[start..],
            location,
        )
    }

    /// Copies the frame payload into `output`, removing unsynchronisation, and returns it.
    ///
    /// Pass the result to [`parse`](Self::parse). `output` needs at most the length of [`location`](Self::location).
    ///
    /// # Errors
    ///
    /// [`Error::Needs`] when [`data`](Self::data) is partial, and [`Error::BufferTooSmall`] when `output` is too short.
    pub fn decode<'b>(&self, output: &'b mut [u8]) -> Result<&'b [u8]> {
        if !self.is_complete() {
            return Err(Error::Needs(self.location));
        }

        let prefix = self.prefix_length();

        if self.major_version < 4 {
            let length = if self.flags.unsynchronised {
                decode_unsynchronisation(self.data, output)?
            } else {
                copy(self.data, output)?
            };

            return output.get(prefix..length).ok_or(Error::Invalid);
        }

        let payload = self.data.get(prefix..).ok_or(Error::Invalid)?;
        let length = if self.flags.unsynchronised {
            decode_unsynchronisation(payload, output)?
        } else {
            copy(payload, output)?
        };

        Ok(&output[..length])
    }

    /// Parses the content of this frame from `payload`, a payload returned by [`decode`](Self::decode).
    ///
    /// `offset` is the position given to `payload[0]` in the locations of the result, such as picture data; pass 0 to get positions in `payload`.
    pub fn parse<'b>(&self, payload: &'b [u8], offset: usize) -> Result<Content<'b>> {
        if self.flags.encrypted {
            return Ok(Content::Encrypted);
        }

        if self.flags.compressed {
            return Ok(Content::Compressed);
        }

        parse(
            self.identifier(),
            self.major_version,
            payload,
            Location::new(offset, payload.len()),
        )
    }
}

fn copy(input: &[u8], output: &mut [u8]) -> Result<usize> {
    output
        .get_mut(..input.len())
        .ok_or(Error::BufferTooSmall)?
        .copy_from_slice(input);

    Ok(input.len())
}

/// Removes ID3v2 unsynchronisation from `input`, dropping each zero byte that follows `0xFF`, and returns the number of bytes written to `output`.
pub fn decode_unsynchronisation(input: &[u8], output: &mut [u8]) -> Result<usize> {
    let mut previous = 0;
    let mut length = 0;

    for byte in input {
        if !(previous == 0xFF && *byte == 0) {
            *output.get_mut(length).ok_or(Error::BufferTooSmall)? = *byte;
            length += 1;
        }

        previous = *byte;
    }

    Ok(length)
}

fn encoding(byte: Option<&u8>) -> Result<Encoding> {
    byte.copied()
        .and_then(Encoding::from_id3)
        .ok_or(Error::Invalid)
}

fn parse<'a>(
    identifier: &[u8],
    version: u8,
    data: &'a [u8],
    location: Location,
) -> Result<Content<'a>> {
    let complete = data.len() >= location.length;
    let data = &data[..data.len().min(location.length)];

    if matches!(identifier, b"APIC" | b"PIC") {
        return match picture(version, data, location) {
            Some(picture) => picture.map(Content::Picture),
            None if complete => Err(Error::Invalid),
            None => Err(grow(location, data.len())),
        };
    }

    if !complete {
        return Err(Error::Needs(location));
    }

    let text = |encoding: Encoding, bytes: &'a [u8]| Text::terminated(encoding, bytes);

    match identifier {
        b"TXXX" | b"TXX" | b"WXXX" | b"WXX" => {
            let encoding = encoding(data.first())?;
            let (description, value) = encoding
                .split_terminated(&data[1..])
                .unwrap_or((&data[1..], &[]));
            let description = Text::new(encoding, description);

            Ok(if identifier[0] == b'T' {
                Content::UserText {
                    description,
                    value: text(encoding, value),
                }
            } else {
                Content::UserUrl {
                    description,
                    url: text(Encoding::Latin1, value),
                }
            })
        }
        b"COMM" | b"COM" | b"USLT" | b"ULT" => {
            let encoding = encoding(data.first())?;
            let language = data.get(1..4).ok_or(Error::Invalid)?;
            let (description, value) = encoding
                .split_terminated(&data[4..])
                .unwrap_or((&data[4..], &[]));

            Ok(Content::Comment {
                language: [language[0], language[1], language[2]],
                description: Text::new(encoding, description),
                text: text(encoding, value),
            })
        }
        [b'T', ..] => {
            let encoding = encoding(data.first())?;

            Ok(Content::Text(text(encoding, &data[1..])))
        }
        [b'W', ..] => Ok(Content::Url(text(Encoding::Latin1, data))),
        _ => Ok(Content::Other(data)),
    }
}

fn picture<'a>(version: u8, data: &'a [u8], location: Location) -> Option<Result<Picture<'a>>> {
    let encoding = match data.first().copied().map(Encoding::from_id3)? {
        Some(encoding) => encoding,
        None => return Some(Err(Error::Invalid)),
    };
    let rest = &data[1..];
    let (mime_type, rest) = if version == 2 {
        (rest.get(..3)?, &rest[3..])
    } else {
        Encoding::Latin1.split_terminated(rest)?
    };
    let (picture_type, rest) = rest.split_first()?;
    let (description, rest) = encoding.split_terminated(rest)?;
    let header = data.len() - rest.len();

    Some(Ok(Picture {
        picture_type: *picture_type as u32,
        mime_type,
        description: Text::new(encoding, description),
        dimensions: None,
        data: Location::new(location.offset + header, location.length - header),
    }))
}
