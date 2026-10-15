use crate::{Configuration, Error, Format, Result};

use super::{Decoder, Encoder, Progress};

const SIGN: u8 = 0x80;
const QUANTIZATION_MASK: u8 = 0x0F;
const SEGMENT_MASK: u8 = 0x70;
const SEGMENT_SHIFT: u8 = 4;

const MU_LAW_BIAS: i32 = 0x84;
const MU_LAW_CLIP: i32 = 8159;
const MU_LAW_SEGMENT_ENDS: [i32; 8] = [0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF, 0x1FFF];
const A_LAW_SEGMENT_ENDS: [i32; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];

fn segment(value: i32, ends: &[i32; 8]) -> usize {
    ends.iter()
        .position(|end| value <= *end)
        .unwrap_or(ends.len())
}

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

pub fn decode_mu_law(code: u8) -> i16 {
    let code = !code;
    let magnitude = ((((code & QUANTIZATION_MASK) as i32) << 3) + MU_LAW_BIAS)
        << ((code & SEGMENT_MASK) >> SEGMENT_SHIFT);

    if code & SIGN != 0 {
        (MU_LAW_BIAS - magnitude) as i16
    } else {
        (magnitude - MU_LAW_BIAS) as i16
    }
}

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

pub fn decode_a_law(code: u8) -> i16 {
    let code = code ^ 0x55;
    let mut magnitude = ((code & QUANTIZATION_MASK) as i32) << 4;

    match (code & SEGMENT_MASK) >> SEGMENT_SHIFT {
        0 => magnitude += 8,
        1 => magnitude += 0x108,
        segment => magnitude = (magnitude + 0x108) << (segment - 1),
    }

    if code & SIGN != 0 {
        magnitude as i16
    } else {
        -magnitude as i16
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

impl Decoder for G711 {
    type Sample = i16;

    fn configuration(&self) -> Option<Configuration> {
        Some(self.pcm_configuration())
    }

    fn minimum_output(&self) -> usize {
        self.channels as usize
    }

    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
        let count = self.whole_samples(input.len().min(output.len()));
        let decode = match self.law {
            Law::A => decode_a_law,
            Law::Mu => decode_mu_law,
        };

        for (output, input) in output[..count].iter_mut().zip(input) {
            *output = decode(*input);
        }

        Ok(Progress::new(count, count))
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
        let encode = match self.law {
            Law::A => encode_a_law,
            Law::Mu => encode_mu_law,
        };

        for (output, input) in output[..count].iter_mut().zip(input) {
            *output = encode(*input);
        }

        Ok(Progress::new(count, count))
    }
}
