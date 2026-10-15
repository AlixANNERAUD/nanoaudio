use nanoaudio_adpcm::{MS_ADPCM_MAXIMUM_COEFFICIENTS, MsAdpcm, MsAdpcmCoefficients};
use nanoaudio_core::{Configuration, Error, Format, Result};

const RIFF_HEADER_SIZE: usize = 12;
const CHUNK_HEADER_SIZE: usize = 8;
const FORMAT_CHUNK_MINIMUM_SIZE: usize = 16;
const MS_ADPCM_COEFFICIENTS_OFFSET: usize = 22;
const FORMAT_CHUNK_MAXIMUM_SIZE: usize =
    MS_ADPCM_COEFFICIENTS_OFFSET + 4 * MS_ADPCM_MAXIMUM_COEFFICIENTS;

const FORMAT_PCM: u16 = 0x0001;
const FORMAT_MS_ADPCM: u16 = 0x0002;
const FORMAT_IEEE_FLOAT: u16 = 0x0003;
const FORMAT_A_LAW: u16 = 0x0006;
const FORMAT_MU_LAW: u16 = 0x0007;
const FORMAT_IMA_ADPCM: u16 = 0x0011;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Encoding {
    Pcm,
    ALaw,
    MuLaw,
    ImaAdpcm {
        block_align: u16,
    },
    MsAdpcm {
        block_align: u16,
        samples_per_block: u16,
        coefficients: MsAdpcmCoefficients,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WavHeader {
    pub configuration: Configuration,
    pub encoding: Encoding,
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
        let mut format = None;

        loop {
            let chunk = bytes
                .get(offset..offset.saturating_add(CHUNK_HEADER_SIZE))
                .ok_or(Error::Incomplete)?;
            let identifier = &chunk[0..4];
            let size = read_u32(&chunk[4..8]);
            let body = offset.saturating_add(CHUNK_HEADER_SIZE);

            match identifier {
                b"fmt " => {
                    let length = (size as usize).min(FORMAT_CHUNK_MAXIMUM_SIZE);
                    let chunk = bytes
                        .get(body..body.saturating_add(length))
                        .ok_or(Error::Incomplete)?;

                    format = Some(parse_format(chunk)?);
                }
                b"data" => {
                    let (configuration, encoding) = format.ok_or(Error::InvalidHeader)?;

                    return Ok(Self {
                        configuration,
                        encoding,
                        data_offset: body,
                        data_size: size,
                    });
                }
                _ => {}
            }

            offset = (size as usize)
                .checked_add(size as usize & 1)
                .and_then(|padded| body.checked_add(padded))
                .ok_or(Error::InvalidHeader)?;
        }
    }

    pub fn frames(&self) -> usize {
        let size = self.data_size as usize;
        let channels = self.configuration.channels as usize;

        match self.encoding {
            Encoding::Pcm => self.configuration.bytes_to_frames(size),
            Encoding::ALaw | Encoding::MuLaw => size / channels,
            Encoding::ImaAdpcm { block_align } => {
                let block = block_align as usize;
                let samples = |length: usize| match length.checked_sub(4 * channels) {
                    Some(data) => data / (4 * channels) * 8 + 1,
                    None => 0,
                };

                size / block * samples(block) + samples(size % block)
            }
            Encoding::MsAdpcm {
                block_align,
                samples_per_block,
                ..
            } => size / block_align as usize * samples_per_block as usize,
        }
    }
}

fn parse_format(chunk: &[u8]) -> Result<(Configuration, Encoding)> {
    if chunk.len() < FORMAT_CHUNK_MINIMUM_SIZE {
        return Err(Error::InvalidHeader);
    }

    let mut tag = read_u16(&chunk[0..2]);
    let channels = read_u16(&chunk[2..4]);
    let sample_rate = read_u32(&chunk[4..8]);
    let block_align = read_u16(&chunk[12..14]);
    let bits_per_sample = read_u16(&chunk[14..16]);

    if tag == FORMAT_EXTENSIBLE {
        let subformat = chunk.get(24..26).ok_or(Error::InvalidHeader)?;

        tag = read_u16(subformat);
    }

    let (format, encoding) = match (tag, bits_per_sample) {
        (FORMAT_PCM, 8) => (Format::U8, Encoding::Pcm),
        (FORMAT_PCM, 16) => (Format::I16, Encoding::Pcm),
        (FORMAT_PCM, 24) => (Format::I24, Encoding::Pcm),
        (FORMAT_PCM, 32) => (Format::I32, Encoding::Pcm),
        (FORMAT_IEEE_FLOAT, 32) => (Format::F32, Encoding::Pcm),
        (FORMAT_A_LAW, 8) => (Format::I16, Encoding::ALaw),
        (FORMAT_MU_LAW, 8) => (Format::I16, Encoding::MuLaw),
        (FORMAT_IMA_ADPCM, 4) => (Format::I16, Encoding::ImaAdpcm { block_align }),
        (FORMAT_MS_ADPCM, 4) => (Format::I16, parse_ms_adpcm(chunk, block_align)?),
        _ => return Err(Error::UnsupportedFormat),
    };

    let channels = u8::try_from(channels).map_err(|_| Error::UnsupportedFormat)?;

    if channels == 0 || sample_rate == 0 {
        return Err(Error::InvalidHeader);
    }

    if let Encoding::ImaAdpcm { block_align } = encoding {
        let header = 4 * channels as usize;
        let block = block_align as usize;

        if block <= header || !(block - header).is_multiple_of(header) {
            return Err(Error::InvalidHeader);
        }
    }

    if let Encoding::MsAdpcm {
        block_align,
        samples_per_block,
        coefficients,
    } = encoding
    {
        MsAdpcm::with_coefficients(
            sample_rate,
            channels,
            block_align,
            samples_per_block,
            coefficients,
        )
        .map_err(|error| match error {
            Error::TooManyChannels => Error::UnsupportedFormat,
            _ => Error::InvalidHeader,
        })?;
    }

    Ok((Configuration::new(sample_rate, channels, format), encoding))
}

fn parse_ms_adpcm(chunk: &[u8], block_align: u16) -> Result<Encoding> {
    let extension = chunk
        .get(FORMAT_CHUNK_MINIMUM_SIZE..MS_ADPCM_COEFFICIENTS_OFFSET)
        .ok_or(Error::InvalidHeader)?;
    let samples_per_block = read_u16(&extension[2..4]);
    let count = read_u16(&extension[4..6]) as usize;

    if count > MS_ADPCM_MAXIMUM_COEFFICIENTS {
        return Err(Error::UnsupportedFormat);
    }

    let bytes = chunk
        .get(MS_ADPCM_COEFFICIENTS_OFFSET..MS_ADPCM_COEFFICIENTS_OFFSET + 4 * count)
        .ok_or(Error::InvalidHeader)?;
    let mut pairs = [[0i16; 2]; MS_ADPCM_MAXIMUM_COEFFICIENTS];

    for (pair, bytes) in pairs.iter_mut().zip(bytes.as_chunks::<4>().0) {
        *pair = [
            i16::from_le_bytes([bytes[0], bytes[1]]),
            i16::from_le_bytes([bytes[2], bytes[3]]),
        ];
    }

    Ok(Encoding::MsAdpcm {
        block_align,
        samples_per_block,
        coefficients: MsAdpcmCoefficients::new(&pairs[..count])
            .map_err(|_| Error::InvalidHeader)?,
    })
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
    fn twenty_four_bit() {
        let bytes = header(FORMAT_PCM, 2, 48_000, 24, &[]);
        let header = WavHeader::parse(&bytes).unwrap();

        assert_eq!(
            header.configuration,
            Configuration::new(48_000, 2, Format::I24)
        );
        assert_eq!(header.configuration.frame_size(), 6);
        assert_eq!(header.frames(), 1);
    }

    #[test]
    fn twenty_four_bit_extensible() {
        let mut extra = std::vec![22, 0, 24, 0, 3, 0, 0, 0];
        extra.extend_from_slice(&FORMAT_PCM.to_le_bytes());
        extra.extend_from_slice(&[0; 14]);
        let bytes = header(FORMAT_EXTENSIBLE, 1, 96_000, 24, &extra);

        assert_eq!(
            WavHeader::parse(&bytes).unwrap().configuration,
            Configuration::new(96_000, 1, Format::I24)
        );
    }

    #[test]
    fn compressed() {
        let a_law = WavHeader::parse(&header(FORMAT_A_LAW, 1, 8_000, 8, &[0, 0])).unwrap();
        assert_eq!(a_law.encoding, Encoding::ALaw);
        assert_eq!(
            a_law.configuration,
            Configuration::new(8_000, 1, Format::I16)
        );
        assert_eq!(a_law.frames(), 8);

        let mu_law = WavHeader::parse(&header(FORMAT_MU_LAW, 2, 8_000, 8, &[0, 0])).unwrap();
        assert_eq!(mu_law.encoding, Encoding::MuLaw);
        assert_eq!(mu_law.frames(), 4);

        let mut bytes = header(FORMAT_IMA_ADPCM, 1, 8_000, 4, &[2, 0, 0xF9, 0x01]);
        let block_align = 12 + 12 + 8 + 12;
        bytes[block_align..block_align + 2].copy_from_slice(&256u16.to_le_bytes());
        let size = bytes.len() - 4;
        bytes[size..].copy_from_slice(&512u32.to_le_bytes());

        let adpcm = WavHeader::parse(&bytes).unwrap();
        assert_eq!(adpcm.encoding, Encoding::ImaAdpcm { block_align: 256 });
        assert_eq!(adpcm.frames(), 2 * 505);

        for (remainder, frames) in [(0, 0), (3, 0), (4, 1), (11, 9), (12, 17), (255, 1 + 62 * 8)] {
            let partial = WavHeader {
                data_size: 512 + remainder,
                ..adpcm
            };

            assert_eq!(partial.frames(), 2 * 505 + frames);
        }
    }

    #[test]
    fn unsupported() {
        let bytes = header(FORMAT_PCM, 2, 44_100, 20, &[]);

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
