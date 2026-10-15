use core::marker::PhantomData;

use crate::{Configuration, Encoding, Format, Result, Sample, WavHeader};

use super::{Decoder, G711, ImaAdpcm, Law, Progress};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Codec {
    Pcm(Format),
    G711(G711),
    ImaAdpcm(ImaAdpcm),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavDecoder<S: Sample = i16> {
    codec: Codec,
    sample_rate: u32,
    channels: u8,
    sample: PhantomData<S>,
}

impl<S: Sample> WavDecoder<S> {
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
        };

        Ok(Self {
            codec,
            sample_rate,
            channels,
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
        }
    }
}
