pub(crate) const HEADER_SIZE: usize = 4;
pub(crate) const LARGEST_FRAME: usize = 1729;
const SYNC_MATCHES: usize = 10;
const XING_FRAMES: u32 = 1;
const XING_BYTES: u32 = 2;
const XING_TABLE: u32 = 4;
const VBRI_OFFSET: usize = HEADER_SIZE + 32;
const VBRI_HEADER_SIZE: usize = 26;
const PERCENT: usize = 100;

pub(crate) const TABLE_POINTS: usize = PERCENT + 1;

const BITRATES: [[[u16; 15]; 3]; 2] = [
    [
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
        ],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    ],
    [
        [
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
        ],
        [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
        ],
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ],
    ],
];

const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 32_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header([u8; HEADER_SIZE]);

impl Header {
    pub(crate) fn read(bytes: &[u8]) -> Option<Self> {
        let header = Self(*bytes.first_chunk::<HEADER_SIZE>()?);

        header.is_valid().then_some(header)
    }

    fn is_valid(self) -> bool {
        let [first, second, third, _] = self.0;

        first == 0xFF
            && (second & 0xF0 == 0xF0 || second & 0xFE == 0xE2)
            && self.layer() != 0
            && third >> 4 != 15
            && (third >> 2) & 3 != 3
    }

    pub(crate) fn matches(self, other: Self) -> bool {
        (self.0[1] ^ other.0[1]) & 0xFE == 0
            && (self.0[2] ^ other.0[2]) & 0x0C == 0
            && self.is_free_format() == other.is_free_format()
    }

    fn layer(self) -> u8 {
        4 - ((self.0[1] >> 1) & 3)
    }

    pub(crate) fn is_mpeg1(self) -> bool {
        self.0[1] & 0x08 != 0
    }

    fn is_mono(self) -> bool {
        self.0[3] & 0xC0 == 0xC0
    }

    fn has_crc(self) -> bool {
        self.0[1] & 1 == 0
    }

    pub(crate) fn is_free_format(self) -> bool {
        self.0[2] & 0xF0 == 0
    }

    pub(crate) fn bitrate(self) -> u32 {
        let layer = self.layer() as usize - 1;

        u32::from(BITRATES[usize::from(self.is_mpeg1())][layer][(self.0[2] >> 4) as usize])
    }

    pub(crate) fn sample_rate(self) -> u32 {
        let shift = u32::from(!self.is_mpeg1()) + u32::from(self.0[1] & 0x10 == 0);

        SAMPLE_RATES[((self.0[2] >> 2) & 3) as usize] >> shift
    }

    pub(crate) fn samples(self) -> u32 {
        match self.layer() {
            1 => 384,
            3 if !self.is_mpeg1() => 576,
            _ => 1152,
        }
    }

    fn padding(self) -> usize {
        match (self.0[2] & 0x02 != 0, self.layer()) {
            (false, _) => 0,
            (true, 1) => 4,
            (true, _) => 1,
        }
    }

    pub(crate) fn unpadded_size(self) -> usize {
        let size = (self.samples() * self.bitrate() * 125 / self.sample_rate()) as usize;

        if self.layer() == 1 { size & !3 } else { size }
    }

    pub(crate) fn size(self) -> usize {
        self.unpadded_size() + self.padding()
    }

    pub(crate) fn side_information_size(self) -> usize {
        let side = match (self.layer(), self.is_mpeg1(), self.is_mono()) {
            (3, true, true) => 17,
            (3, true, false) => 32,
            (3, false, true) => 9,
            (3, false, false) => 17,
            _ => 0,
        };

        HEADER_SIZE + 2 * usize::from(self.has_crc()) + side
    }

    pub(crate) fn maximum_reservoir(self) -> usize {
        match (self.layer(), self.is_mpeg1()) {
            (3, true) => 511,
            (3, false) => 255,
            _ => 0,
        }
    }
}

fn matches_following(input: &[u8], first: Header) -> bool {
    let mut position = 0;

    for count in 0..SYNC_MATCHES {
        let Some(header) = Header::read(&input[position..]) else {
            return false;
        };

        position += header.size();

        if position + HEADER_SIZE > input.len() {
            return count > 0;
        }

        if !Header::read(&input[position..]).is_some_and(|next| first.matches(next)) {
            return false;
        }
    }

    true
}

