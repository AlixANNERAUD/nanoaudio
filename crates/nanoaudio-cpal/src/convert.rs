use cpal::SampleFormat;
use nanoaudio_core::{Format, I24, Sample};
use nanoaudio_dsp::{convert_bytes, convert_channels};

pub(crate) const CHUNK: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Packed(Format),
    Padded24,
}

impl Encoding {
    pub(crate) fn from_cpal(format: SampleFormat) -> Option<Self> {
        match format {
            SampleFormat::U8 => Some(Self::Packed(Format::U8)),
            SampleFormat::I16 => Some(Self::Packed(Format::I16)),
            SampleFormat::I24 => Some(Self::Padded24),
            SampleFormat::I32 => Some(Self::Packed(Format::I32)),
            SampleFormat::F32 => Some(Self::Packed(Format::F32)),
            _ => None,
        }
    }

    pub(crate) fn size(self) -> usize {
        match self {
            Self::Packed(format) => format.sample_size(),
            Self::Padded24 => 4,
        }
    }

    pub(crate) fn silence(self) -> u8 {
        match self {
            Self::Packed(Format::U8) => 0x80,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) encoding: Encoding,
    pub(crate) channels: u8,
}

impl Layout {
    pub(crate) fn frame_size(self) -> usize {
        self.encoding.size() * self.channels as usize
    }
}

pub(crate) struct Converter {
    pub(crate) from: Layout,
    pub(crate) to: Layout,
    bytes: [u8; CHUNK * 4],
    floats: [u8; CHUNK * 4],
    first: [f32; CHUNK],
    second: [f32; CHUNK],
}

impl Converter {
    pub(crate) fn new(from: Layout, to: Layout) -> Self {
        Self {
            from,
            to,
            bytes: [0; CHUNK * 4],
            floats: [0; CHUNK * 4],
            first: [0.0; CHUNK],
            second: [0.0; CHUNK],
        }
    }

    pub(crate) fn is_identity(&self) -> bool {
        self.from == self.to
    }

    pub(crate) fn chunk_frames(&self) -> usize {
        CHUNK / self.from.channels.max(self.to.channels) as usize
    }

