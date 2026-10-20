#![no_std]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

use nanoaudio_core::{Error, Result};

/// Size of the fixed part of a page header, in bytes.
pub const OGG_HEADER_SIZE: usize = 27;
/// Largest page header with its segment table, in bytes.
pub const OGG_MAXIMUM_HEADER_SIZE: usize = OGG_HEADER_SIZE + 255;
/// Largest page, header included, in bytes.
pub const OGG_MAXIMUM_PAGE_SIZE: usize = OGG_MAXIMUM_HEADER_SIZE + 255 * 255;

const CAPTURE_PATTERN: &[u8; 4] = b"OggS";
const VERSION: u8 = 0;
const CONTINUED: u8 = 0x01;
const BEGINNING_OF_STREAM: u8 = 0x02;
const END_OF_STREAM: u8 = 0x04;
const NO_GRANULE_POSITION: u64 = u64::MAX;
const CRC_OFFSET: usize = 22;
const CRC_POLYNOMIAL: u32 = 0x04C1_1DB7;
const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;

    while index < 256 {
        let mut value = (index as u32) << 24;
        let mut bit = 0;

        while bit < 8 {
            value = if value & 0x8000_0000 != 0 {
                (value << 1) ^ CRC_POLYNOMIAL
            } else {
                value << 1
            };
            bit += 1;
        }

        table[index] = value;
        index += 1;
    }

    table
}

fn update_crc(crc: u32, bytes: &[u8]) -> u32 {
    bytes.iter().fold(crc, |crc, &byte| {
        (crc << 8) ^ CRC_TABLE[((crc >> 24) as u8 ^ byte) as usize]
    })
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from(read_u32(bytes)) | (u64::from(read_u32(&bytes[4..])) << 32)
}

/// A page whose checksum was verified, borrowed from the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OggPage<'a> {
    header_type: u8,
    granule_position: u64,
    serial_number: u32,
    sequence_number: u32,
    segments: &'a [u8],
    body: &'a [u8],
}

impl<'a> OggPage<'a> {
    /// Parses the page at the start of `bytes` and checks its CRC.
    ///
    /// # Errors
    ///
    /// [`Error::Incomplete`] until `bytes` holds the whole page, [`Error::InvalidData`] if it does not start with `OggS` or the CRC does not match, and [`Error::UnsupportedFormat`] for a page version other than 0.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let length = bytes.len().min(CAPTURE_PATTERN.len());

        if bytes[..length] != CAPTURE_PATTERN[..length] {
            return Err(Error::InvalidData);
        }

        let header = bytes.get(..OGG_HEADER_SIZE).ok_or(Error::Incomplete)?;

        if header[4] != VERSION {
            return Err(Error::UnsupportedFormat);
        }

        let header_size = OGG_HEADER_SIZE + header[26] as usize;
        let segments = bytes
            .get(OGG_HEADER_SIZE..header_size)
            .ok_or(Error::Incomplete)?;
        let body_size = segments
            .iter()
            .map(|&lacing| lacing as usize)
            .sum::<usize>();
        let page = bytes
            .get(..header_size + body_size)
            .ok_or(Error::Incomplete)?;

        let crc = update_crc(0, &page[..CRC_OFFSET]);
        let crc = update_crc(crc, &[0; 4]);
        let crc = update_crc(crc, &page[CRC_OFFSET + 4..]);

        if crc != read_u32(&header[CRC_OFFSET..]) {
            return Err(Error::InvalidData);
        }