pub(crate) fn find_frame(input: &[u8], reference: Option<Header>) -> Option<(usize, Header)> {
    (0..input.len().saturating_sub(HEADER_SIZE)).find_map(|index| {
        let header = Header::read(&input[index..])
            .filter(|header| !header.is_free_format())
            .filter(|header| reference.is_none_or(|reference| reference.matches(*header)))?;
        let size = header.size();

        ((index + size <= input.len() && matches_following(&input[index..], header))
            || (index == 0 && size == input.len()))
        .then_some((index, header))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Plain,
    Constant,
    Variable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Table {
    pub(crate) frames: [u32; TABLE_POINTS],
    pub(crate) bytes: [u32; TABLE_POINTS],
    pub(crate) length: usize,
}

impl Table {
    const EMPTY: Self = Self {
        frames: [0; TABLE_POINTS],
        bytes: [0; TABLE_POINTS],
        length: 0,
    };

    fn push(&mut self, frame: u32, byte: u32) {
        if self.length < TABLE_POINTS {
            self.frames[self.length] = frame;
            self.bytes[self.length] = byte;
            self.length += 1;
        }
    }

    pub(crate) fn position(&self, frame: u64) -> Option<u64> {
        let frames = &self.frames[..self.length];
        let index = frames
            .partition_point(|point| u64::from(*point) <= frame)
            .checked_sub(1)?;
        let (start, byte) = (u64::from(frames[index]), u64::from(self.bytes[index]));
        let Some((&end, &next)) = frames.get(index + 1).zip(self.bytes.get(index + 1)) else {
            return Some(byte);
        };
        let span = u64::from(end) - start;

        Some(byte + (u64::from(next).saturating_sub(byte)) * (frame - start) / span.max(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tag {
    pub(crate) kind: Kind,
    pub(crate) frames: Option<u32>,
    pub(crate) table: Table,
}

impl Tag {
    pub(crate) fn parse(frame: &[u8], header: Header) -> Self {
        let plain = Self {
            kind: Kind::Plain,
            frames: None,
            table: Table::EMPTY,
        };

        if header.layer() != 3 {
            return plain;
        }

        Self::xing(frame.get(header.side_information_size()..).unwrap_or(&[]))
            .or_else(|| Self::vbri(frame.get(VBRI_OFFSET..).unwrap_or(&[]), frame.len()))
            .unwrap_or(plain)
    }

    fn xing(tag: &[u8]) -> Option<Self> {
        let (marker, mut rest) = tag.split_first_chunk::<4>()?;
        let kind = match marker {
            b"Xing" => Kind::Variable,
            b"Info" => Kind::Constant,
            _ => return None,
        };
        let flags = read_u32(&mut rest)?;
        let frames = (flags & XING_FRAMES != 0)
            .then(|| read_u32(&mut rest))
            .flatten();
        let bytes = (flags & XING_BYTES != 0)
            .then(|| read_u32(&mut rest))
            .flatten();
        let mut table = Table::EMPTY;

        if let (Some(frames), Some(bytes), Some(toc)) = (
            frames,
            bytes,
            (flags & XING_TABLE != 0)
                .then(|| rest.first_chunk::<PERCENT>())
                .flatten(),
        ) {
            for (percent, entry) in toc.iter().enumerate() {
                table.push(
                    (u64::from(frames) * percent as u64 / PERCENT as u64) as u32,
                    (u64::from(*entry) * u64::from(bytes) / 256) as u32,
                );
            }

            table.push(frames, bytes);
        } else if let (Some(frames), Some(bytes)) = (frames, bytes) {
            table.push(0, 0);
            table.push(frames, bytes);
        }

        Some(Self {
            kind,
            frames,
            table,
        })
    }

    fn vbri(tag: &[u8], frame_size: usize) -> Option<Self> {
        let (header, entries) = tag.split_first_chunk::<VBRI_HEADER_SIZE>()?;

        if &header[..4] != b"VBRI" {
            return None;
        }

        let field = |index: usize| u16::from_be_bytes([header[index], header[index + 1]]);
        let frames = u32::from_be_bytes([header[14], header[15], header[16], header[17]]);
        let count = field(18) as usize;
        let scale = u32::from(field(20));
        let entry_size = field(22) as usize;
        let per_entry = u32::from(field(24));
        let mut table = Table::EMPTY;

        if (1..=4).contains(&entry_size) && entries.len() >= count * entry_size {
            let stride = count.div_ceil(PERCENT).max(1);
            let mut byte = frame_size as u32;

            table.push(0, byte);

            for (index, entry) in entries[..count * entry_size]
                .chunks_exact(entry_size)
                .enumerate()
            {
                let size = entry
                    .iter()
                    .fold(0u32, |value, byte| value << 8 | u32::from(*byte));

                byte = byte.saturating_add(size.saturating_mul(scale));

                if (index + 1) % stride == 0 || index + 1 == count {
                    table.push(
                        ((index as u32 + 1).saturating_mul(per_entry)).min(frames),
                        byte,
                    );
                }
            }
        }

        Some(Self {
            kind: Kind::Variable,
            frames: Some(frames),
            table,
        })
    }
}

fn read_u32(input: &mut &[u8]) -> Option<u32> {
    let (value, rest) = input.split_first_chunk::<4>()?;

    *input = rest;

    Some(u32::from_be_bytes(*value))
}
