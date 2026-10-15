use nanoaudio_core::{I24, Sample};

pub(crate) trait Bytes<const SIZE: usize>: Sample {
    fn from_bytes(bytes: &[u8; SIZE]) -> Self;

    #[inline(always)]
    fn from_separate_bytes(bytes: &[u8; SIZE]) -> Self {
        Self::from_bytes(bytes)
    }

    fn to_bytes(self) -> [u8; SIZE];

    #[inline(always)]
    fn store(self, bytes: &mut [u8; SIZE]) {
        *bytes = self.to_bytes();
    }
}

impl Bytes<1> for u8 {
    #[inline(always)]
    fn from_bytes(bytes: &[u8; 1]) -> Self {
        bytes[0]
    }

    #[inline(always)]
    fn to_bytes(self) -> [u8; 1] {
        [self]
    }
}

impl Bytes<2> for i16 {
    #[inline(always)]
    fn from_bytes(bytes: &[u8; 2]) -> Self {
        i16::from_le_bytes(*bytes)
    }

    #[inline(always)]
    fn from_separate_bytes(bytes: &[u8; 2]) -> Self {
        i16::from_le_bytes([bytes[0], bytes[1]])
    }

    #[inline(always)]
    fn to_bytes(self) -> [u8; 2] {
        self.to_le_bytes()
    }
}

impl Bytes<3> for I24 {
    #[inline(always)]
    fn from_bytes(bytes: &[u8; 3]) -> Self {
        I24::new(i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8)
    }

    #[inline(always)]
    fn to_bytes(self) -> [u8; 3] {
        let [low, middle, high, _] = self.value().to_le_bytes();

        [low, middle, high]
    }

    #[inline(always)]
    fn store(self, bytes: &mut [u8; 3]) {
        let [low, middle, high] = self.to_bytes();

        bytes[0] = low;
        bytes[1] = middle;
        bytes[2] = high;
    }
}

impl Bytes<4> for i32 {
    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4]) -> Self {
        i32::from_le_bytes(*bytes)
    }

    #[inline(always)]
    fn to_bytes(self) -> [u8; 4] {
        self.to_le_bytes()
    }
}

impl Bytes<4> for f32 {
    #[inline(always)]
    fn from_bytes(bytes: &[u8; 4]) -> Self {
        f32::from_le_bytes(*bytes)
    }

    #[inline(always)]
    fn to_bytes(self) -> [u8; 4] {
        self.to_le_bytes()
    }
}

#[inline(always)]
pub(crate) fn read<S: Bytes<INPUT>, const INPUT: usize, const OUTPUT: usize>(
    bytes: &[u8; INPUT],
) -> S {
    if OUTPUT == 1 {
        S::from_separate_bytes(bytes)
    } else {
        S::from_bytes(bytes)
    }
}

#[inline(always)]
#[allow(clippy::chunks_exact_to_as_chunks)]
pub(crate) fn each<const SIZE: usize>(bytes: &mut [u8], mut function: impl FnMut(&mut [u8; SIZE])) {
    if SIZE == 1 {
        for chunk in bytes.chunks_exact_mut(SIZE) {
            if let Some(chunk) = chunk.first_chunk_mut::<SIZE>() {
                function(chunk);
            }
        }
    } else {
        for chunk in bytes.as_chunks_mut::<SIZE>().0 {
            function(chunk);
        }
    }
}

#[inline(always)]
#[allow(clippy::chunks_exact_to_as_chunks)]
pub(crate) fn pairs<const INPUT: usize, const OUTPUT: usize>(
    input: &[u8],
    output: &mut [u8],
    mut function: impl FnMut(&[u8; INPUT], &mut [u8; OUTPUT]),
) -> usize {
    let count = (input.len() / INPUT).min(output.len() / OUTPUT);

    if OUTPUT == 1 {
        for (input, output) in input
            .chunks_exact(INPUT)
            .zip(output.chunks_exact_mut(OUTPUT))
        {
            if let (Some(input), Some(output)) = (
                input.first_chunk::<INPUT>(),
                output.first_chunk_mut::<OUTPUT>(),
            ) {
                function(input, output);
            }
        }
    } else {
        let (input, _) = input.as_chunks::<INPUT>();
        let (output, _) = output.as_chunks_mut::<OUTPUT>();

        for (input, output) in input.iter().zip(output.iter_mut()) {
            function(input, output);
        }
    }

    count
}

#[cfg(test)]
pub(crate) mod testing {
    use nanoaudio_core::Format;