        Ok(Self {
            header_type: header[5],
            granule_position: read_u64(&header[6..]),
            serial_number: read_u32(&header[14..]),
            sequence_number: read_u32(&header[18..]),
            segments,
            body: &page[header_size..],
        })
    }

    /// Returns the offset of the first `OggS` in `bytes`, or of the start of one cut by the end of `bytes`.
    ///
    /// `None` means no page starts in `bytes`, so all of it can be dropped.
    pub fn find(bytes: &[u8]) -> Option<usize> {
        let mut index = 0;

        while let Some(offset) = bytes[index..].iter().position(|&byte| byte == b'O') {
            let start = index + offset;
            let length = (bytes.len() - start).min(CAPTURE_PATTERN.len());

            if bytes[start..start + length] == CAPTURE_PATTERN[..length] {
                return Some(start);
            }

            index = start + 1;
        }

        None
    }

    /// Returns the granule position, or `None` when no packet ends on this page.
    pub const fn granule_position(&self) -> Option<u64> {
        match self.granule_position {
            NO_GRANULE_POSITION => None,
            position => Some(position),
        }
    }

    /// Returns the serial number of the logical stream the page belongs to.
    pub const fn serial_number(&self) -> u32 {
        self.serial_number
    }

    /// Returns the page sequence number.
    pub const fn sequence_number(&self) -> u32 {
        self.sequence_number
    }

    /// Returns the segment table: one lacing value per segment of the body.
    pub const fn segments(&self) -> &'a [u8] {
        self.segments
    }

    /// Returns the page body, the packet data after the header.
    pub const fn body(&self) -> &'a [u8] {
        self.body
    }

    /// Returns the size of the header and segment table, in bytes.
    pub const fn header_size(&self) -> usize {
        OGG_HEADER_SIZE + self.segments.len()
    }

    /// Returns the size of the whole page, in bytes.
    pub const fn size(&self) -> usize {
        self.header_size() + self.body.len()
    }

    /// Returns `true` if the first packet on the page started on the previous page.
    pub const fn is_continued(&self) -> bool {
        self.header_type & CONTINUED != 0
    }

    /// Returns `true` on the first page of a logical stream.
    pub const fn is_beginning_of_stream(&self) -> bool {
        self.header_type & BEGINNING_OF_STREAM != 0
    }

    /// Returns `true` on the last page of a logical stream.
    pub const fn is_end_of_stream(&self) -> bool {
        self.header_type & END_OF_STREAM != 0
    }

    /// Returns `true` if the last packet on the page continues on the next page.
    pub fn has_unfinished_packet(&self) -> bool {
        self.segments.last() == Some(&255)
    }

    /// Returns the packets on the page, in order.
    ///
    /// The first one is the end of a packet from the previous page when [`is_continued`](Self::is_continued) is `true`, and the last one is only the start of a packet when [`has_unfinished_packet`](Self::has_unfinished_packet) is `true`.
    pub const fn packets(&self) -> OggPagePackets<'a> {
        OggPagePackets {
            segments: self.segments,
            body: self.body,
        }
    }
}

/// An iterator over the packets of an [`OggPage`], from [`OggPage::packets`].
#[derive(Debug, Clone)]
pub struct OggPagePackets<'a> {
    segments: &'a [u8],
    body: &'a [u8],
}

impl<'a> Iterator for OggPagePackets<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.segments.is_empty() {
            return None;
        }

        let mut length = 0;
        let mut count = 0;

        for &lacing in self.segments {
            length += lacing as usize;
            count += 1;

            if lacing < 255 {
                break;
            }
        }

        let (packet, body) = self.body.split_at(length);

        self.segments = &self.segments[count..];
        self.body = body;

        Some(packet)
    }
}

/// A packet read by [`OggPacketReader`], whose bytes are at the start of the storage passed to [`read`](OggPacketReader::read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OggPacket {
    /// Length of the packet, in bytes.
    pub length: usize,
    /// Granule position of the page, on the last packet that ends on it.
    pub granule_position: Option<u64>,
    /// `true` on the last packet of the logical stream.
    pub end_of_stream: bool,
}

/// What [`OggPacketReader::read`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OggEvent<'a> {
    /// A page of the stream starts; its packets come next.
    Page {
        /// The whole page, still in the input.
        page: OggPage<'a>,
        /// `true` if data was lost or skipped since the previous page of the stream, or after [`resync`](OggPacketReader::resync). An unfinished packet is then dropped.
        lost: bool,
    },
    /// A packet is complete.
    Packet(OggPacket),
}

/// The result of [`OggPacketReader::read`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct OggRead<'a> {
    /// Input bytes consumed.
    pub consumed: usize,
    /// What was found, or `None` when the reader needs more input.
    pub event: Option<OggEvent<'a>>,
}

enum Run {
    Packet(OggPacket),
    Wait,
    Next,
}

