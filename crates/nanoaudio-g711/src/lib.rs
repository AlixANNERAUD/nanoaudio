#![no_std]
#![doc = include_str!("../README.md")]

use nanoaudio_core::codec::{Decoder, Encoder, Progress, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Format, Result, Sample};

const SIGN: u8 = 0x80;
const QUANTIZATION_MASK: u8 = 0x0F;
const SEGMENT_MASK: u8 = 0x70;
const SEGMENT_SHIFT: u8 = 4;

const MU_LAW_BIAS: i32 = 0x84;
const MU_LAW_CLIP: i32 = 8159;
const MU_LAW_SEGMENT_ENDS: [i32; 8] = [0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF, 0x1FFF];
const A_LAW_SEGMENT_ENDS: [i32; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];

#[inline]
fn segment(value: i32, ends: &[i32; 8]) -> usize {
    ends.iter()
        .position(|end| value <= *end)
        .unwrap_or(ends.len())
}

#[inline]
pub fn encode_mu_law(sample: i16) -> u8 {
    let mut value = sample as i32 >> 2;
    let mask = if value < 0 {
        value = -value;
        0x7F
    } else {
        0xFF
    };

    value = value.min(MU_LAW_CLIP) + (MU_LAW_BIAS >> 2);

    let segment = segment(value, &MU_LAW_SEGMENT_ENDS);

    if segment >= 8 {
        return 0x7F ^ mask;
    }

    let code =
        ((segment as u8) << SEGMENT_SHIFT) | ((value >> (segment + 1)) as u8 & QUANTIZATION_MASK);

    code ^ mask
}

const fn expand_mu_law(code: u8) -> i16 {
    let code = !code;
    let magnitude = ((((code & QUANTIZATION_MASK) as i32) << 3) + MU_LAW_BIAS)
        << ((code & SEGMENT_MASK) >> SEGMENT_SHIFT);

    if code & SIGN != 0 {
        (MU_LAW_BIAS - magnitude) as i16
    } else {
        (magnitude - MU_LAW_BIAS) as i16
    }
}

#[inline]
pub fn encode_a_law(sample: i16) -> u8 {
    let mut value = sample as i32 >> 3;
    let mask = if value >= 0 {
        0xD5
    } else {
        value = -value - 1;
        0x55
    };

    let segment = segment(value, &A_LAW_SEGMENT_ENDS);

    if segment >= 8 {
        return 0x7F ^ mask;
    }

    let shift = if segment < 2 { 1 } else { segment };
    let code = ((segment as u8) << SEGMENT_SHIFT) | ((value >> shift) as u8 & QUANTIZATION_MASK);

    code ^ mask
}

const fn expand_a_law(code: u8) -> i16 {
    let code = code ^ 0x55;
    let segment = (code & SEGMENT_MASK) >> SEGMENT_SHIFT;
    let bias = if segment == 0 { 8 } else { 0x108 };
    let shift = segment.saturating_sub(1);
    let magnitude = ((((code & QUANTIZATION_MASK) as i16) << 4) + bias) << shift;

    if code & SIGN != 0 {
        magnitude
    } else {
        -magnitude
    }
}

const fn table(law: Law) -> [i16; 256] {
    let mut table = [0; 256];
    let mut code = 0;

    while code < table.len() {
        table[code] = match law {
            Law::A => expand_a_law(code as u8),
            Law::Mu => expand_mu_law(code as u8),
        };
        code += 1;
    }

    table
}

const LOOKUP: bool = cfg!(not(any(
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "arm64ec",
    all(target_arch = "arm", target_feature = "neon"),
    all(target_arch = "wasm32", target_feature = "simd128"),
)));

static A_LAW: [i16; 256] = table(Law::A);
static MU_LAW: [i16; 256] = table(Law::Mu);

#[inline]
pub fn decode_a_law(code: u8) -> i16 {
    if LOOKUP {
        A_LAW[code as usize]
    } else {
        expand_a_law(code)
    }
}

#[inline]
pub fn decode_mu_law(code: u8) -> i16 {
    if LOOKUP {
        MU_LAW[code as usize]
    } else {
        expand_mu_law(code)
    }
}

fn decode_with<S: Sample>(input: &[u8], output: &mut [S], decode: impl Fn(u8) -> i16) {
    for (output, input) in output.iter_mut().zip(input) {
        let value = decode(*input);

        *output = if const { matches!(S::FORMAT, Format::I16) } {
            S::read_le(&value.to_le_bytes())
        } else {
            value.convert()
        };
    }
}

fn encode_with(input: &[i16], output: &mut [u8], encode: impl Fn(i16) -> u8) {
    for (output, input) in output.iter_mut().zip(input) {
        *output = encode(*input);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Law {
    A,
    Mu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct G711 {
    law: Law,
    sample_rate: u32,
    channels: u8,
}

impl G711 {
    pub const fn new(law: Law, sample_rate: u32, channels: u8) -> Result<Self> {
        if channels == 0 || sample_rate == 0 {
            return Err(Error::InvalidParameter);
        }

        Ok(Self {
            law,
            sample_rate,
            channels,
        })
    }

    pub const fn law(&self) -> Law {
        self.law
    }

    fn pcm_configuration(&self) -> Configuration {
        Configuration::new(self.sample_rate, self.channels, Format::I16)
    }

    fn whole_samples(&self, count: usize) -> usize {
        count - count % self.channels as usize
    }
}

impl G711 {
    pub fn decode_into<S: Sample>(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        let count = self.whole_samples(input.len().min(output.len()));

        match self.law {
            Law::A => decode_with(input, &mut output[..count], decode_a_law),
            Law::Mu => decode_with(input, &mut output[..count], decode_mu_law),
        }

        Ok(Progress::new(count, count))
    }
}

impl Decoder for G711 {
    type Sample = i16;

    fn configuration(&self) -> Option<Configuration> {
        Some(self.pcm_configuration())
    }

    fn minimum_output(&self) -> usize {
        self.channels as usize
    }

    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
        self.decode_into(input, output)
    }
}

impl Seek for G711 {
    fn seek(&mut self, frame: u64) -> Result<SeekRequest> {
        let byte_offset = frame
            .checked_mul(self.channels as u64)
            .ok_or(Error::InvalidParameter)?;

        Ok(SeekRequest {
            byte_offset,
            skip_frames: 0,
        })
    }
}

impl Encoder for G711 {
    type Sample = i16;

    fn configuration(&self) -> Configuration {
        self.pcm_configuration()
    }

    fn minimum_input(&self) -> usize {
        self.channels as usize
    }

    fn minimum_output(&self) -> usize {
        self.channels as usize
    }

    fn encode(&mut self, input: &[i16], output: &mut [u8]) -> Result<Progress> {
        let count = self.whole_samples(input.len().min(output.len()));

        match self.law {
            Law::A => encode_with(&input[..count], output, encode_a_law),
            Law::Mu => encode_with(&input[..count], output, encode_mu_law),
        }

        Ok(Progress::new(count, count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_expansion() {
        for code in 0..=u8::MAX {
            assert_eq!(A_LAW[code as usize], expand_a_law(code));
            assert_eq!(MU_LAW[code as usize], expand_mu_law(code));
            assert_eq!(decode_a_law(code), expand_a_law(code));
            assert_eq!(decode_mu_law(code), expand_mu_law(code));
        }
    }
}
