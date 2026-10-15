use nanoaudio_core::{Format, I24, Sample};

use crate::bytes::{Bytes, read};
use crate::{DefaultRepresentation, Representation, Stream};

type Value<S> = <<S as Stream>::Representation as Representation>::Value;

const MULTIPLIER: u32 = 0x915F_77F5;
const INCREMENT: u32 = 0x9E37_79B9;
const LANES: usize = 8;
const VECTOR: bool = cfg!(any(
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_feature = "simd128"
));

const fn inverse(value: u32) -> u32 {
    let mut inverse = value;
    let mut round = 0;

    while round < 5 {
        inverse = inverse.wrapping_mul(2u32.wrapping_sub(value.wrapping_mul(inverse)));
        round += 1;
    }

    inverse
}

const fn jump(steps: usize) -> (u32, u32) {
    let mut multiplier = 1u32;
    let mut increment = 0u32;
    let mut step = 0;

    while step < steps {
        multiplier = multiplier.wrapping_mul(MULTIPLIER);
        increment = increment.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT);
        step += 1;
    }

    (multiplier, increment)
}

const INVERSE_MULTIPLIER: u32 = inverse(MULTIPLIER);
const LANE_JUMP: (u32, u32) = jump(LANES);

#[derive(Debug, Clone, Copy)]
struct Noise {
    state: u32,
    shift: u32,
    mask: i32,
}

impl Noise {
    fn new(target: Format, seed: u32) -> Self {
        let (shift, mask) = match target {
            Format::U8 => (7, -1),
            Format::I16 => (15, -1),
            Format::I24 => (23, -1),
            Format::I32 | Format::F32 => (31, 0),
        };

        Self {
            state: seed,
            shift,
            mask,
        }
    }

    #[inline(always)]
    fn value(&self, state: u32) -> i32 {
        let bits = state ^ (state >> 16);
        let first = (((bits & 0xFFFF) << 15) >> self.shift) as i32;
        let second = (((bits >> 16) << 15) >> self.shift) as i32;

        (first - second) & self.mask
    }

    #[inline(always)]
    fn next(&mut self) -> i32 {
        self.state = self.state.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT);
        self.value(self.state)
    }

    #[inline(always)]
    fn lanes(&self) -> [u32; LANES] {
        let mut lanes = [self.state; LANES];

        for index in (0..LANES - 1).rev() {
            lanes[index] = lanes[index + 1]
                .wrapping_sub(INCREMENT)
                .wrapping_mul(INVERSE_MULTIPLIER);
        }

        lanes
    }

    #[inline(always)]
    fn zip<A, B>(
        &mut self,
        input: &[A],
        output: &mut [B],
        mut function: impl FnMut(&A, &mut B, i32),
    ) {
        if !VECTOR {
            for (input, output) in input.iter().zip(output.iter_mut()) {
                function(input, output, self.next());
            }

            return;
        }

        let input_blocks = input.as_chunks::<LANES>().0;
        let output_blocks = output.as_chunks_mut::<LANES>().0;
        let mut lanes = self.lanes();
        let mut blocks = 0;

        for (input, output) in input_blocks.iter().zip(output_blocks.iter_mut()) {
            lanes = lanes.map(|lane| lane.wrapping_mul(LANE_JUMP.0).wrapping_add(LANE_JUMP.1));

            for ((input, output), lane) in input.iter().zip(output.iter_mut()).zip(lanes) {
                function(input, output, self.value(lane));
            }

            blocks += 1;
        }

        self.state = lanes[LANES - 1];
        let done = blocks * LANES;

        for (input, output) in input[done..].iter().zip(output[done..].iter_mut()) {
            function(input, output, self.next());
        }
    }

    #[inline(always)]
    fn apply<R: Representation>(&mut self, value: R::Value) -> R::Value {
        dither::<R>(value, self.next())
    }
}

#[inline(always)]
fn dither<R: Representation>(value: R::Value, noise: i32) -> R::Value {
    R::add(value, R::from_sample(noise))
}

/// The stage returned by [`Stream::dither`], which adds triangular (TPDF) dither of one LSB of the target format to every value.
///
/// Use it as the last stage before writing to a narrower format, such as float or 24-bit audio to 16 or 8 bits. It adds nothing when the target is 32-bit.
pub struct Dither<St: Stream> {
    stream: St,
    noise: Noise,
}

