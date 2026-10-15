use crate::{Configuration, Error, Format, Result};

const FORM_HEADER_SIZE: usize = 12;
const CHUNK_HEADER_SIZE: usize = 8;
const COMMON_CHUNK_SIZE: usize = 18;
const COMPRESSED_COMMON_CHUNK_SIZE: usize = 22;
const SOUND_HEADER_SIZE: usize = 8;
const EXTENDED_EXPONENT_BIAS: i32 = 16_383;
const EXTENDED_MANTISSA_BITS: i32 = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AiffEncoding {
    BigEndian,
    LittleEndian,
    ALaw,
    MuLaw,
}

impl AiffEncoding {
    const fn sample_size(self, format: Format) -> usize {
        match self {
            AiffEncoding::BigEndian | AiffEncoding::LittleEndian => format.sample_size(),
            AiffEncoding::ALaw | AiffEncoding::MuLaw => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AiffHeader {
    pub configuration: Configuration,
    pub encoding: AiffEncoding,
    pub data_offset: usize,
    pub data_size: u32,
}

#[derive(Debug, Clone, Copy)]
struct Common {
    configuration: Configuration,
    encoding: AiffEncoding,
    frames: u32,
}

impl AiffHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let header = bytes.get(..FORM_HEADER_SIZE).ok_or(Error::Incomplete)?;

        let compressed = match (&header[0..4], &header[8..12]) {
            (b"FORM", b"AIFF") => false,
            (b"FORM", b"AIFC") => true,
            _ => return Err(Error::InvalidHeader),
        };

        let mut offset = FORM_HEADER_SIZE;
        let mut common = None;
        let mut sound = None;

        loop {
            if let (Some(common), Some((data_offset, size))) = (common, sound) {
                return Ok(Self::new(common, data_offset, size));
            }

            let chunk = bytes
                .get(offset..offset.saturating_add(CHUNK_HEADER_SIZE))
                .ok_or(Error::Incomplete)?;
            let identifier = &chunk[0..4];
            let size = read_u32(&chunk[4..8]) as usize;
            let body = offset.saturating_add(CHUNK_HEADER_SIZE);

            match identifier {
                b"COMM" => {
                    let length = if compressed {
                        COMPRESSED_COMMON_CHUNK_SIZE
                    } else {
                        COMMON_CHUNK_SIZE
                    };

                    if size < length {
                        return Err(Error::InvalidHeader);
                    }

                    let chunk = bytes
                        .get(body..body.saturating_add(length))
                        .ok_or(Error::Incomplete)?;

                    common = Some(parse_common(chunk, compressed)?);
                }
                b"SSND" => {
                    let chunk = bytes
                        .get(body..body.saturating_add(SOUND_HEADER_SIZE))
                        .ok_or(Error::Incomplete)?;
                    let data_offset = read_u32(&chunk[0..4]) as usize;
                    let data_size = size
                        .checked_sub(SOUND_HEADER_SIZE)
                        .and_then(|size| size.checked_sub(data_offset))
                        .ok_or(Error::InvalidHeader)?;

                    sound = Some((body + SOUND_HEADER_SIZE + data_offset, data_size));
                }
                _ => {}
            }

            offset = body
                .checked_add(size + (size & 1))
                .ok_or(Error::InvalidHeader)?;
        }
    }

    fn new(common: Common, data_offset: usize, size: usize) -> Self {
        let frame_size = common.configuration.channels as u64
            * common.encoding.sample_size(common.configuration.format) as u64;
        let data_size = (size as u64).min(common.frames as u64 * frame_size);

        Self {
            configuration: common.configuration,
            encoding: common.encoding,
            data_offset,
            data_size: data_size.min(u32::MAX as u64) as u32,
        }
    }

    pub fn frame_size(&self) -> usize {
        self.configuration.channels as usize * self.encoding.sample_size(self.configuration.format)
    }

    pub fn frames(&self) -> usize {
        self.data_size as usize / self.frame_size()
    }
}

fn parse_common(chunk: &[u8], compressed: bool) -> Result<Common> {
    let channels = read_u16(&chunk[0..2]);
    let frames = read_u32(&chunk[2..6]);
    let sample_size = read_u16(&chunk[6..8]);
    let sample_rate = read_extended(&chunk[8..18])?;
    let compression = if compressed { &chunk[18..22] } else { b"NONE" };

    let (format, encoding) = match (compression, sample_size) {
        (b"NONE" | b"twos" | b"sowt", 1..=8) => (Format::U8, AiffEncoding::BigEndian),
        (b"NONE" | b"twos", 9..=16) => (Format::I16, AiffEncoding::BigEndian),
        (b"NONE" | b"twos", 25..=32) => (Format::I32, AiffEncoding::BigEndian),
        (b"sowt", 9..=16) => (Format::I16, AiffEncoding::LittleEndian),
        (b"sowt", 25..=32) => (Format::I32, AiffEncoding::LittleEndian),
        (b"fl32" | b"FL32", 32) => (Format::F32, AiffEncoding::BigEndian),
        (b"alaw" | b"ALAW", _) => (Format::I16, AiffEncoding::ALaw),
        (b"ulaw" | b"ULAW", _) => (Format::I16, AiffEncoding::MuLaw),
        _ => return Err(Error::UnsupportedFormat),
    };

    let channels = u8::try_from(channels).map_err(|_| Error::UnsupportedFormat)?;

    if channels == 0 || sample_rate == 0 {
        return Err(Error::InvalidHeader);
    }

    Ok(Common {
        configuration: Configuration::new(sample_rate, channels, format),
        encoding,
        frames,
    })
}

fn read_extended(bytes: &[u8]) -> Result<u32> {
    let exponent = read_u16(&bytes[0..2]) as i32;
    let mantissa = u64::from_be_bytes([
        bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9],
    ]);
    let shift = u32::try_from(EXTENDED_EXPONENT_BIAS + EXTENDED_MANTISSA_BITS - exponent)
        .map_err(|_| Error::InvalidHeader)?;

