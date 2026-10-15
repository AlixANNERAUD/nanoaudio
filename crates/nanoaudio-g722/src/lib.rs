#![no_std]
#![doc = include_str!("../README.md")]

use nanoaudio_core::codec::{Decoder, Encoder, Progress};
use nanoaudio_core::{Configuration, Format, Result, Sample};

pub const G722_SAMPLE_RATE: u32 = 16_000;
pub const G722_RTP_CLOCK_RATE: u32 = 8_000;

const FILTER: [i32; 24] = [
    3, -11, -11, 53, 12, -156, 32, 362, -210, -805, 951, 3876, 3876, 951, -805, -210, 362, 32,
    -156, 12, 53, -11, -11, 3,
];

const LOW_DECISION_LEVELS: [i32; 30] = [
    0, 35, 72, 110, 150, 190, 233, 276, 323, 370, 422, 473, 530, 587, 650, 714, 786, 858, 940,
    1023, 1121, 1219, 1339, 1458, 1612, 1765, 1980, 2195, 2557, 2919,
];

const LOW_LEVELS_6: [i32; 64] = [
    -136, -136, -136, -136, -24808, -21904, -19008, -16704, -14984, -13512, -12280, -11192, -10232,
    -9360, -8576, -7856, -7192, -6576, -6000, -5456, -4944, -4464, -4008, -3576, -3168, -2776,
    -2400, -2032, -1688, -1360, -1040, -728, 24808, 21904, 19008, 16704, 14984, 13512, 12280,
    11192, 10232, 9360, 8576, 7856, 7192, 6576, 6000, 5456, 4944, 4464, 4008, 3576, 3168, 2776,
    2400, 2032, 1688, 1360, 1040, 728, 432, 136, -432, -136,
];

const LOW_LEVELS_5: [i32; 32] = [
    -280, -280, -23352, -17560, -14120, -11664, -9752, -8184, -6864, -5712, -4696, -3784, -2960,
    -2208, -1520, -880, 23352, 17560, 14120, 11664, 9752, 8184, 6864, 5712, 4696, 3784, 2960, 2208,
    1520, 880, 280, -280,
];

const LOW_LEVELS_4: [i32; 16] = [
    0, -20456, -12896, -8968, -6288, -4240, -2584, -1200, 20456, 12896, 8968, 6288, 4240, 2584,
    1200, 0,
];

const LOW_MAGNITUDES: [usize; 16] = [0, 7, 6, 5, 4, 3, 2, 1, 7, 6, 5, 4, 3, 2, 1, 0];
const LOW_LOGARITHMIC_STEPS: [i32; 8] = [-60, -30, 58, 172, 334, 538, 1198, 3042];

const HIGH_DECISION_LEVEL: i32 = 564;
const HIGH_LEVELS: [i32; 4] = [-7408, -1616, 7408, 1616];
const HIGH_MAGNITUDES: [usize; 4] = [2, 1, 2, 1];
const HIGH_LOGARITHMIC_STEPS: [i32; 3] = [0, -214, 798];

const INVERSE_LOGARITHM: [i32; 32] = [
    2048, 2093, 2139, 2186, 2233, 2282, 2332, 2383, 2435, 2489, 2543, 2599, 2656, 2714, 2774, 2834,
    2896, 2960, 3025, 3091, 3158, 3228, 3298, 3371, 3444, 3520, 3597, 3676, 3756, 3838, 3922, 4008,
];

const LOW_MAXIMUM_LOGARITHMIC_SCALE: i32 = 18_432;
const HIGH_MAXIMUM_LOGARITHMIC_SCALE: i32 = 22_528;
const LOW_MINIMUM_SCALE: i32 = 32;
const HIGH_MINIMUM_SCALE: i32 = 8;
const ZEROS: usize = 6;

fn saturate(value: i32) -> i32 {
    value.clamp(i16::MIN as i32, i16::MAX as i32)
}