/// A packet reader for one logical stream of an Ogg file that does not allocate.
///
/// It follows the first stream it sees and skips the pages of other streams. Packets are copied into the storage the caller passes, so they can span pages. Each page must be whole in the input before it is read, up to [`OGG_MAXIMUM_PAGE_SIZE`] bytes, because its CRC is checked first.
///
/// The input must start with a page. After that, junk and pages with a bad CRC are skipped, and the next page is reported as `lost`.
///
/// ```
/// use nanoaudio_core::Result;
/// use nanoaudio_ogg::{OggEvent, OggPacketReader};
///
/// fn count_packets(mut input: &[u8]) -> Result<usize> {
///     let mut reader = OggPacketReader::new();
///     let mut storage = [0; 8192];
///     let mut packets = 0;
///
///     loop {
///         let read = reader.read(input, &mut storage)?;
///         input = &input[read.consumed..];
///
///         match read.event {
///             Some(OggEvent::Packet(_)) => packets += 1,
///             Some(OggEvent::Page { .. }) => {}
///             None => return Ok(packets),
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct OggPacketReader {
    serial_number: Option<u32>,
    sequence_number: Option<u32>,
    segments: [u8; 255],
    segment_count: usize,
    segment_index: usize,
    granule_position: u64,
    end_of_stream: bool,
    length: usize,
    continuing: bool,
    discarding: bool,
    ended: bool,
    searching: bool,
    lost: bool,
    error: Option<Error>,
}

impl OggPacketReader {
    /// Creates a reader waiting for the first page.
    pub const fn new() -> Self {
        Self {
            serial_number: None,
            sequence_number: None,
            segments: [0; 255],
            segment_count: 0,
            segment_index: 0,
            granule_position: NO_GRANULE_POSITION,
            end_of_stream: false,
            length: 0,
            continuing: false,
            discarding: false,
            ended: false,
            searching: false,
            lost: false,
            error: None,
        }
    }

    /// Returns to the state of [`new`](Self::new), to read another file.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Prepares the reader for input from another offset of the same file, after a seek.
    ///
    /// It keeps the stream it follows, drops any unfinished packet, skips bytes up to the next page, and reports that page as `lost`.
    pub fn resync(&mut self) {
        self.drop_packet();
        self.sequence_number = None;
        self.segment_count = 0;
        self.segment_index = 0;
        self.end_of_stream = false;
        self.ended = false;
        self.searching = true;
        self.lost = true;
        self.error = None;
    }

    /// Returns the serial number of the stream being read, once its first page is read.
    pub const fn serial_number(&self) -> Option<u32> {
        self.serial_number
    }

    /// Returns `true` once the last packet of the stream is read.
    pub const fn is_ended(&self) -> bool {
        self.ended
    }

    /// Returns the length of the unfinished packet at the start of the storage, which continues on the next page.
    pub const fn pending(&self) -> usize {
        match self.continuing && !self.discarding {
            true => self.length,
            false => 0,
        }
    }