    u32::try_from(mantissa.checked_shr(shift).unwrap_or(0)).map_err(|_| Error::InvalidHeader)
}

fn read_u16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    const RATE_44100: [u8; 10] = [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0];

    fn chunk(bytes: &mut Vec<u8>, identifier: &[u8; 4], body: &[u8]) {
        bytes.extend_from_slice(identifier);
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(body);

        if body.len() % 2 == 1 {
            bytes.push(0);
        }
    }

    fn common(
        channels: u16,
        frames: u32,
        bits: u16,
        rate: [u8; 10],
        compression: &[u8],
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&channels.to_be_bytes());
        body.extend_from_slice(&frames.to_be_bytes());
        body.extend_from_slice(&bits.to_be_bytes());
        body.extend_from_slice(&rate);
        body.extend_from_slice(compression);
        body
    }

    fn file(kind: &[u8; 4], common: &[u8], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"FORM");
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(kind);
        chunk(&mut bytes, b"NAME", b"odd");
        chunk(&mut bytes, b"COMM", common);

        let mut sound = Vec::new();
        sound.extend_from_slice(&[0; 8]);
        sound.extend_from_slice(data);
        chunk(&mut bytes, b"SSND", &sound);
        bytes
    }

    #[test]
    fn extended_sample_rates() {
        assert_eq!(read_extended(&RATE_44100), Ok(44_100));
        assert_eq!(
            read_extended(&[0x40, 0x0B, 0xFA, 0, 0, 0, 0, 0, 0, 0]),
            Ok(8_000)
        );
        assert_eq!(
            read_extended(&[0x40, 0x0F, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]),
            Ok(96_000)
        );
        assert_eq!(
            read_extended(&[0x40, 0x0D, 0xAD, 0xDD, 0x17, 0x45, 0xD1, 0x45, 0xD1, 0x74]),
            Ok(22_254)
        );
        assert_eq!(
            read_extended(&[0x3F, 0xFF, 0x80, 0, 0, 0, 0, 0, 0, 0]),
            Ok(1)
        );
        assert_eq!(
            read_extended(&[0x40, 0x1E, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]),
            Ok(u32::MAX)
        );
        assert_eq!(
            read_extended(&[0x3F, 0xFE, 0x80, 0, 0, 0, 0, 0, 0, 0]),
            Ok(0)
        );
        assert_eq!(read_extended(&[0; 10]), Ok(0));
        assert_eq!(
            read_extended(&[0x40, 0x1F, 0x80, 0, 0, 0, 0, 0, 0, 0]),
            Err(Error::InvalidHeader)
        );
        assert_eq!(
            read_extended(&[0xC0, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]),
            Err(Error::InvalidHeader)
        );
    }

    #[test]
    fn pcm() {
        let bytes = file(b"AIFF", &common(2, 2, 16, RATE_44100, &[]), &[0; 8]);
        let header = AiffHeader::parse(&bytes).unwrap();

        assert_eq!(
            header.configuration,
            Configuration::new(44_100, 2, Format::I16)
        );
        assert_eq!(header.encoding, AiffEncoding::BigEndian);
        assert_eq!(header.data_offset, bytes.len() - 8);
        assert_eq!(header.data_size, 8);
        assert_eq!(header.frames(), 2);
    }

    #[test]
    fn compressed() {
        let cases: [(&[u8; 4], u16, Format, AiffEncoding); 8] = [
            (b"NONE", 8, Format::U8, AiffEncoding::BigEndian),
            (b"twos", 12, Format::I16, AiffEncoding::BigEndian),
            (b"NONE", 32, Format::I32, AiffEncoding::BigEndian),
            (b"sowt", 16, Format::I16, AiffEncoding::LittleEndian),
            (b"sowt", 32, Format::I32, AiffEncoding::LittleEndian),
            (b"fl32", 32, Format::F32, AiffEncoding::BigEndian),
            (b"alaw", 16, Format::I16, AiffEncoding::ALaw),
            (b"ULAW", 8, Format::I16, AiffEncoding::MuLaw),
        ];

        for (compression, bits, format, encoding) in cases {
            let mut extension = Vec::from(*compression);
            extension.extend_from_slice(&[3, b'a', b'b', b'c']);
            let bytes = file(
                b"AIFC",
                &common(1, 4, bits, RATE_44100, &extension),
                &[0; 8],
            );
            let header = AiffHeader::parse(&bytes).unwrap();

            assert_eq!(header.configuration, Configuration::new(44_100, 1, format));
            assert_eq!(header.encoding, encoding);
            assert_eq!(header.frames(), 4.min(8 / format.sample_size()));
        }
    }

    #[test]
    fn frames_are_limited_by_the_common_chunk() {
        let bytes = file(b"AIFF", &common(1, 3, 16, RATE_44100, &[]), &[0; 9]);
        let header = AiffHeader::parse(&bytes).unwrap();

        assert_eq!(header.data_size, 6);
        assert_eq!(header.frames(), 3);

        let bytes = file(b"AIFF", &common(1, 30, 16, RATE_44100, &[]), &[0; 9]);
        let header = AiffHeader::parse(&bytes).unwrap();

        assert_eq!(header.data_size, 9);
        assert_eq!(header.frames(), 4);
    }

    #[test]
    fn sound_offset() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"FORM\0\0\0\0AIFF");
        chunk(&mut bytes, b"COMM", &common(1, 2, 8, RATE_44100, &[]));
        chunk(
            &mut bytes,
            b"SSND",
            &[0, 0, 0, 3, 0, 0, 0, 0, 9, 9, 9, 1, 2],
        );

        let header = AiffHeader::parse(&bytes).unwrap();
        assert_eq!(
            &bytes[header.data_offset..][..header.data_size as usize],
            [1, 2]
        );

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"FORM\0\0\0\0AIFF");
        chunk(&mut bytes, b"SSND", &[0, 0, 0, 9, 0, 0, 0, 0, 9]);
        assert_eq!(AiffHeader::parse(&bytes), Err(Error::InvalidHeader));
    }

    #[test]
    fn sound_before_common() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"FORM\0\0\0\0AIFF");
        chunk(&mut bytes, b"SSND", &[0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3]);
        chunk(&mut bytes, b"COMM", &common(1, 3, 8, RATE_44100, &[]));

        let header = AiffHeader::parse(&bytes).unwrap();
        assert_eq!(header.data_offset, 28);
        assert_eq!(header.frames(), 3);

        for length in 0..bytes.len() {
            assert_eq!(AiffHeader::parse(&bytes[..length]), Err(Error::Incomplete));
        }
    }

    #[test]
    fn unsupported() {
        for (kind, bits, compression) in [
            (b"AIFF", 24, &b""[..]),
            (b"AIFF", 64, &b""[..]),
            (b"AIFC", 24, &b"sowt"[..]),
            (b"AIFC", 64, &b"fl64"[..]),
            (b"AIFC", 16, &b"ima4"[..]),
            (b"AIFC", 16, &b"fl32"[..]),
        ] {
            let bytes = file(kind, &common(1, 2, bits, RATE_44100, compression), &[0; 8]);

            assert_eq!(AiffHeader::parse(&bytes), Err(Error::UnsupportedFormat));
        }

        let bytes = file(b"AIFF", &common(256, 2, 16, RATE_44100, &[]), &[0; 8]);
        assert_eq!(AiffHeader::parse(&bytes), Err(Error::UnsupportedFormat));
    }

    #[test]
    fn incomplete() {
        let bytes = file(b"AIFC", &common(2, 2, 16, RATE_44100, b"NONE\0\0"), &[0; 8]);

        for length in 0..bytes.len() - 8 {
            assert_eq!(AiffHeader::parse(&bytes[..length]), Err(Error::Incomplete));
        }
    }

    #[test]
    fn invalid() {
        assert_eq!(
            AiffHeader::parse(b"RIFF\0\0\0\0AIFF"),
            Err(Error::InvalidHeader)
        );
        assert_eq!(
            AiffHeader::parse(b"FORM\0\0\0\0WAVE"),
            Err(Error::InvalidHeader)
        );

        let bytes = file(b"AIFF", &common(0, 2, 16, RATE_44100, &[]), &[0; 8]);
        assert_eq!(AiffHeader::parse(&bytes), Err(Error::InvalidHeader));

        let bytes = file(b"AIFF", &common(1, 2, 16, [0; 10], &[]), &[0; 8]);
        assert_eq!(AiffHeader::parse(&bytes), Err(Error::InvalidHeader));

        let bytes = file(b"AIFC", &common(1, 2, 16, RATE_44100, &[]), &[0; 8]);
        assert_eq!(AiffHeader::parse(&bytes), Err(Error::InvalidHeader));

        let mut bytes = Vec::from(*b"FORM\0\0\0\0AIFF");
        bytes.extend_from_slice(b"junk");
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(AiffHeader::parse(&bytes).is_err());
    }
}
