use core::marker::PhantomData;

use nanoaudio_adpcm::{ImaAdpcm, MsAdpcm};
use nanoaudio_core::codec::{Decoder, Progress, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Format, I24, Result, Sample};
use nanoaudio_g711::{G711, Law};

use crate::{Encoding, WavHeader};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Codec {
    Pcm(Format),
    G711(G711),
    ImaAdpcm(ImaAdpcm),
    MsAdpcm(MsAdpcm),
}

/// A decoder for the sound data of a WAV file, producing interleaved samples of type `S`.
///
/// Feed it the bytes that follow [`WavHeader::data_offset`]. Any [`Sample`] type works as output: PCM is converted from the format in the file, and the compressed encodings are decoded to 16-bit and then converted.
///
/// With [`Seek`], the returned [`SeekRequest::byte_offset`] counts from `data_offset`, not from the start of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavDecoder<S: Sample = i16> {
    codec: Codec,
    sample_rate: u32,
    channels: u8,
    frames: u64,
    sample: PhantomData<S>,
}

impl<S: Sample> WavDecoder<S> {
    /// Creates a decoder for the encoding described by `header`.
    pub fn new(header: &WavHeader) -> Result<Self> {
        let Configuration {
            sample_rate,
            channels,
            format,
        } = header.configuration;

        let codec = match header.encoding {
            Encoding::Pcm => Codec::Pcm(format),
            Encoding::ALaw => Codec::G711(G711::new(Law::A, sample_rate, channels)?),
            Encoding::MuLaw => Codec::G711(G711::new(Law::Mu, sample_rate, channels)?),
            Encoding::ImaAdpcm { block_align } => {
                Codec::ImaAdpcm(ImaAdpcm::new(sample_rate, channels, block_align)?)
            }
            Encoding::MsAdpcm {
                block_align,
                samples_per_block,
                coefficients,
            } => Codec::MsAdpcm(MsAdpcm::with_coefficients(
                sample_rate,
                channels,
                block_align,
                samples_per_block,
                coefficients,
            )?),
        };

        Ok(Self {
            codec,
            sample_rate,
            channels,
            frames: header.frames() as u64,
            sample: PhantomData,
        })
    }

    fn decode_pcm(&self, format: Format, input: &[u8], output: &mut [S]) -> Progress {
        let channels = self.channels as usize;
        let frame_size = channels * format.sample_size();
        let frames = (input.len() / frame_size).min(output.len() / channels);
        let samples = frames * channels;

        for (output, bytes) in output[..samples]
            .iter_mut()
            .zip(input.chunks_exact(format.sample_size()))
        {
            *output = match format {
                Format::U8 => u8::read_le(bytes).convert(),
                Format::I16 => i16::read_le(bytes).convert(),
                Format::I24 => I24::read_le(bytes).convert(),
                Format::I32 => i32::read_le(bytes).convert(),
                Format::F32 => f32::read_le(bytes).convert(),
            };
        }

        Progress::new(frames * frame_size, samples)
    }
}

impl<S: Sample> Decoder for WavDecoder<S> {
    type Sample = S;

    fn configuration(&self) -> Option<Configuration> {
        Some(Configuration::new(
            self.sample_rate,
            self.channels,
            S::FORMAT,
        ))
    }

    fn minimum_output(&self) -> usize {
        match &self.codec {
            Codec::Pcm(_) | Codec::G711(_) => self.channels as usize,
            Codec::ImaAdpcm(codec) => codec.minimum_output(),
            Codec::MsAdpcm(codec) => codec.minimum_output(),
        }
    }

    fn finish(&mut self) {
        if let Codec::ImaAdpcm(codec) = &mut self.codec {
            codec.finish();
        }
    }

    fn decode(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        match &mut self.codec {
            Codec::Pcm(format) => {
                let format = *format;

                Ok(self.decode_pcm(format, input, output))
            }
            Codec::G711(codec) => codec.decode_into(input, output),
            Codec::ImaAdpcm(codec) => codec.decode_into(input, output),
            Codec::MsAdpcm(codec) => codec.decode_into(input, output),
        }
    }
}

impl<S: Sample> Seek for WavDecoder<S> {
    fn seek(&mut self, frame: u64) -> Result<SeekRequest> {
        if frame > self.frames {
            return Err(Error::InvalidParameter);
        }

        match &mut self.codec {
            Codec::Pcm(format) => Ok(SeekRequest {
                byte_offset: frame * self.channels as u64 * format.sample_size() as u64,
                skip_frames: 0,
            }),
            Codec::G711(codec) => codec.seek(frame),
            Codec::ImaAdpcm(codec) => codec.seek(frame),
            Codec::MsAdpcm(codec) => codec.seek(frame),
        }
    }
}