    /// Reads from `input` until the next page starts or a packet is complete, copying packet bytes into `storage`.
    ///
    /// Call it with the same storage until the stream ends. The input can be cut anywhere: the reader keeps its place.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidHeader`] if the input does not start with a page, [`Error::InvalidData`] or [`Error::UnsupportedFormat`] if that first page is corrupt or of another version, and [`Error::BufferTooSmall`] when a packet does not fit in `storage`. That packet is then skipped. An error found after some input was consumed is returned by the next call.
    pub fn read<'a>(&mut self, input: &'a [u8], storage: &mut [u8]) -> Result<OggRead<'a>> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }

        let mut consumed = 0;

        match self.next(input, storage, &mut consumed) {
            Ok(event) => Ok(OggRead { consumed, event }),
            Err(error) if consumed == 0 => Err(error),
            Err(error) => {
                self.error = Some(error);

                Ok(OggRead {
                    consumed,
                    event: None,
                })
            }
        }
    }

    fn next<'a>(
        &mut self,
        input: &'a [u8],
        storage: &mut [u8],
        consumed: &mut usize,
    ) -> Result<Option<OggEvent<'a>>> {
        loop {
            if self.segment_index < self.segment_count {
                match self.next_packet(input, storage, consumed)? {
                    Run::Packet(packet) => return Ok(Some(OggEvent::Packet(packet))),
                    Run::Wait => return Ok(None),
                    Run::Next => continue,
                }
            }

            if self.ended {
                return Ok(None);
            }

            let remaining = &input[*consumed..];

            if !self.searching {
                if remaining.len() >= CAPTURE_PATTERN.len()
                    && remaining[..CAPTURE_PATTERN.len()] != *CAPTURE_PATTERN
                {
                    return Err(Error::InvalidHeader);
                }

                match OggPage::parse(remaining) {
                    Ok(page) => {
                        if let Some(event) = self.begin(page, consumed) {
                            return Ok(Some(event));
                        }
                    }
                    Err(Error::Incomplete) => return Ok(None),
                    Err(error) => return Err(error),
                }

                continue;
            }

            let Some(offset) = OggPage::find(remaining) else {
                if !remaining.is_empty() {
                    *consumed = input.len();
                    self.lose();
                }

                return Ok(None);
            };

            if offset != 0 {
                *consumed += offset;
                self.lose();
            }

            match OggPage::parse(&input[*consumed..]) {
                Ok(page) => {
                    if let Some(event) = self.begin(page, consumed) {
                        return Ok(Some(event));
                    }
                }
                Err(Error::Incomplete) => return Ok(None),
                Err(_) => {
                    *consumed += 1;
                    self.lose();
                }
            }
        }
    }

    fn next_packet(
        &mut self,
        input: &[u8],
        storage: &mut [u8],
        consumed: &mut usize,
    ) -> Result<Run> {
        let mut end = self.segment_index;
        let mut size = 0;
        let mut complete = false;

        while end < self.segment_count {
            let lacing = self.segments[end];
            size += lacing as usize;
            end += 1;

            if lacing < 255 {
                complete = true;
                break;
            }
        }

        let Some(bytes) = input.get(*consumed..*consumed + size) else {
            return Ok(Run::Wait);
        };

        let mut overflow = false;

        if !self.discarding {
            match storage.get_mut(self.length..self.length + size) {
                Some(target) => target.copy_from_slice(bytes),
                None => {
                    self.discarding = true;
                    overflow = true;
                }
            }
        }

        self.length += size;
        self.segment_index = end;
        *consumed += size;

        let last = self.segments[end..self.segment_count]
            .iter()
            .all(|&lacing| lacing == 255);

        if end == self.segment_count && self.end_of_stream {
            self.ended = true;
        }

        if overflow {
            return Err(Error::BufferTooSmall);
        }

        self.continuing = !complete;

        if !complete {
            return Ok(Run::Next);
        }

        let length = core::mem::take(&mut self.length);

        if core::mem::take(&mut self.discarding) {
            return Ok(Run::Next);
        }

        Ok(Run::Packet(OggPacket {
            length,
            granule_position: match self.granule_position {
                NO_GRANULE_POSITION => None,
                position => last.then_some(position),
            },
            end_of_stream: last && self.end_of_stream,
        }))
    }

    fn begin<'a>(&mut self, page: OggPage<'a>, consumed: &mut usize) -> Option<OggEvent<'a>> {
        match self.serial_number {
            None => self.serial_number = Some(page.serial_number),
            Some(serial_number) if serial_number != page.serial_number => {
                *consumed += page.size();

                return None;
            }
            Some(_) => {}
        }

        let mut lost = core::mem::take(&mut self.lost);

        if let Some(sequence_number) = self.sequence_number {
            lost |= page.sequence_number != sequence_number.wrapping_add(1);
        }

        if lost {
            self.drop_packet();
        }

        if page.is_continued() {
            if !self.continuing {
                self.discarding = true;
                self.continuing = true;
            }
        } else if self.continuing {
            self.drop_packet();
            lost = true;
        }

        self.sequence_number = Some(page.sequence_number);
        self.granule_position = page.granule_position;
        self.end_of_stream = page.is_end_of_stream();
        self.segment_count = page.segments.len();
        self.segment_index = 0;
        self.segments[..self.segment_count].copy_from_slice(page.segments);
        self.ended = self.segment_count == 0 && self.end_of_stream;
        self.searching = true;
        *consumed += page.header_size();

        Some(OggEvent::Page { page, lost })
    }

    fn drop_packet(&mut self) {
        self.length = 0;
        self.continuing = false;
        self.discarding = false;
    }

    fn lose(&mut self) {
        self.drop_packet();
        self.lost = true;
    }
}

impl Default for OggPacketReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
