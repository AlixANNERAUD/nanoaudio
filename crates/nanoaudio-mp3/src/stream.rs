pub(crate) const HEADER_SIZE: usize = 4;
pub(crate) const LARGEST_FRAME: usize = 1729;
pub(crate) const FREE_FORMAT_LIMIT: usize = 2304;
pub(crate) const FREE_FORMAT_KEPT: usize = 2 * (FREE_FORMAT_LIMIT - 1) + HEADER_SIZE;
const SYNC_MATCHES: usize = 10;
const XING_FRAMES: u32 = 1;
const XING_BYTES: u32 = 2;
const XING_TABLE: u32 = 4;
const XING_QUALITY: u32 = 8;
const LAME_SIZE: usize = 24;
const LAME_VERSION: (u8, u32) = (3, 90);
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
pub(crate) struct Header {
    bytes: [u8; HEADER_SIZE],
    free: u16,
}

impl Header {
    pub(crate) fn read(bytes: &[u8]) -> Option<Self> {
        let header = Self {
            bytes: *bytes.first_chunk::<HEADER_SIZE>()?,
            free: 0,
        };

        header.is_valid().then_some(header)
    }

    pub(crate) fn read_like(bytes: &[u8], reference: Self) -> Option<Self> {
        Self::read(bytes)
            .filter(|header| reference.matches(*header))
            .map(|header| Self {
                free: reference.free,
                ..header
            })
    }

    fn is_valid(self) -> bool {
        let [first, second, third, _] = self.bytes;

        first == 0xFF
            && (second & 0xF0 == 0xF0 || second & 0xFE == 0xE2)
            && second & 0x06 != 0
            && third >> 4 != 15
            && (third >> 2) & 3 != 3
    }

    pub(crate) fn matches(self, other: Self) -> bool {
        (self.bytes[1] ^ other.bytes[1]) & 0xFE == 0
            && (self.bytes[2] ^ other.bytes[2]) & 0x0C == 0
            && self.is_free_format() == other.is_free_format()
    }

    fn layer(self) -> u8 {
        4 - ((self.bytes[1] >> 1) & 3)
    }

    pub(crate) fn is_mpeg1(self) -> bool {
        self.bytes[1] & 0x08 != 0
    }

    fn is_mono(self) -> bool {
        self.bytes[3] & 0xC0 == 0xC0
    }

    fn has_crc(self) -> bool {
        self.bytes[1] & 1 == 0
    }

    pub(crate) fn is_free_format(self) -> bool {
        self.bytes[2] & 0xF0 == 0
    }

    pub(crate) fn bitrate(self) -> u32 {
        let layer = self.layer() as usize - 1;

        u32::from(BITRATES[usize::from(self.is_mpeg1())][layer][(self.bytes[2] >> 4) as usize])
    }

    pub(crate) fn sample_rate(self) -> u32 {
        let shift = u32::from(!self.is_mpeg1()) + u32::from(self.bytes[1] & 0x10 == 0);

        SAMPLE_RATES[((self.bytes[2] >> 2) & 3) as usize] >> shift
    }

    pub(crate) fn samples(self) -> u32 {
        match self.layer() {
            1 => 384,
            3 if !self.is_mpeg1() => 576,
            _ => 1152,
        }
    }

    fn padding(self) -> usize {
        match (self.bytes[2] & 0x02 != 0, self.layer()) {
            (false, _) => 0,
            (true, 1) => 4,
            (true, _) => 1,
        }
    }

    fn padding_unit(self) -> u64 {
        if self.layer() == 1 { 4 } else { 1 }
    }

    pub(crate) fn unpadded_size(self) -> usize {
        if self.is_free_format() {
            return usize::from(self.free);
        }

        self.standard_size(self.bitrate())
    }

    fn standard_size(self, bitrate: u32) -> usize {
        let size = (self.samples() * bitrate * 125 / self.sample_rate()) as usize;

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
        let Some(header) = Header::read_like(&input[position..], first) else {
            return false;
        };

        position += header.size();

        if position + HEADER_SIZE > input.len() {
            return count > 0;
        }

        if Header::read_like(&input[position..], first).is_none() {
            return false;
        }
    }

    true
}

fn free_format_size(input: &[u8], first: Header) -> Option<u16> {
    (HEADER_SIZE..FREE_FORMAT_LIMIT)
        .take_while(|distance| 2 * distance + HEADER_SIZE < input.len())
        .find_map(|distance| {
            let next = Header::read(&input[distance..]).filter(|next| first.matches(*next))?;
            let size = distance - first.padding();
            let after = distance + size + next.padding();

            (size != 0
                && after + HEADER_SIZE <= input.len()
                && Header::read(&input[after..]).is_some_and(|last| first.matches(last)))
            .then_some(size as u16)
        })
}

pub(crate) fn free_format_start(input: &[u8]) -> Option<usize> {
    input
        .windows(HEADER_SIZE)
        .position(|bytes| Header::read(bytes).is_some_and(|header| header.is_free_format()))
}

