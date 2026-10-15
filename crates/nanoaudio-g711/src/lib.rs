#![no_std]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

use nanoaudio_core::codec::{Decoder, Encoder, Progress, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Format, Result, Sample};

const SIGN: u8 = 0x80;
const QUANTIZATION_MASK: u8 = 0x0F;
const SEGMENT_MASK: u8 = 0x70;
const SEGMENT_SHIFT: u8 = 4;

const MU_LAW_BIAS: i32 = 0x84;
const MU_LAW_MAXIMUM: i32 = 0x1FFF;

const FLOAT: bool = cfg!(any(
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "arm64ec",
    all(target_arch = "arm", target_feature = "neon"),
    target_arch = "wasm32",
    target_arch = "wasm64",
));

const LEADING_ZEROS: bool = cfg!(any(
    all(target_arch = "arm", target_has_atomic = "32"),
    all(
        any(target_arch = "riscv32", target_arch = "riscv64"),
        target_feature = "zbb"
    ),
    target_arch = "loongarch64",
    target_arch = "mips",
    target_arch = "mips64",
    target_arch = "powerpc",
    target_arch = "powerpc64",
    target_arch = "s390x",
));

static LENGTHS: [u8; 256] = {
    let mut table = [0; 256];
    let mut index = 0;

    while index < table.len() {
        table[index] = (u32::BITS - (index as u32).leading_zeros()) as u8;
        index += 1;
    }

    table
};

#[inline]
fn compress_float(value: u32, offset: u32) -> u32 {
    ((value as f32).to_bits() >> 19) - ((126 + offset) << SEGMENT_SHIFT)
}

#[inline]
fn compress_length(value: u32, offset: u32, length: u32) -> u32 {
    ((length - offset) << SEGMENT_SHIFT) | ((value >> (length - 5)) & QUANTIZATION_MASK as u32)
}

#[inline]
fn compress_leading_zeros(value: u32, offset: u32) -> u32 {
    compress_length(value, offset, u32::BITS - value.leading_zeros())
}

#[inline]
fn compress_lookup(value: u32, offset: u32) -> u32 {
    compress_length(
        value,
        offset,
        LENGTHS[(value >> 5) as u8 as usize] as u32 + 5,
    )
}

#[inline]
fn compress(value: u32, offset: u32) -> u32 {
    if FLOAT {
        compress_float(value, offset)
    } else if LEADING_ZEROS {
        compress_leading_zeros(value, offset)
    } else {
        compress_lookup(value, offset)
    }
}

/// Encodes a 16-bit sample to a µ-law code.
///
/// Only the top 14 bits of the sample are used, as in G.711.
#[inline]
pub fn encode_mu_law(sample: i16) -> u8 {
    encode_mu_law_with(sample, compress)
}

