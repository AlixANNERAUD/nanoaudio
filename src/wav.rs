use crate::{Configuration, Error, Format, Result};

const RIFF_HEADER_SIZE: usize = 12;
const CHUNK_HEADER_SIZE: usize = 8;
const FORMAT_CHUNK_MINIMUM_SIZE: usize = 16;
const FORMAT_CHUNK_EXTENSIBLE_SIZE: usize = 40;

const FORMAT_PCM: u16 = 0x0001;
const FORMAT_IEEE_FLOAT: u16 = 0x0003;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WavHeader {
    pub configuration: Configuration,
    pub data_offset: usize,
    pub data_size: u32,
}

impl WavHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let header = bytes.get(..RIFF_HEADER_SIZE).ok_or(Error::Incomplete)?;

        if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
            return Err(Error::InvalidHeader);
        }

        let mut offset = RIFF_HEADER_SIZE;
        let mut configuration = None;

        loop {
            let chunk = bytes
                .get(offset..offset.saturating_add(CHUNK_HEADER_SIZE))
                .ok_or(Error::Incomplete)?;
            let identifier = &chunk[0..4];
            let size = read_u32(&chunk[4..8]);
            let body = offset.saturating_add(CHUNK_HEADER_SIZE);

            match identifier {
                b"fmt " => {
                    let length = (size as usize).min(FORMAT_CHUNK_EXTENSIBLE_SIZE);
                    let format = bytes
                        .get(body..body.saturating_add(length))
                        .ok_or(Error::Incomplete)?;

                    configuration = Some(parse_format(format)?);
                }
                b"data" => {
                    return Ok(Self {
                        configuration: configuration.ok_or(Error::InvalidHeader)?,
                        data_offset: body,
                        data_size: size,
                    });
                }
                _ => {}
            }

            offset = body
                .checked_add(size as usize + (size as usize & 1))
                .ok_or(Error::InvalidHeader)?;
        }
    }

    pub fn frames(&self) -> usize {
        self.configuration.bytes_to_frames(self.data_size as usize)
    }
}

fn parse_format(chunk: &[u8]) -> Result<Configuration> {
    if chunk.len() < FORMAT_CHUNK_MINIMUM_SIZE {
        return Err(Error::InvalidHeader);
    }

    let mut tag = read_u16(&chunk[0..2]);
    let channels = read_u16(&chunk[2..4]);
    let sample_rate = read_u32(&chunk[4..8]);
    let bits_per_sample = read_u16(&chunk[14..16]);

    if tag == FORMAT_EXTENSIBLE {
        let subformat = chunk.get(24..26).ok_or(Error::InvalidHeader)?;

        tag = read_u16(subformat);
    }

    let format = match (tag, bits_per_sample) {
        (FORMAT_PCM, 8) => Format::U8,
        (FORMAT_PCM, 16) => Format::I16,
        (FORMAT_PCM, 32) => Format::I32,
        (FORMAT_IEEE_FLOAT, 32) => Format::F32,
        _ => return Err(Error::UnsupportedFormat),
    };

    let channels = u8::try_from(channels).map_err(|_| Error::UnsupportedFormat)?;

    if channels == 0 || sample_rate == 0 {
        return Err(Error::InvalidHeader);
    }

    Ok(Configuration::new(sample_rate, channels, format))
}

fn read_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    fn header(tag: u16, channels: u16, sample_rate: u32, bits: u16, extra: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");

        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 0]);

        let mut format = Vec::new();
        format.extend_from_slice(&tag.to_le_bytes());
        format.extend_from_slice(&channels.to_le_bytes());
        format.extend_from_slice(&sample_rate.to_le_bytes());
        format.extend_from_slice(&(sample_rate * (channels * bits / 8) as u32).to_le_bytes());
        format.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        format.extend_from_slice(&bits.to_le_bytes());
        format.extend_from_slice(extra);

        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&(format.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&format);

        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes
    }

    #[test]
    fn pcm() {
        let bytes = header(FORMAT_PCM, 2, 44_100, 16, &[]);
        let header = WavHeader::parse(&bytes).unwrap();

        assert_eq!(
            header.configuration,
            Configuration::new(44_100, 2, Format::I16)
        );
        assert_eq!(header.data_offset, bytes.len());
        assert_eq!(header.data_size, 8);
        assert_eq!(header.frames(), 2);
    }

    #[test]
    fn float() {
        let bytes = header(FORMAT_IEEE_FLOAT, 1, 48_000, 32, &[0, 0]);

        assert_eq!(
            WavHeader::parse(&bytes).unwrap().configuration,
            Configuration::new(48_000, 1, Format::F32)
        );
    }

    #[test]
    fn extensible() {
        let mut extra = std::vec![22, 0, 16, 0, 3, 0, 0, 0];
        extra.extend_from_slice(&FORMAT_PCM.to_le_bytes());
        extra.extend_from_slice(&[0; 14]);
        let bytes = header(FORMAT_EXTENSIBLE, 2, 16_000, 8, &extra);

        assert_eq!(
            WavHeader::parse(&bytes).unwrap().configuration,
            Configuration::new(16_000, 2, Format::U8)
        );
    }

    #[test]
    fn unsupported() {
        let bytes = header(FORMAT_PCM, 2, 44_100, 24, &[]);

        assert_eq!(WavHeader::parse(&bytes), Err(Error::UnsupportedFormat));
    }

    #[test]
    fn incomplete() {
        let bytes = header(FORMAT_PCM, 2, 44_100, 16, &[]);

        for length in 0..bytes.len() {
            assert_eq!(WavHeader::parse(&bytes[..length]), Err(Error::Incomplete));
        }
    }

    #[test]
    fn invalid() {
        assert_eq!(
            WavHeader::parse(b"RIFX\0\0\0\0WAVE"),
            Err(Error::InvalidHeader)
        );

        let mut bytes = std::vec::Vec::from(*b"RIFF\0\0\0\0WAVE");
        bytes.extend_from_slice(b"data\0\0\0\0");
        assert_eq!(WavHeader::parse(&bytes), Err(Error::InvalidHeader));
    }

    #[test]
    fn huge_chunk_sizes_do_not_overflow() {
        let mut bytes = std::vec::Vec::from(*b"RIFF\0\0\0\0WAVE");
        bytes.extend_from_slice(b"junk");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());

        assert!(WavHeader::parse(&bytes).is_err());
    }
}
