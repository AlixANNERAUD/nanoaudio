#![no_std]
#![doc = include_str!("../README.md")]

use nanoaudio_core::bits::BitReader;
use nanoaudio_core::codec::{Decoder, Encoder, Progress};
use nanoaudio_core::crc;
use nanoaudio_core::{Configuration, Error, Format, Result, Sample};

const SYNCHRONIZATION: u8 = 0x9C;
const MSBC_SYNCHRONIZATION: u8 = 0xAD;
const HEADER_SIZE: usize = 4;
const MAXIMUM_SUBBANDS: usize = 8;
const MAXIMUM_CHANNELS: usize = 2;
const MAXIMUM_BLOCKS: usize = 16;
const MSBC_BLOCKS: u8 = 15;
const WINDOW: usize = 10 * MAXIMUM_SUBBANDS;
const SYNTHESIS: usize = 20 * MAXIMUM_SUBBANDS;
const SUBBAND_FRACTION: u32 = 10;
const COSINE_FRACTION: u32 = 30;
const PROTOTYPE_FRACTION: u32 = 30;
const ANALYSIS_FRACTION: u32 = 12;
const PARTIAL_FRACTION: u32 = 15;

pub const SBC_MAXIMUM_FRAME_LENGTH: usize = 524;
pub const SBC_MAXIMUM_FRAME_SAMPLES: usize = MAXIMUM_BLOCKS * MAXIMUM_SUBBANDS * MAXIMUM_CHANNELS;

type Crc8 = crc::Crc8<0x1D>;

const SAMPLE_RATES: [u32; 4] = [16_000, 32_000, 44_100, 48_000];

const LOUDNESS_OFFSETS_4: [[i32; 4]; 4] =
    [[-1, 0, 0, 0], [-2, 0, 0, 1], [-2, 0, 0, 1], [-2, 0, 0, 1]];

const LOUDNESS_OFFSETS_8: [[i32; 8]; 4] = [
    [-2, 0, 0, 0, 0, 0, 0, 1],
    [-3, 0, 0, 0, 0, 0, 1, 2],
    [-4, 0, 0, 0, 0, 0, 1, 2],
    [-4, 0, 0, 0, 0, 0, 1, 2],
];

const PROTOTYPE_4: [i32; 40] = [
    0, 576115, 1601898, 2935298, 4120164, 4179058, 2003406, -3285782, 11718563, 21945682, 31005089,
    34567968, 27784982, 6584670, -30947094, -83372133, 145592169, 209366600, 264824099, 302610729,
    316018681, 302610729, 264824099, 209366600, -145592169, -83372133, -30947094, 6584670,
    27784982, 34567968, 31005089, 21945682, -11718563, -3285782, 2003406, 4179058, 4120164,
    2935298, 1601898, 576115,
];

const PROTOTYPE_8: [i32; 80] = [
    0, 168122, 368569, 595519, 884677, 1223985, 1585274, 1915252, 2160181, 2258852, 2141627,
    1735771, 968681, -191991, -1771385, -3755063, 6076836, 8621515, 11229669, 13687238, 15733030,
    17077392, 17417002, 16448018, 13891192, 9510749, 3139712, -5278279, -15720018, -28035265,
    -41956610, -57109432, 73013309, 89104205, 104770779, 119396536, 132354301, 143091576,
    151132925, 156111160, 157791803, 156111160, 151132925, 143091576, 132354301, 119396536,
    104770779, 89104205, -73013309, -57109432, -41956610, -28035265, -15720018, -5278279, 3139712,
    9510749, 13891192, 16448018, 17417002, 17077392, 15733030, 13687238, 11229669, 8621515,
    -6076836, -3755063, -1771385, -191991, 968681, 1735771, 2141627, 2258852, 2160181, 1915252,
    1585274, 1223985, 884677, 595519, 368569, 168122,
];

const COSINE: [i32; 32] = [
    1073741824,
    1053110176,
    992008094,
    892783698,
    759250125,
    596538995,
    410903207,
    209476638,
    0,
    -209476638,
    -410903207,
    -596538995,
    -759250125,
    -892783698,
    -992008094,
    -1053110176,
    -1073741824,
    -1053110176,
    -992008094,
    -892783698,
    -759250125,
    -596538995,
    -410903207,
    -209476638,
    0,
    209476638,
    410903207,
    596538995,
    759250125,
    892783698,
    992008094,
    1053110176,
];