#[inline]
fn encode_mu_law_with(sample: i16, compress: impl Fn(u32, u32) -> u32) -> u8 {
    let sign = sample as i32 >> 15;
    let magnitude = ((sample as i32 >> 2) ^ sign) - sign;
    let value = (magnitude + (MU_LAW_BIAS >> 2)).min(MU_LAW_MAXIMUM) as u32;
    let mask = 0xFF ^ (sign as u32 & SIGN as u32);

    (compress(value, 6) ^ mask) as u8
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

/// Encodes a 16-bit sample to an A-law code.
///
/// Only the top 13 bits of the sample are used, as in G.711.
#[inline]
pub fn encode_a_law(sample: i16) -> u8 {
    encode_a_law_with(sample, compress)
}

#[inline]
fn encode_a_law_with(sample: i16, compress: impl Fn(u32, u32) -> u32) -> u8 {
    let sign = sample as i32 >> 15;
    let value = ((sample as i32 >> 3) ^ sign) as u32;
    let small = (value < 32) as u32;
    let mask = 0xD5 ^ (sign as u32 & SIGN as u32);

    ((compress(value | (small << 5), 5) - (small << SEGMENT_SHIFT)) ^ mask) as u8
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

const FLOAT_EXPANSION: bool = cfg!(target_arch = "wasm32");

#[inline]
fn expand_float(code: u32) -> i32 {
    f32::from_bits((code << 19) + ((134 << 23) | (1 << 18))) as i32
}

#[inline]
fn expand_mu_law_float(code: u8) -> i16 {
    let code = !code as u32;
    let sign = (code as i32) << 24 >> 31;
    let magnitude = expand_float(code & 0x7F) - MU_LAW_BIAS;

    ((magnitude ^ sign) - sign) as i16
}

#[inline]
fn expand_a_law_float(code: u8) -> i16 {
    let code = (code ^ 0x55) as u32;
    let sign = !((code as i32) << 24 >> 31);
    let first = ((code & SEGMENT_MASK as u32) == 0) as u32;
    let magnitude = expand_float((code & 0x7F) | (first << SEGMENT_SHIFT)) - (first << 8) as i32;

    ((magnitude ^ sign) - sign) as i16
}

static A_LAW: [i16; 256] = table(Law::A);
static MU_LAW: [i16; 256] = table(Law::Mu);

/// Decodes an A-law code to a 16-bit sample.
#[inline]
pub fn decode_a_law(code: u8) -> i16 {
    if LOOKUP {
        A_LAW[code as usize]
    } else if FLOAT_EXPANSION {
        expand_a_law_float(code)
    } else {
        expand_a_law(code)
    }
}

/// Decodes a µ-law code to a 16-bit sample.
#[inline]
pub fn decode_mu_law(code: u8) -> i16 {
    if LOOKUP {
        MU_LAW[code as usize]
    } else if FLOAT_EXPANSION {
        expand_mu_law_float(code)
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

/// A G.711 companding law.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Law {
    /// A-law, used in Europe and most countries outside North America and Japan.
    A,
    /// µ-law, used in North America and Japan.
    Mu,
}

/// A G.711 encoder and decoder, one byte per sample.
///
/// Any sample rate and channel count are accepted: G.711 stores neither, so they come from the container or the caller, usually 8 kHz mono for telephony. Samples are interleaved, and each call processes whole frames only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct G711 {
    law: Law,
    sample_rate: u32,
    channels: u8,
}

impl G711 {
    /// Creates a codec for `law`, at `sample_rate` Hz with `channels` interleaved channels.
    ///
    /// Returns [`Error::InvalidParameter`] if `sample_rate` or `channels` is zero.
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

    /// Returns the companding law.
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
    /// Like [`Decoder::decode`], but writes any [`Sample`] format.
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

    /// One frame: one sample per channel.
    fn minimum_output(&self) -> usize {
        self.channels as usize
    }

    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
        self.decode_into(input, output)
    }
}

impl Seek for G711 {
    /// Seeks to `frame` directly, as every byte decodes on its own: the byte offset is `frame` times the channel count, from the start of the encoded data, and nothing is skipped.
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

    /// One frame: one sample per channel.
    fn minimum_input(&self) -> usize {
        self.channels as usize
    }

    /// One byte per channel.
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

    const MU_LAW_CLIP: i32 = 8159;
    const MU_LAW_SEGMENT_ENDS: [i32; 8] = [0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF, 0x1FFF];
    const A_LAW_SEGMENT_ENDS: [i32; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];

    fn segment(value: i32, ends: &[i32; 8]) -> usize {
        ends.iter()
            .position(|end| value <= *end)
            .unwrap_or(ends.len())
    }

    fn reference_mu_law(sample: i16) -> u8 {
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

        let code = ((segment as u8) << SEGMENT_SHIFT)
            | ((value >> (segment + 1)) as u8 & QUANTIZATION_MASK);

        code ^ mask
    }

    fn reference_a_law(sample: i16) -> u8 {
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
        let code =
            ((segment as u8) << SEGMENT_SHIFT) | ((value >> shift) as u8 & QUANTIZATION_MASK);

        code ^ mask
    }

    #[test]
    fn encoders_match_reference() {
        let step = if cfg!(miri) { 61 } else { 1 };

        for sample in (i16::MIN..=i16::MAX).step_by(step) {
            let mu_law = reference_mu_law(sample);
            let a_law = reference_a_law(sample);

            assert_eq!(encode_mu_law(sample), mu_law, "{sample}");
            assert_eq!(encode_a_law(sample), a_law, "{sample}");

            for compress in [compress_float, compress_leading_zeros, compress_lookup] {
                assert_eq!(encode_mu_law_with(sample, compress), mu_law, "{sample}");
                assert_eq!(encode_a_law_with(sample, compress), a_law, "{sample}");
            }
        }
    }

    #[test]
    fn tables_match_expansion() {
        for code in 0..=u8::MAX {
            assert_eq!(A_LAW[code as usize], expand_a_law(code));
            assert_eq!(MU_LAW[code as usize], expand_mu_law(code));
            assert_eq!(decode_a_law(code), expand_a_law(code));
            assert_eq!(decode_mu_law(code), expand_mu_law(code));
            assert_eq!(expand_a_law_float(code), expand_a_law(code));
            assert_eq!(expand_mu_law_float(code), expand_mu_law(code));
        }
    }
}