    pub(crate) const FORMATS: [Format; 5] = [
        Format::U8,
        Format::I16,
        Format::I24,
        Format::I32,
        Format::F32,
    ];

    pub(crate) const GAINS: [u16; 11] = [
        0,
        1,
        2047,
        2048,
        3072,
        4095,
        4096,
        4097,
        12_345,
        40_000,
        u16::MAX,
    ];

    pub(crate) const LENGTH: usize = if cfg!(miri) { 67 } else { 1031 };

    pub(crate) const ROUNDS: u64 = if cfg!(miri) { 1 } else { 64 };

    const EDGES: [u32; 24] = [
        0,
        1,
        u32::MAX,
        0x7FFF_FFFF,
        0x8000_0000,
        0x8000_0001,
        0x0000_7FFF,
        0x0000_8000,
        0x0000_FFFF,
        0x007F_FFFF,
        0x0080_0000,
        0x00FF_FFFF,
        0x0000_007F,
        0x0000_0080,
        0x0000_00FF,
        0x7F80_0000,
        0xFF80_0000,
        0x7FC0_0000,
        0x3F80_0000,
        0xBF80_0000,
        0x4000_0000,
        0x4F00_0000,
        0xCF00_0000,
        0x0080_0001,
    ];

    pub(crate) fn fill(bytes: &mut [u8], format: Format, round: u64, salt: u64) {
        let mut state = round.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ salt.wrapping_mul(0xD1B5_4A32_D192_ED03)
            ^ format.sample_size() as u64;

        for chunk in bytes.chunks_mut(8) {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut mixed = state;
            mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            mixed ^= mixed >> 31;

            for (byte, value) in chunk.iter_mut().zip(mixed.to_le_bytes()) {
                *byte = value;
            }
        }

        if round == 0 {
            let size = format.sample_size();
            let edges = EDGES.iter().cycle().skip((salt as usize * 5) % EDGES.len());

            for (chunk, edge) in bytes.chunks_exact_mut(size).zip(edges) {
                chunk.copy_from_slice(&edge.to_le_bytes()[..size]);
            }
        }

        if cfg!(miri) && format == Format::F32 {
            for chunk in bytes.as_chunks_mut::<4>().0 {
                if !f32::from_le_bytes(*chunk).is_finite() {
                    chunk[3] &= 0xBF;
                }
            }
        }
    }

    pub(crate) fn length(round: u64, offset: u64) -> usize {
        LENGTH - ((round * 3 + offset) % 13) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches_sample<S: Bytes<SIZE>, const SIZE: usize>(bytes: [u8; SIZE]) {
        let sample = S::from_bytes(&bytes);
        let mut written = [0u8; SIZE];
        sample.write_le(&mut written);

        let mut stored = [0u8; SIZE];
        sample.store(&mut stored);

        assert!(sample == S::read_le(&bytes));
        assert!(sample == S::from_separate_bytes(&bytes));
        assert_eq!(sample.to_bytes(), written);
        assert_eq!(stored, written);
    }

    #[test]
    fn matches_sample_layout() {
        let stride = if cfg!(miri) { 65_521 } else { 1 };

        for value in (0..=u32::from(u16::MAX)).step_by(stride) {
            let [low, high, _, _] = value.to_le_bytes();

            matches_sample::<u8, 1>([low]);
            matches_sample::<i16, 2>([low, high]);
        }

        let stride = if cfg!(miri) { 99_991 } else { 1 };

        for value in (0..1u32 << 24)
            .step_by(stride)
            .chain([0x7F_FFFF, 0x80_0000, 0xFF_FFFF])
        {
            let [low, middle, high, _] = value.to_le_bytes();

            matches_sample::<I24, 3>([low, middle, high]);
        }

        let stride = if cfg!(miri) { 99_999_989 } else { 65_521 };

        for value in (0..=u32::MAX)
            .step_by(stride)
            .chain([0x7F80_0000, 0xFFC0_0001, u32::MAX])
        {
            let bytes = value.to_le_bytes();

            matches_sample::<i32, 4>(bytes);

            let sample = f32::from_bytes(&bytes);
            let mut stored = [0u8; 4];
            sample.store(&mut stored);

            assert_eq!(sample.to_bits(), f32::read_le(&bytes).to_bits());
            assert_eq!(f32::from_separate_bytes(&bytes).to_bits(), sample.to_bits());
            assert_eq!(sample.to_bytes(), bytes);
            assert_eq!(stored, bytes);
        }
    }
}