const fn modulation<const SIZE: usize>(subbands: usize, analysis: bool) -> [i32; SIZE] {
    let mut matrix = [0; SIZE];
    let mut index = 0;

    while index < SIZE {
        let (subband, column) = if analysis {
            (
                index / (2 * subbands),
                (index % (2 * subbands)) as isize - subbands as isize,
            )
        } else {
            (index % subbands, (index / subbands) as isize)
        };
        let phase = ((2 * subband as isize + 1) * (2 * column + subbands as isize) * 4)
            >> subbands.trailing_zeros();

        matrix[index] = COSINE[(phase & 31) as usize];
        index += 1;
    }

    matrix
}

const SYNTHESIS_4: [i32; 32] = modulation(4, false);
const SYNTHESIS_8: [i32; 128] = modulation(8, false);
const ANALYSIS_4: [i32; 32] = modulation(4, true);
const ANALYSIS_8: [i32; 128] = modulation(8, true);

struct Filterbank {
    prototype: &'static [i32],
    synthesis: &'static [i32],
    analysis: &'static [i32],
}

impl Filterbank {
    const fn new(subbands: usize) -> Self {
        if subbands == 4 {
            Self {
                prototype: &PROTOTYPE_4,
                synthesis: &SYNTHESIS_4,
                analysis: &ANALYSIS_4,
            }
        } else {
            Self {
                prototype: &PROTOTYPE_8,
                synthesis: &SYNTHESIS_8,
                analysis: &ANALYSIS_8,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SbcChannelMode {
    Mono,
    DualChannel,
    Stereo,
    #[default]
    JointStereo,
}

impl SbcChannelMode {
    pub const fn channels(self) -> u8 {
        match self {
            SbcChannelMode::Mono => 1,
            _ => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SbcAllocation {
    #[default]
    Loudness,
    Snr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SbcParameters {
    pub sample_rate: u32,
    pub channel_mode: SbcChannelMode,
    pub allocation: SbcAllocation,
    pub blocks: u8,
    pub subbands: u8,
    pub bitpool: u8,
}

impl Default for SbcParameters {
    fn default() -> Self {
        Self::HIGH_QUALITY
    }
}

impl SbcParameters {
    pub const HIGH_QUALITY: Self = Self {
        sample_rate: 44_100,
        channel_mode: SbcChannelMode::JointStereo,
        allocation: SbcAllocation::Loudness,
        blocks: 16,
        subbands: 8,
        bitpool: 53,
    };

    pub const MSBC: Self = Self {
        sample_rate: 16_000,
        channel_mode: SbcChannelMode::Mono,
        allocation: SbcAllocation::Loudness,
        blocks: MSBC_BLOCKS,
        subbands: 8,
        bitpool: 26,
    };

    pub fn parse(header: &[u8]) -> Result<Self> {
        let [synchronization, settings, bitpool, ..] = *header else {
            return Err(Error::Incomplete);
        };

        if synchronization == MSBC_SYNCHRONIZATION {
            return Ok(Self::MSBC);
        }

        if synchronization != SYNCHRONIZATION {
            return Err(Error::InvalidHeader);
        }

        let parameters = Self {
            sample_rate: SAMPLE_RATES[(settings >> 6) as usize],
            channel_mode: match (settings >> 2) & 3 {
                0 => SbcChannelMode::Mono,
                1 => SbcChannelMode::DualChannel,
                2 => SbcChannelMode::Stereo,
                _ => SbcChannelMode::JointStereo,
            },
            allocation: if settings & 2 == 0 {
                SbcAllocation::Loudness
            } else {
                SbcAllocation::Snr
            },
            blocks: 4 * (((settings >> 4) & 3) + 1),
            subbands: if settings & 1 == 0 { 4 } else { 8 },
            bitpool,
        };

        parameters.validate().map_err(|_| Error::InvalidHeader)?;

        Ok(parameters)
    }

    pub fn validate(&self) -> Result<()> {
        if self.is_msbc() {
            return if *self == Self::MSBC {
                Ok(())
            } else {
                Err(Error::InvalidParameter)
            };
        }

        let valid = SAMPLE_RATES.contains(&self.sample_rate)
            && matches!(self.blocks, 4 | 8 | 12 | 16)
            && matches!(self.subbands, 4 | 8)
            && self.bitpool >= 2
            && self.bitpool as usize <= self.maximum_bitpool();

        if valid {
            Ok(())
        } else {
            Err(Error::InvalidParameter)
        }
    }

    pub const fn is_msbc(&self) -> bool {
        self.blocks == MSBC_BLOCKS
    }

    pub const fn channels(&self) -> u8 {
        self.channel_mode.channels()
    }

    pub const fn maximum_bitpool(&self) -> usize {
        match self.channel_mode {
            SbcChannelMode::Mono | SbcChannelMode::DualChannel => 16 * self.subbands as usize,
            SbcChannelMode::Stereo | SbcChannelMode::JointStereo => 32 * self.subbands as usize,
        }
    }

    pub const fn frame_length(&self) -> usize {
        let channels = self.channels() as usize;
        let blocks = self.blocks as usize;
        let subbands = self.subbands as usize;
        let bitpool = self.bitpool as usize;
        let audio = match self.channel_mode {
            SbcChannelMode::Mono | SbcChannelMode::DualChannel => blocks * channels * bitpool,
            SbcChannelMode::Stereo => blocks * bitpool,
            SbcChannelMode::JointStereo => subbands + blocks * bitpool,
        };

        HEADER_SIZE + 4 * subbands * channels / 8 + audio.div_ceil(8)
    }

    pub const fn samples_per_channel(&self) -> usize {
        self.blocks as usize * self.subbands as usize
    }

    pub const fn frame_samples(&self) -> usize {
        self.samples_per_channel() * self.channels() as usize
    }

    pub fn configuration(&self) -> Configuration {
        Configuration::new(self.sample_rate, self.channels(), Format::I16)
    }

    fn frequency_index(&self) -> usize {
        SAMPLE_RATES
            .iter()
            .position(|rate| *rate == self.sample_rate)
            .unwrap_or(0)
    }

    fn header(&self) -> [u8; 3] {
        if self.is_msbc() {
            return [MSBC_SYNCHRONIZATION, 0, 0];
        }

        let mode = match self.channel_mode {
            SbcChannelMode::Mono => 0,
            SbcChannelMode::DualChannel => 1,
            SbcChannelMode::Stereo => 2,
            SbcChannelMode::JointStereo => 3,
        };
        let settings = (self.frequency_index() as u8) << 6
            | (self.blocks / 4 - 1) << 4
            | mode << 2
            | (self.allocation as u8) << 1
            | (self.subbands == 8) as u8;

        [SYNCHRONIZATION, settings, self.bitpool]
    }

    fn side_bits(&self) -> usize {
        let join = if self.channel_mode == SbcChannelMode::JointStereo {
            self.subbands as usize
        } else {
            0
        };

        join + 4 * self.subbands as usize * self.channels() as usize
    }
}

type SubbandValues = [[u8; MAXIMUM_SUBBANDS]; MAXIMUM_CHANNELS];

fn bit_need(parameters: &SbcParameters, scale_factor: u8, subband: usize) -> i32 {
    let scale_factor = scale_factor as i32;

    if parameters.allocation == SbcAllocation::Snr {
        return scale_factor;
    }

    if scale_factor == 0 {
        return -5;
    }

    let frequency = parameters.frequency_index();
    let offset = if parameters.subbands == 4 {
        LOUDNESS_OFFSETS_4[frequency][subband]
    } else {
        LOUDNESS_OFFSETS_8[frequency][subband]
    };
    let loudness = scale_factor - offset;

    if loudness > 0 { loudness / 2 } else { loudness }
}

fn allocate_channels(
    parameters: &SbcParameters,
    scale_factors: &SubbandValues,
    channels: core::ops::Range<usize>,
    bits: &mut SubbandValues,
) {
    let subbands = parameters.subbands as usize;
    let bitpool = parameters.bitpool as i32;
    let mut needs = [[0i32; MAXIMUM_SUBBANDS]; MAXIMUM_CHANNELS];

    for channel in channels.clone() {
        for subband in 0..subbands {
            needs[channel][subband] =
                bit_need(parameters, scale_factors[channel][subband], subband);
        }
    }

    let each = || {
        (0..subbands).flat_map({
            let channels = channels.clone();
            move |subband| channels.clone().map(move |channel| (channel, subband))
        })
    };

    let mut slice = each()
        .map(|(channel, subband)| needs[channel][subband])
        .max()
        .unwrap_or(0)
        + 1;
    let mut count = 0;
    let mut slice_count = 0;

    loop {
        slice -= 1;
        count += slice_count;
        slice_count = 0;

        for (channel, subband) in each() {
            let need = needs[channel][subband];

            if need > slice + 1 && need < slice + 16 {
                slice_count += 1;
            } else if need == slice + 1 {
                slice_count += 2;
            }
        }

        if count + slice_count >= bitpool {
            break;
        }
    }

    if count + slice_count == bitpool {
        count += slice_count;
        slice -= 1;
    }

    for (channel, subband) in each() {
        let need = needs[channel][subband];

        bits[channel][subband] = if need < slice + 2 {
            0
        } else {
            (need - slice).min(16) as u8
        };
    }

    for (channel, subband) in each() {
        if count >= bitpool {
            break;
        }

        let bit = &mut bits[channel][subband];

        if (2..16).contains(bit) {
            *bit += 1;
            count += 1;
        } else if needs[channel][subband] == slice + 1 && bitpool > count + 1 {
            *bit = 2;
            count += 2;
        }
    }

    for (channel, subband) in each() {
        if count >= bitpool {
            break;
        }

        let bit = &mut bits[channel][subband];

        if *bit < 16 {
            *bit += 1;
            count += 1;
        }
    }
}

fn allocate(parameters: &SbcParameters, scale_factors: &SubbandValues) -> SubbandValues {
    let mut bits = [[0; MAXIMUM_SUBBANDS]; MAXIMUM_CHANNELS];

    match parameters.channel_mode {
        SbcChannelMode::Mono => allocate_channels(parameters, scale_factors, 0..1, &mut bits),
        SbcChannelMode::DualChannel => {
            allocate_channels(parameters, scale_factors, 0..1, &mut bits);
            allocate_channels(parameters, scale_factors, 1..2, &mut bits);
        }
        SbcChannelMode::Stereo | SbcChannelMode::JointStereo => {
            allocate_channels(parameters, scale_factors, 0..2, &mut bits)
        }
    }

    bits
}

fn checksum(frame: &[u8], side_bits: usize) -> u8 {
    let side = &frame[HEADER_SIZE..];
    let bytes = side_bits / 8;
    let crc = Crc8::update_bits(0x0F, frame[1], 8);
    let crc = Crc8::update_bits(crc, frame[2], 8);
    let crc = side[..bytes]
        .iter()
        .fold(crc, |crc, byte| Crc8::update_bits(crc, *byte, 8));

    match side_bits % 8 {
        0 => crc,
        bits => Crc8::update_bits(crc, side[bytes], bits as u32),
    }
}

struct BitWriter<'a> {
    data: &'a mut [u8],
    position: usize,
}

impl BitWriter<'_> {
    fn write(&mut self, value: u32, count: u8) {
        for bit in (0..count).rev() {
            if (value >> bit) & 1 == 1 {
                self.data[self.position / 8] |= 0x80 >> (self.position % 8);
            }

            self.position += 1;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SbcDecoder {
    parameters: Option<SbcParameters>,
    synthesis: [[i32; SYNTHESIS]; MAXIMUM_CHANNELS],
    finishing: bool,
}

impl Default for SbcDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SbcDecoder {
    pub const fn new() -> Self {
        Self {
            parameters: None,
            synthesis: [[0; SYNTHESIS]; MAXIMUM_CHANNELS],
            finishing: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub const fn parameters(&self) -> Option<SbcParameters> {
        self.parameters
    }

    pub fn decode_into<S: Sample>(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        let mut progress = Progress::default();

        loop {
            let input = &input[progress.consumed..];
            let output = &mut output[progress.produced..];

            match self.decode_next(input, output) {
                Ok(Some((consumed, produced))) => {
                    progress.consumed += consumed;
                    progress.produced += produced;
                }
                Ok(None) => return Ok(progress),
                Err(error) if progress.is_stalled() => return Err(error),
                Err(_) => return Ok(progress),
            }
        }
    }

    fn decode_next<S: Sample>(
        &mut self,
        input: &[u8],
        output: &mut [S],
    ) -> Result<Option<(usize, usize)>> {
        let parameters = match SbcParameters::parse(input) {
            Err(Error::Incomplete) if input.is_empty() || !self.finishing => return Ok(None),
            result => result?,
        };
        let length = parameters.frame_length();
        let samples = parameters.frame_samples();

        if input.len() < length {
            return if self.finishing {
                Err(Error::Incomplete)
            } else {
                Ok(None)
            };
        }

        if output.len() < samples {
            return Err(Error::BufferTooSmall);
        }

        self.decode_frame(&parameters, &input[..length], output)?;

        Ok(Some((length, samples)))
    }

    fn decode_frame<S: Sample>(
        &mut self,
        parameters: &SbcParameters,
        frame: &[u8],
        output: &mut [S],
    ) -> Result<()> {
        if checksum(frame, parameters.side_bits()) != frame[3] {
            return Err(Error::InvalidData);
        }

        if self
            .parameters
            .is_none_or(|previous| previous.subbands != parameters.subbands)
        {
            self.synthesis = [[0; SYNTHESIS]; MAXIMUM_CHANNELS];
        }

        self.parameters = Some(*parameters);

        let channels = parameters.channels() as usize;
        let subbands = parameters.subbands as usize;
        let mut reader = BitReader::new(&frame[HEADER_SIZE..]);

        let mut join = [false; MAXIMUM_SUBBANDS];

        if parameters.channel_mode == SbcChannelMode::JointStereo {
            for joined in &mut join[..subbands] {
                *joined = reader.read(1)? == 1;
            }
        }

        let mut scale_factors = [[0u8; MAXIMUM_SUBBANDS]; MAXIMUM_CHANNELS];

        for channel in &mut scale_factors[..channels] {
            for scale_factor in &mut channel[..subbands] {
                *scale_factor = reader.read(4)? as u8;
            }
        }

        let bits = allocate(parameters, &scale_factors);

        for block in 0..parameters.blocks as usize {
            let mut samples = [[0i32; MAXIMUM_SUBBANDS]; MAXIMUM_CHANNELS];

            for channel in 0..channels {
                for subband in 0..subbands {
                    let bit = bits[channel][subband];

                    if bit == 0 {
                        continue;
                    }

                    let levels = (1i64 << bit) - 1;
                    let quantized = reader.read(bit.into())? as i64;
                    let shift = scale_factors[channel][subband] as u32 + 1 + SUBBAND_FRACTION;

                    samples[channel][subband] =
                        (((2 * quantized + 1 - levels) << shift) / levels) as i32;
                }
            }

            for subband in 0..subbands {
                if join[subband] {
                    let (left, right) = (samples[0][subband], samples[1][subband]);

                    samples[0][subband] = left + right;
                    samples[1][subband] = left - right;
                }
            }

            for (channel, samples) in samples[..channels].iter().enumerate() {
                let mut pcm = [0i16; MAXIMUM_SUBBANDS];

                synthesize(
                    &mut self.synthesis[channel],
                    &samples[..subbands],
                    &mut pcm[..subbands],
                );

                for (index, sample) in pcm[..subbands].iter().enumerate() {
                    output[(block * subbands + index) * channels + channel] = sample.convert();
                }
            }
        }

        Ok(())
    }
}

fn synthesize(state: &mut [i32; SYNTHESIS], samples: &[i32], output: &mut [i16]) {
    let subbands = samples.len();
    let filterbank = Filterbank::new(subbands);

    state.copy_within(..18 * subbands, 2 * subbands);

    for (value, row) in state[..2 * subbands]
        .iter_mut()
        .zip(filterbank.synthesis.chunks_exact(subbands))
    {
        let sum: i64 = row
            .iter()
            .zip(samples)
            .map(|(cosine, sample)| *cosine as i64 * *sample as i64)
            .sum();

        *value = (sum >> COSINE_FRACTION).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    }

    let shift = PROTOTYPE_FRACTION + SUBBAND_FRACTION - subbands.trailing_zeros();

    for (index, output) in output.iter_mut().enumerate() {
        let mut sum = 0i64;

        for group in 0..5 {
            let base = 4 * subbands * group + index;
            let position = 2 * subbands * group + index;

            sum += state[base] as i64 * filterbank.prototype[position] as i64
                + state[base + 3 * subbands] as i64
                    * filterbank.prototype[position + subbands] as i64;
        }

        let value = (-sum + (1 << (shift - 1))) >> shift;

        *output = value.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
    }
}

impl Decoder for SbcDecoder {
    type Sample = i16;

    fn configuration(&self) -> Option<Configuration> {
        self.parameters.map(|parameters| parameters.configuration())
    }

    fn minimum_output(&self) -> usize {
        SBC_MAXIMUM_FRAME_SAMPLES
    }

    fn finish(&mut self) {
        self.finishing = true;
    }

    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<Progress> {
        self.decode_into(input, output)
    }
}

fn analyze(history: &mut [i16; WINDOW], input: impl Iterator<Item = i16>, output: &mut [i32]) {
    let subbands = output.len();
    let filterbank = Filterbank::new(subbands);

    history.copy_within(..9 * subbands, subbands);

    for (index, sample) in input.enumerate() {
        history[subbands - 1 - index] = sample;
    }

    let mut partial = [0i64; 2 * MAXIMUM_SUBBANDS];

    for (row, partial) in partial[..2 * subbands].iter_mut().enumerate() {
        let sum: i64 = (row..10 * subbands)
            .step_by(2 * subbands)
            .map(|index| filterbank.prototype[index] as i64 * history[index] as i64)
            .sum();

        *partial = sum >> (PROTOTYPE_FRACTION - PARTIAL_FRACTION);
    }

    for (output, row) in output
        .iter_mut()
        .zip(filterbank.analysis.chunks_exact(2 * subbands))
    {
        let sum: i64 = row
            .iter()
            .zip(&partial)
            .map(|(cosine, partial)| *cosine as i64 * partial)
            .sum();

        *output = (sum >> (COSINE_FRACTION + PARTIAL_FRACTION - ANALYSIS_FRACTION)) as i32;
    }
}

fn scale_factor(maximum: i32) -> u8 {
    (0..15u8)
        .find(|factor| maximum < 1 << (*factor as u32 + 1 + ANALYSIS_FRACTION))
        .unwrap_or(15)
}

fn quantize(sample: i32, scale_factor: u8, bits: u8) -> u32 {
    let levels = (1i64 << bits) - 1;
    let shift = scale_factor as u32 + 1 + ANALYSIS_FRACTION;
    let quantized = ((sample as i64 + (1 << shift)) * levels) >> (shift + 1);

    quantized.clamp(0, levels) as u32
}

type SubbandSamples = [[[i32; MAXIMUM_SUBBANDS]; MAXIMUM_CHANNELS]; MAXIMUM_BLOCKS];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SbcEncoder {
    parameters: SbcParameters,
    analysis: [[i16; WINDOW]; MAXIMUM_CHANNELS],
}

impl SbcEncoder {
    pub fn new(parameters: SbcParameters) -> Result<Self> {
        parameters.validate()?;

        Ok(Self {
            parameters,
            analysis: [[0; WINDOW]; MAXIMUM_CHANNELS],
        })
    }

    pub const fn parameters(&self) -> SbcParameters {
        self.parameters
    }

    pub fn reset(&mut self) {
        self.analysis = [[0; WINDOW]; MAXIMUM_CHANNELS];
    }

    fn encode_frame(&mut self, input: &[i16], frame: &mut [u8]) {
        let parameters = self.parameters;
        let channels = parameters.channels() as usize;
        let subbands = parameters.subbands as usize;
        let blocks = parameters.blocks as usize;
        let mut samples: SubbandSamples =
            [[[0; MAXIMUM_SUBBANDS]; MAXIMUM_CHANNELS]; MAXIMUM_BLOCKS];

        for (block, samples) in samples[..blocks].iter_mut().enumerate() {
            for (channel, samples) in samples[..channels].iter_mut().enumerate() {
                let start = block * subbands * channels + channel;
                let input = input[start..]
                    .iter()
                    .step_by(channels)
                    .take(subbands)
                    .copied();

                analyze(&mut self.analysis[channel], input, &mut samples[..subbands]);
            }
        }

        let maximum = |samples: &SubbandSamples, channel: usize, subband: usize| {
            samples[..blocks]
                .iter()
                .map(|block| block[channel][subband].saturating_abs())
                .max()
                .unwrap_or(0)
        };

        let mut scale_factors = [[0u8; MAXIMUM_SUBBANDS]; MAXIMUM_CHANNELS];

        for (channel, factors) in scale_factors[..channels].iter_mut().enumerate() {
            for (subband, factor) in factors[..subbands].iter_mut().enumerate() {
                *factor = scale_factor(maximum(&samples, channel, subband));
            }
        }

        let mut join = [false; MAXIMUM_SUBBANDS];

        if parameters.channel_mode == SbcChannelMode::JointStereo {
            for (subband, joined) in join[..subbands - 1].iter_mut().enumerate() {
                let mut sum = 0;
                let mut difference = 0;

                for block in &samples[..blocks] {
                    let (left, right) = (block[0][subband], block[1][subband]);

                    sum = sum.max(((left >> 1) + (right >> 1)).saturating_abs());
                    difference = difference.max(((left >> 1) - (right >> 1)).saturating_abs());
                }

                let (sum, difference) = (scale_factor(sum), scale_factor(difference));

                if sum + difference < scale_factors[0][subband] + scale_factors[1][subband] {
                    *joined = true;
                    scale_factors[0][subband] = sum;
                    scale_factors[1][subband] = difference;

                    for block in &mut samples[..blocks] {
                        let (left, right) = (block[0][subband] >> 1, block[1][subband] >> 1);

                        block[0][subband] = left + right;
                        block[1][subband] = left - right;
                    }
                }
            }
        }

        let bits = allocate(&parameters, &scale_factors);
        let length = parameters.frame_length();
        let frame = &mut frame[..length];

        frame.fill(0);
        frame[..3].copy_from_slice(&parameters.header());

        let mut writer = BitWriter {
            data: frame,
            position: HEADER_SIZE * 8,
        };

        if parameters.channel_mode == SbcChannelMode::JointStereo {
            for joined in &join[..subbands] {
                writer.write(*joined as u32, 1);
            }
        }

        for factors in &scale_factors[..channels] {
            for factor in &factors[..subbands] {
                writer.write(*factor as u32, 4);
            }
        }

        for block in &samples[..blocks] {
            for channel in 0..channels {
                for subband in 0..subbands {
                    let bit = bits[channel][subband];

                    if bit > 0 {
                        let factor = scale_factors[channel][subband];

                        writer.write(quantize(block[channel][subband], factor, bit), bit);
                    }
                }
            }
        }

        frame[3] = checksum(frame, parameters.side_bits());
    }
}

impl Encoder for SbcEncoder {
    type Sample = i16;

    fn configuration(&self) -> Configuration {
        self.parameters.configuration()
    }

    fn minimum_input(&self) -> usize {
        self.parameters.frame_samples()
    }

    fn minimum_output(&self) -> usize {
        self.parameters.frame_length()
    }

    fn encode(&mut self, input: &[i16], output: &mut [u8]) -> Result<Progress> {
        let samples = self.parameters.frame_samples();
        let length = self.parameters.frame_length();

        if output.len() < length {
            return Err(Error::BufferTooSmall);
        }

        let mut progress = Progress::default();

        for (input, frame) in input
            .chunks_exact(samples)
            .zip(output.chunks_exact_mut(length))
        {
            self.encode_frame(input, frame);

            progress.consumed += samples;
            progress.produced += length;
        }

        Ok(progress)
    }
}