pub(crate) fn find_frame(input: &[u8], reference: Option<Header>) -> Option<(usize, Header)> {
    (0..input.len().saturating_sub(HEADER_SIZE)).find_map(|index| {
        let header = match reference {
            Some(reference) => Header::read_like(&input[index..], reference)?,
            None => {
                let header = Header::read(&input[index..])?;

                match header.is_free_format() {
                    true => Header {
                        free: free_format_size(&input[index..], header)?,
                        ..header
                    },
                    false => header,
                }
            }
        };
        let size = header.size();

        ((index + size <= input.len() && matches_following(&input[index..], header))
            || (index == 0 && size == input.len()))
        .then_some((index, header))
    })
}

pub(crate) fn rate(input: &[u8], header: Header) -> (u64, u64) {
    let sample_rate = u64::from(header.sample_rate());
    let scale = u64::from(header.samples()) * 125;

    if !header.is_free_format() {
        return (scale * u64::from(header.bitrate()), sample_rate);
    }

    let size = header.unpadded_size() as u64;
    let unit = header.padding_unit();
    let (mut frames, mut padded, mut position) = (0, 0, 0);

    while let Some(next) = input
        .get(position..)
        .and_then(|rest| Header::read_like(rest, header))
    {
        frames += 1;
        padded += u64::from(next.padding() != 0);
        position += next.size();
    }

    let frames = frames.max(1);
    let bitrate = (size * sample_rate).div_ceil(scale);
    let average = scale * bitrate;
    let consistent = u32::try_from(bitrate)
        .is_ok_and(|bitrate| header.standard_size(bitrate) as u64 == size)
        && (padded * sample_rate * unit).abs_diff(frames * (average - size * sample_rate))
            <= sample_rate * unit;

    if consistent {
        (average, sample_rate)
    } else {
        (frames * size + padded * unit, frames)
    }
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
pub(crate) struct Gapless {
    pub(crate) delay: u16,
    pub(crate) padding: u16,
}

impl Gapless {
    fn parse(lame: &[u8]) -> Option<Self> {
        let lame = lame.first_chunk::<LAME_SIZE>()?;
        let (encoder, version) = lame.split_at(4);

        if !matches!(encoder, b"Lavc" | b"Lavf") && !(encoder == b"LAME" && supported(version)) {
            return None;
        }

        let delay = u32::from_be_bytes([0, lame[21], lame[22], lame[23]]);

        Some(Self {
            delay: (delay >> 12) as u16,
            padding: (delay & 0x0FFF) as u16,
        })
    }
}

fn supported(version: &[u8]) -> bool {
    let [major, b'.', minor @ ..] = version else {
        return false;
    };
    let digits = minor
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();

    major.is_ascii_digit()
        && digits != 0
        && (
            major - b'0',
            minor[..digits]
                .iter()
                .fold(0, |value, digit| value * 10 + u32::from(digit - b'0')),
        ) >= LAME_VERSION
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tag {
    pub(crate) kind: Kind,
    pub(crate) frames: Option<u32>,
    pub(crate) table: Table,
    pub(crate) gapless: Option<Gapless>,
}

impl Tag {
    pub(crate) fn parse(frame: &[u8], header: Header) -> Self {
        let plain = Self {
            kind: Kind::Plain,
            frames: None,
            table: Table::EMPTY,
            gapless: None,
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
        let toc = (flags & XING_TABLE != 0)
            .then(|| rest.split_first_chunk::<PERCENT>())
            .flatten()
            .map(|(toc, after)| {
                rest = after;
                toc
            });
        let mut table = Table::EMPTY;

        if flags & XING_QUALITY != 0 {
            rest = rest.get(4..).unwrap_or_default();
        }

        if let (Some(frames), Some(bytes), Some(toc)) = (frames, bytes, toc) {
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
            gapless: (toc.is_some() || flags & XING_TABLE == 0)
                .then(|| Gapless::parse(rest))
                .flatten(),
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
            gapless: None,
        })
    }
}

fn read_u32(input: &mut &[u8]) -> Option<u32> {
    let (value, rest) = input.split_first_chunk::<4>()?;

    *input = rest;

    Some(u32::from_be_bytes(*value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALE: u64 = 576 * 125;
    const SAMPLE_RATE: u64 = 22_050;

    fn stream(sizes: impl Iterator<Item = u64>, unpadded: u64) -> [u8; 4096] {
        let mut input = [0; 4096];
        let mut position = 0;

        for size in sizes {
            if position + HEADER_SIZE > input.len() {
                break;
            }

            let padded = u8::from(size > unpadded);

            input[position..position + HEADER_SIZE].copy_from_slice(&[
                0xFF,
                0xF3,
                padded << 1,
                0xC4,
            ]);
            position += size as usize;
        }

        input
    }

    fn rate_of(input: &[u8]) -> (u64, u64) {
        let (index, header) = find_frame(input, None).unwrap();

        assert_eq!(index, 0);

        rate(input, header)
    }

    #[test]
    fn free_format_padding_gives_the_integer_bitrate() {
        let average = SCALE * 45;
        let sizes = (0..)
            .map(|index: u64| (index + 1) * average / SAMPLE_RATE - index * average / SAMPLE_RATE);

        assert_eq!(rate_of(&stream(sizes, 146)), (average, SAMPLE_RATE));
    }

    #[test]
    fn free_format_without_padding_keeps_its_size() {
        let (bytes, frames) = rate_of(&stream(core::iter::repeat(146), 146));

        assert_eq!(bytes, 146 * frames);
    }
}