impl<St: Stream> Dither<St> {
    pub(crate) fn new(stream: St, target: Format, seed: u32) -> Self {
        Self {
            stream,
            noise: Noise::new(target, seed),
        }
    }
}

impl<St: Stream> Stream for Dither<St> {
    type Representation = St::Representation;

    fn channels(&self) -> u8 {
        self.stream.channels()
    }

    fn next_frame(&mut self, frame: &mut [Value<Self>]) -> bool {
        if !self.stream.next_frame(frame) {
            return false;
        }

        for value in frame {
            *value = self.noise.apply::<St::Representation>(*value);
        }

        true
    }
}

/// Converts samples like [`convert`](fn@crate::convert), adding triangular (TPDF) dither of one LSB of the output format.
///
/// `state` carries the noise generator between calls: seed it once, with any value, and keep passing it. A seed gives the same noise on every target, however the input is split into calls. Processing uses the [`DefaultRepresentation`].
pub fn convert_dithered<I: Sample, O: Sample>(
    input: &[I],
    output: &mut [O],
    state: &mut u32,
) -> usize {
    let mut noise = Noise::new(O::FORMAT, *state);
    let count = input.len().min(output.len());

    noise.zip(input, output, |input, output, noise| {
        *output = DefaultRepresentation::to_sample(dither::<DefaultRepresentation>(
            DefaultRepresentation::from_sample(*input),
            noise,
        ));
    });

    *state = noise.state;
    count
}

/// Converts bytes like [`convert_bytes`](crate::convert_bytes), adding triangular (TPDF) dither of one LSB of the output format.
///
/// `state` works as in [`convert_dithered`].
pub fn convert_bytes_dithered(
    input: &[u8],
    input_format: Format,
    output: &mut [u8],
    output_format: Format,
    state: &mut u32,
) -> usize {
    match input_format {
        Format::U8 => convert_bytes_dithered_from::<u8, 1>(input, output, output_format, state),
        Format::I16 => convert_bytes_dithered_from::<i16, 2>(input, output, output_format, state),
        Format::I24 => convert_bytes_dithered_from::<I24, 3>(input, output, output_format, state),
        Format::I32 => convert_bytes_dithered_from::<i32, 4>(input, output, output_format, state),
        Format::F32 => convert_bytes_dithered_from::<f32, 4>(input, output, output_format, state),
    }
}

fn convert_bytes_dithered_from<I: Bytes<INPUT>, const INPUT: usize>(
    input: &[u8],
    output: &mut [u8],
    output_format: Format,
    state: &mut u32,
) -> usize {
    match output_format {
        Format::U8 => convert_bytes_dithered_typed::<I, u8, INPUT, 1>(input, output, state),
        Format::I16 => convert_bytes_dithered_typed::<I, i16, INPUT, 2>(input, output, state),
        Format::I24 => convert_bytes_dithered_typed::<I, I24, INPUT, 3>(input, output, state),
        Format::I32 => convert_bytes_dithered_typed::<I, i32, INPUT, 4>(input, output, state),
        Format::F32 => convert_bytes_dithered_typed::<I, f32, INPUT, 4>(input, output, state),
    }
}

fn convert_bytes_dithered_typed<
    I: Bytes<INPUT>,
    O: Bytes<OUTPUT>,
    const INPUT: usize,
    const OUTPUT: usize,
