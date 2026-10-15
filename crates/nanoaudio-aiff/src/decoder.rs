use core::marker::PhantomData;

use nanoaudio_core::codec::{Decoder, Progress, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Format, I24, Result, Sample};
use nanoaudio_g711::{G711, Law};

use crate::{AiffEncoding, AiffHeader};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Codec {
    Pcm { format: Format, little_endian: bool },
    G711(G711),
}

/// A decoder for the sound data of an AIFF or AIFF-C file, producing interleaved samples of type `S`.
///
/// Feed it the bytes that follow [`AiffHeader::data_offset`]. Any [`Sample`] type works as output: PCM is converted from the format in the file, with 8-bit samples read as signed, and A-law and µ-law are decoded to 16-bit and then converted.
///
/// With [`Seek`], the returned [`SeekRequest::byte_offset`] counts from `data_offset`, not from the start of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiffDecoder<S: Sample = i16> {
    codec: Codec,
    sample_rate: u32,
    channels: u8,
    frames: u64,
    sample: PhantomData<S>,
}

impl<S: Sample> AiffDecoder<S> {
    /// Creates a decoder for the encoding described by `header`.
    pub fn new(header: &AiffHeader) -> Result<Self> {
        let Configuration {
            sample_rate,
            channels,
            format,
        } = header.configuration;

        let codec = match header.encoding {
            AiffEncoding::BigEndian => Codec::Pcm {
                format,
                little_endian: false,
            },
            AiffEncoding::LittleEndian => Codec::Pcm {
                format,
                little_endian: true,
            },
            AiffEncoding::ALaw => Codec::G711(G711::new(Law::A, sample_rate, channels)?),
            AiffEncoding::MuLaw => Codec::G711(G711::new(Law::Mu, sample_rate, channels)?),
        };

        Ok(Self {
            codec,
            sample_rate,
            channels,
            frames: header.frames() as u64,
            sample: PhantomData,
        })
    }

    fn decode_pcm(
        &self,
        format: Format,
        little_endian: bool,
        input: &[u8],
        output: &mut [S],
    ) -> Progress {
        let channels = self.channels as usize;
        let size = format.sample_size();
        let frame_size = channels * size;
        let frames = (input.len() / frame_size).min(output.len() / channels);
        let samples = frames * channels;

        for (output, bytes) in output[..samples].iter_mut().zip(input.chunks_exact(size)) {
            let mut buffer = [0u8; 4];
            buffer[..size].copy_from_slice(bytes);

            if !little_endian {
                buffer[..size].reverse();
            }

            *output = match format {
                Format::U8 => (buffer[0] ^ 0x80).convert(),
                Format::I16 => i16::read_le(&buffer).convert(),
                Format::I24 => I24::read_le(&buffer).convert(),
                Format::I32 => i32::read_le(&buffer).convert(),
                Format::F32 => f32::read_le(&buffer).convert(),
            };
        }

        Progress::new(frames * frame_size, samples)
    }
}

impl<S: Sample> Decoder for AiffDecoder<S> {
    type Sample = S;

    fn configuration(&self) -> Option<Configuration> {
        Some(Configuration::new(
            self.sample_rate,
            self.channels,
            S::FORMAT,
        ))
    }

    fn minimum_output(&self) -> usize {
        self.channels as usize
    }

    fn decode(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        match &mut self.codec {
            Codec::Pcm {
                format,
                little_endian,
            } => {
                let (format, little_endian) = (*format, *little_endian);

                Ok(self.decode_pcm(format, little_endian, input, output))
            }
            Codec::G711(codec) => codec.decode_into(input, output),
        }
    }
}

impl<S: Sample> Seek for AiffDecoder<S> {
    fn seek(&mut self, frame: u64) -> Result<SeekRequest> {
        if frame > self.frames {
            return Err(Error::InvalidParameter);
        }

        match &mut self.codec {
            Codec::Pcm { format, .. } => Ok(SeekRequest {
                byte_offset: frame * self.channels as u64 * format.sample_size() as u64,
                skip_frames: 0,
            }),
            Codec::G711(codec) => codec.seek(frame),
        }
    }
}