fn negative(value: i32) -> bool {
    value < 0
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum G722BitRate {
    #[default]
    Rate64000,
    Rate56000,
    Rate48000,
}

impl G722BitRate {
    pub const fn bits_per_second(self) -> u32 {
        match self {
            G722BitRate::Rate64000 => 64_000,
            G722BitRate::Rate56000 => 56_000,
            G722BitRate::Rate48000 => 48_000,
        }
    }

    pub const fn mode(self) -> u8 {
        match self {
            G722BitRate::Rate64000 => 1,
            G722BitRate::Rate56000 => 2,
            G722BitRate::Rate48000 => 3,
        }
    }

    fn low_difference(self, code: u8, scale: i32) -> i32 {
        let level = match self {
            G722BitRate::Rate64000 => LOW_LEVELS_6[code as usize],
            G722BitRate::Rate56000 => LOW_LEVELS_5[code as usize >> 1],
            G722BitRate::Rate48000 => LOW_LEVELS_4[code as usize >> 2],
        };

        (scale * level) >> 15
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Band {
    estimate: i32,
    zero_estimate: i32,
    reconstructed: [i32; 2],
    partial: [i32; 2],
    poles: [i32; 2],
    differences: [i32; ZEROS],
    zeros: [i32; ZEROS],
    logarithmic_scale: i32,
    scale: i32,
}

impl Band {
    const fn new(scale: i32) -> Self {
        Self {
            estimate: 0,
            zero_estimate: 0,
            reconstructed: [0; 2],
            partial: [0; 2],
            poles: [0; 2],
            differences: [0; ZEROS],
            zeros: [0; ZEROS],
            logarithmic_scale: 0,
            scale,
        }
    }

    fn update_scale(&mut self, step: i32, maximum: i32, shift: i32) {
        let leaked = (self.logarithmic_scale * 127) >> 7;

        self.logarithmic_scale = (leaked + step).clamp(0, maximum);

        let mantissa = INVERSE_LOGARITHM[((self.logarithmic_scale >> 6) & 31) as usize];
        let exponent = shift - (self.logarithmic_scale >> 11);
        let scale = if exponent < 0 {
            mantissa << -exponent
        } else {
            mantissa >> exponent
        };

        self.scale = scale << 2;
    }

    fn update_low_scale(&mut self, code: u8) {
        self.update_scale(
            LOW_LOGARITHMIC_STEPS[LOW_MAGNITUDES[code as usize]],
            LOW_MAXIMUM_LOGARITHMIC_SCALE,
            8,
        );
    }

    fn update_high_scale(&mut self, code: u8) {
        self.update_scale(
            HIGH_LOGARITHMIC_STEPS[HIGH_MAGNITUDES[code as usize]],
            HIGH_MAXIMUM_LOGARITHMIC_SCALE,
            10,
        );
    }

    fn adapt(&mut self, difference: i32) {
        let reconstructed = saturate(self.estimate + difference);
        let partial = saturate(self.zero_estimate + difference);

        let partial_sign = negative(partial);
        let same_first = partial_sign == negative(self.partial[0]);
        let same_second = partial_sign == negative(self.partial[1]);

        let scaled = saturate(self.poles[0] << 2);
        let gradient = if same_first {
            (-scaled).min(i16::MAX as i32)
        } else {
            scaled
        };
        let second = ((gradient >> 7)
            + if same_second { 128 } else { -128 }
            + ((self.poles[1] * 32_512) >> 15))
            .clamp(-12_288, 12_288);

        let limit = saturate(15_360 - second);
        let first =
            saturate(if same_first { 192 } else { -192 } + ((self.poles[0] * 32_640) >> 15))
                .clamp(-limit, limit);

        let step = if difference == 0 { 0 } else { 128 };
        let difference_sign = negative(difference);

        for (zero, previous) in self.zeros.iter_mut().zip(&self.differences) {
            let correction = if negative(*previous) == difference_sign {
                step
            } else {
                -step
            };

            *zero = saturate(correction + ((*zero * 32_640) >> 15));
        }

        self.differences.copy_within(..ZEROS - 1, 1);
        self.differences[0] = difference;
        self.reconstructed = [reconstructed, self.reconstructed[0]];
        self.partial = [partial, self.partial[0]];
        self.poles = [first, second];

        let pole_estimate = saturate(
            self.poles
                .iter()
                .zip(&self.reconstructed)
                .map(|(pole, reconstructed)| (pole * saturate(reconstructed * 2)) >> 15)
                .sum(),
        );

        self.zero_estimate = self.zeros.iter().zip(&self.differences).rev().fold(
            0,
            |estimate, (zero, difference)| {
                saturate(estimate + ((zero * saturate(difference * 2)) >> 15))
            },
        );

        self.estimate = saturate(pole_estimate + self.zero_estimate);
    }
}

fn configuration() -> Configuration {
    Configuration::new(G722_SAMPLE_RATE, 1, Format::I16)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct G722Encoder {
    history: [i32; 24],
    low: Band,
    high: Band,
}

impl Default for G722Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl G722Encoder {
    pub const fn new() -> Self {
        Self {
            history: [0; 24],
            low: Band::new(LOW_MINIMUM_SCALE),
            high: Band::new(HIGH_MINIMUM_SCALE),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn encode_pair(&mut self, first: i16, second: i16) -> u8 {
        self.history.copy_within(2.., 0);
        self.history[22] = first as i32;
        self.history[23] = second as i32;

        let (mut even, mut odd) = (0, 0);

        for (age, coefficient) in FILTER.iter().enumerate() {
            let product = coefficient * self.history[23 - age];

            if age % 2 == 0 {
                even += product;
            } else {
                odd += product;
            }
        }

        let low = self.encode_low((even + odd) >> 14);
        let high = self.encode_high((even - odd) >> 14);

        (high << 6) | low
    }

    fn encode_low(&mut self, sample: i32) -> u8 {
        let band = &mut self.low;
        let error = saturate(sample - band.estimate);
        let magnitude = if error >= 0 { error } else { -(error + 1) };
        let interval = (1..LOW_DECISION_LEVELS.len())
            .find(|interval| magnitude < (LOW_DECISION_LEVELS[*interval] * band.scale) >> 12)
            .unwrap_or(LOW_DECISION_LEVELS.len()) as u8;

        let code = match (error < 0, interval) {
            (false, interval) => 62 - interval,
            (true, 1) => 63,
            (true, 2) => 62,
            (true, interval) => 34 - interval,
        };

        let truncated = code >> 2;
        let difference = (band.scale * LOW_LEVELS_4[truncated as usize]) >> 15;

        band.update_low_scale(truncated);
        band.adapt(difference);

        code
    }

    fn encode_high(&mut self, sample: i32) -> u8 {
        let band = &mut self.high;
        let error = saturate(sample - band.estimate);
        let magnitude = if error >= 0 { error } else { -(error + 1) };
        let outer = magnitude >= (HIGH_DECISION_LEVEL * band.scale) >> 12;

        let code = match (error < 0, outer) {
            (false, false) => 3,
            (false, true) => 2,
            (true, false) => 1,
            (true, true) => 0,
        };

        let difference = (band.scale * HIGH_LEVELS[code as usize]) >> 15;

        band.update_high_scale(code);
        band.adapt(difference);

        code
    }
}

impl Encoder for G722Encoder {
    type Sample = i16;

    fn configuration(&self) -> Configuration {
        configuration()
    }

    fn minimum_input(&self) -> usize {
        2
    }

    fn minimum_output(&self) -> usize {
        1
    }

    fn encode(&mut self, input: &[i16], output: &mut [u8]) -> Result<Progress> {
        let (pairs, _) = input.as_chunks::<2>();
        let count = pairs.len().min(output.len());

        for (output, pair) in output.iter_mut().zip(&pairs[..count]) {
            *output = self.encode_pair(pair[0], pair[1]);
        }

        Ok(Progress::new(count * 2, count))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct G722Decoder {
    bit_rate: G722BitRate,
    sums: [i32; 12],
    differences: [i32; 12],
    low: Band,
    high: Band,
}

impl Default for G722Decoder {
    fn default() -> Self {
        Self::new(G722BitRate::Rate64000)
    }
}

impl G722Decoder {
    pub const fn new(bit_rate: G722BitRate) -> Self {
        Self {
            bit_rate,
            sums: [0; 12],
            differences: [0; 12],
            low: Band::new(LOW_MINIMUM_SCALE),
            high: Band::new(HIGH_MINIMUM_SCALE),
        }
    }

    pub const fn bit_rate(&self) -> G722BitRate {
        self.bit_rate
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.bit_rate);
    }

    pub fn decode_byte(&mut self, byte: u8) -> [i16; 2] {
        let low = self.decode_low(byte & 0x3F);
        let high = self.decode_high(byte >> 6);

        self.sums.copy_within(1.., 0);
        self.differences.copy_within(1.., 0);
        self.sums[11] = low + high;
        self.differences[11] = low - high;

        let (mut first, mut second) = (0, 0);

        for age in 0..12 {
            first += self.differences[11 - age] * FILTER[2 * age];
            second += self.sums[11 - age] * FILTER[2 * age + 1];
        }

        [saturate(first >> 11) as i16, saturate(second >> 11) as i16]
    }

    fn decode_low(&mut self, code: u8) -> i32 {
        let band = &mut self.low;
        let truncated = code >> 2;
        let output =
            (band.estimate + self.bit_rate.low_difference(code, band.scale)).clamp(-16_384, 16_383);
        let difference = (band.scale * LOW_LEVELS_4[truncated as usize]) >> 15;

        band.update_low_scale(truncated);
        band.adapt(difference);

        output
    }

    fn decode_high(&mut self, code: u8) -> i32 {
        let band = &mut self.high;
        let difference = (band.scale * HIGH_LEVELS[code as usize]) >> 15;
        let output = (band.estimate + difference).clamp(-16_384, 16_383);

        band.update_high_scale(code);
        band.adapt(difference);

        output
    }

    pub fn decode_into<S: Sample>(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        let (pairs, _) = output.as_chunks_mut::<2>();
        let count = pairs.len().min(input.len());

        for (pair, byte) in pairs.iter_mut().zip(&input[..count]) {
            let [first, second] = self.decode_byte(*byte);

            *pair = [first.convert(), second.convert()];
        }

        Ok(Progress::new(count, count * 2))
    }
}

impl Decoder for G722Decoder {
    type Sample = i16;

    fn configuration(&self) -> Option<Configuration> {
        Some(configuration())
    }

    fn minimum_output(&self) -> usize {
        2
    }

    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
        self.decode_into(input, output)
    }
}