>(
    input: &[u8],
    output: &mut [u8],
    state: &mut u32,
) -> usize {
    let mut noise = Noise::new(O::FORMAT, *state);
    let input = input.as_chunks::<INPUT>().0;
    let output = output.as_chunks_mut::<OUTPUT>().0;

    noise.zip(input, output, |input, output, noise| {
        let value = DefaultRepresentation::from_sample(read::<I, INPUT, OUTPUT>(input));
        DefaultRepresentation::to_sample::<O>(dither::<DefaultRepresentation>(value, noise))
            .store(output);
    });

    *state = noise.state;
    input.len().min(output.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::testing::{FORMATS, LENGTH, ROUNDS, fill, length};

    fn reference(
        input: &[u8],
        input_format: Format,
        output: &mut [u8],
        output_format: Format,
        state: &mut u32,
    ) -> usize {
        match input_format {
            Format::U8 => reference_from::<u8>(input, output, output_format, state),
            Format::I16 => reference_from::<i16>(input, output, output_format, state),
            Format::I24 => reference_from::<I24>(input, output, output_format, state),
            Format::I32 => reference_from::<i32>(input, output, output_format, state),
            Format::F32 => reference_from::<f32>(input, output, output_format, state),
        }
    }

    fn reference_from<I: Sample>(
        input: &[u8],
        output: &mut [u8],
        output_format: Format,
        state: &mut u32,
    ) -> usize {
        match output_format {
            Format::U8 => reference_typed::<I, u8>(input, output, state),
            Format::I16 => reference_typed::<I, i16>(input, output, state),
            Format::I24 => reference_typed::<I, I24>(input, output, state),
            Format::I32 => reference_typed::<I, i32>(input, output, state),
            Format::F32 => reference_typed::<I, f32>(input, output, state),
        }
    }

    fn reference_typed<I: Sample, O: Sample>(
        input: &[u8],
        output: &mut [u8],
        state: &mut u32,
    ) -> usize {
        let mut noise = Noise::new(O::FORMAT, *state);
        let mut count = 0;

        for (input, output) in input
            .chunks_exact(I::SIZE)
            .zip(output.chunks_exact_mut(O::SIZE))
        {
            let value = DefaultRepresentation::from_sample(I::read_le(input));
            DefaultRepresentation::to_sample::<O>(noise.apply::<DefaultRepresentation>(value))
                .write_le(output);
            count += 1;
        }

        *state = noise.state;
        count
    }

    #[test]
    fn bytes_match_reference() {
        let mut input = [0u8; LENGTH];
        let mut expected = [0u8; LENGTH];

        for round in 0..ROUNDS {
            for input_format in FORMATS {
                for output_format in FORMATS {
                    let input = &mut input[..length(round, 0)];
                    let output_length = length(round, 5);
                    fill(input, input_format, round, 0);
                    fill(&mut expected, output_format, round, 1);
                    let mut actual = expected;
                    let mut actual_state = round as u32;
                    let mut expected_state = round as u32;

                    assert_eq!(
                        convert_bytes_dithered(
                            input,
                            input_format,
                            &mut actual[..output_length],
                            output_format,
                            &mut actual_state
                        ),
                        reference(
                            input,
                            input_format,
                            &mut expected[..output_length],
                            output_format,
                            &mut expected_state
                        )
                    );
                    assert_eq!(actual, expected);
                    assert_eq!(actual_state, expected_state);
                }
            }
        }
    }

    #[test]
    fn noise_sequence_is_stable() {
        let mut noise = Noise::new(Format::I16, 1);
        let values: [i32; 8] = core::array::from_fn(|_| noise.next());

        assert_eq!(
            values,
            [
                44_706, -23_277, -20_996, 13_197, 9_578, 23_591, -12_512, 15_561
            ]
        );
    }

    #[test]
    fn lanes_match_the_serial_sequence() {
        for length in 0..40 {
            for seed in [0, 1, u32::MAX] {
                let mut serial = Noise::new(Format::U8, seed);
                let mut lanes = Noise::new(Format::U8, seed);
                let input = [0u8; 40];
                let mut output = [0i32; 40];

                lanes.zip(&input[..length], &mut output, |_, output, noise| {
                    *output = noise;
                });

                assert!(output[..length].iter().all(|value| *value == serial.next()));
                assert_eq!(lanes.state, serial.state);
            }
        }
    }

    #[test]
    fn slices_continue_at_any_split() {
        let input: [i32; 37] = core::array::from_fn(|index| index as i32 * 1_000_003);
        let mut whole = [0i16; 37];
        let mut state = 3;
        convert_dithered(&input, &mut whole, &mut state);

        for split in 0..input.len() {
            let mut parts = [0i16; 37];
            let mut state = 3;
            convert_dithered(&input[..split], &mut parts[..split], &mut state);
            convert_dithered(&input[split..], &mut parts[split..], &mut state);

            assert_eq!(parts, whole);
        }
    }

    #[test]
    fn noise_stays_within_one_lsb() {
        for (target, lsb) in [
            (Format::U8, 1 << 24),
            (Format::I16, 1 << 16),
            (Format::I24, 1 << 8),
        ] {
            let mut noise = Noise::new(target, 0);
            let mut sum = 0i64;

            for _ in 0..4_096 {
                let value = noise.next();
                assert!(value.abs() < lsb);
                sum += value as i64;
            }

            assert!((sum / 4_096).abs() < lsb as i64 / 16);
        }

        assert_eq!(Noise::new(Format::F32, 1).next(), 0);
        assert_eq!(Noise::new(Format::I32, 1).next(), 0);
    }
}