    pub(crate) fn convert(&mut self, input: &[u8], output: &mut [u8]) {
        if self.from == self.to {
            output.copy_from_slice(input);
            return;
        }

        if self.from.channels == self.to.channels {
            encode(
                input,
                self.from.encoding,
                output,
                self.to.encoding,
                &mut self.bytes,
            );
            return;
        }

        let samples = input.len() / self.from.encoding.size();
        let frames = samples / self.from.channels as usize;
        let floats = &mut self.floats[..samples * 4];

        encode(
            input,
            self.from.encoding,
            floats,
            Encoding::Packed(Format::F32),
            &mut self.bytes,
        );

        for (sample, bytes) in self.first.iter_mut().zip(floats.as_chunks::<4>().0) {
            *sample = f32::from_le_bytes(*bytes);
        }

        let mapped = frames * self.to.channels as usize;

        convert_channels(
            &self.first[..samples],
            self.from.channels,
            &mut self.second[..mapped],
            self.to.channels,
        );

        let floats = &mut self.floats[..mapped * 4];

        for (bytes, sample) in floats.as_chunks_mut::<4>().0.iter_mut().zip(&self.second) {
            *bytes = sample.to_le_bytes();
        }

        encode(
            floats,
            Encoding::Packed(Format::F32),
            output,
            self.to.encoding,
            &mut self.bytes,
        );
    }
}

fn encode(input: &[u8], from: Encoding, output: &mut [u8], to: Encoding, scratch: &mut [u8]) {
    match (from, to) {
        (Encoding::Packed(from), Encoding::Packed(to)) => {
            convert_bytes(input, from, output, to);
        }
        (Encoding::Padded24, Encoding::Padded24) => output.copy_from_slice(input),
        (Encoding::Padded24, Encoding::Packed(to)) => {
            let widened = &mut scratch[..input.len()];

            for (output, input) in widened
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(input.as_chunks::<4>().0)
            {
                *output = I24::new(i32::from_le_bytes(*input)).to_i32().to_le_bytes();
            }

            convert_bytes(widened, Format::I32, output, to);
        }
        (Encoding::Packed(from), Encoding::Padded24) => {
            convert_bytes(input, from, output, Format::I32);

            for sample in output.as_chunks_mut::<4>().0 {
                *sample = I24::from_i32(i32::from_le_bytes(*sample))
                    .value()
                    .to_le_bytes();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(encoding: Encoding, channels: u8) -> Layout {
        Layout { encoding, channels }
    }

    fn convert(from: Layout, to: Layout, input: &[u8]) -> Vec<u8> {
        let mut converter = Box::new(Converter::new(from, to));
        let frames = input.len() / from.frame_size();
        let mut output = vec![0xAA; frames * to.frame_size()];

        converter.convert(input, &mut output);
        output
    }

    fn bytes_i16(samples: &[i16]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    fn bytes_f32(samples: &[f32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    fn bytes_i32(samples: &[i32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn maps_cpal_formats() {
        assert_eq!(
            Encoding::from_cpal(SampleFormat::F32),
            Some(Encoding::Packed(Format::F32))
        );
        assert_eq!(
            Encoding::from_cpal(SampleFormat::I24),
            Some(Encoding::Padded24)
        );
        assert_eq!(Encoding::from_cpal(SampleFormat::F64), None);
        assert_eq!(Encoding::from_cpal(SampleFormat::U16), None);
        assert_eq!(Encoding::Padded24.size(), SampleFormat::I24.sample_size());
    }

    #[test]
    fn identity_copies() {
        let input = bytes_i16(&[1, -2, 3, -4]);
        let stereo = layout(Encoding::Packed(Format::I16), 2);

        assert_eq!(convert(stereo, stereo, &input), input);
    }

    #[test]
    fn converts_format() {
        let output = convert(
            layout(Encoding::Packed(Format::I16), 2),
            layout(Encoding::Packed(Format::F32), 2),
            &bytes_i16(&[i16::MIN, 0, 16_384, -16_384]),
        );

        assert_eq!(output, bytes_f32(&[-1.0, 0.0, 0.5, -0.5]));
    }

    #[test]
    fn converts_to_and_from_padded_24_bit() {
        let padded = convert(
            layout(Encoding::Packed(Format::I16), 1),
            layout(Encoding::Padded24, 1),
            &bytes_i16(&[i16::MIN, 0x1234, -1]),
        );

        assert_eq!(padded, bytes_i32(&[-(1 << 23), 0x12_3400, -0x100]));

        let packed = convert(
            layout(Encoding::Padded24, 1),
            layout(Encoding::Packed(Format::I16), 1),
            &bytes_i32(&[-(1 << 23), 0x12_3400, 0x7FFF_FFFF]),
        );

        assert_eq!(packed, bytes_i16(&[i16::MIN, 0x1234, i16::MAX]));
    }

    #[test]
    fn upmixes_mono() {
        let output = convert(
            layout(Encoding::Packed(Format::I16), 1),
            layout(Encoding::Packed(Format::F32), 2),
            &bytes_i16(&[16_384, -16_384]),
        );

        assert_eq!(output, bytes_f32(&[0.5, 0.5, -0.5, -0.5]));
    }

    #[test]
    fn downmixes_to_mono() {
        let output = convert(
            layout(Encoding::Packed(Format::F32), 2),
            layout(Encoding::Packed(Format::I16), 1),
            &bytes_f32(&[0.5, 0.0, -0.25, -0.25]),
        );

        assert_eq!(output, bytes_i16(&[8_192, -8_192]));
    }

    #[test]
    fn pads_extra_channels_with_silence() {
        let output = convert(
            layout(Encoding::Packed(Format::U8), 2),
            layout(Encoding::Packed(Format::U8), 4),
            &[0x10, 0x20],
        );

        assert_eq!(output, [0x10, 0x20, 0x80, 0x80]);
    }

    #[test]
    fn chunks_fit_the_widest_side() {
        let converter = Box::new(Converter::new(
            layout(Encoding::Packed(Format::I16), 2),
            layout(Encoding::Padded24, 255),
        ));

        assert_eq!(converter.chunk_frames(), CHUNK / 255);
        assert!(converter.chunk_frames() >= 1);
    }
}
