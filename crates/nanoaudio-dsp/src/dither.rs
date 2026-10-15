use nanoaudio_core::{Format, I24, Sample};

use crate::bytes::{Bytes, pairs, read};
use crate::{DefaultRepresentation, Representation, Stream};

type Value<S> = <<S as Stream>::Representation as Representation>::Value;

const DEFAULT_SEED: u32 = 0x9E37_79B9;

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
            state: if seed == 0 { DEFAULT_SEED } else { seed },
            shift,
            mask,
        }
    }

    #[inline(always)]
    fn next(&mut self) -> i32 {
        let mut state = self.state;
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        self.state = state;

        let first = (((state & 0xFFFF) << 15) >> self.shift) as i32;
        let second = (((state >> 16) << 15) >> self.shift) as i32;

        (first - second) & self.mask
    }

    #[inline(always)]
    fn apply<R: Representation>(&mut self, value: R::Value) -> R::Value {
        R::add(value, R::from_sample(self.next()))
    }
}

/// Adds triangular (TPDF) dither of one LSB of the target format to every sample.
/// Use it as the last stage before writing to a narrower format, such as float or 24-bit audio to 16 or 8 bits.
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

/// Converts samples like `convert`, adding triangular (TPDF) dither of one LSB of the output format.
/// `state` carries the noise generator between calls; seed it once and keep passing it.
pub fn convert_dithered<I: Sample, O: Sample>(
    input: &[I],
    output: &mut [O],
    state: &mut u32,
) -> usize {
    let mut noise = Noise::new(O::FORMAT, *state);
    let count = input.len().min(output.len());

    for (output, input) in output.iter_mut().zip(input) {
        *output = DefaultRepresentation::to_sample(
            noise.apply::<DefaultRepresentation>(DefaultRepresentation::from_sample(*input)),
        );
    }

    *state = noise.state;
    count
}

/// Converts bytes like `convert_bytes`, adding triangular (TPDF) dither of one LSB of the output format.
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
    let count = pairs::<INPUT, OUTPUT>(input, output, |input, output| {
        let value = DefaultRepresentation::from_sample(read::<I, INPUT, OUTPUT>(input));
        DefaultRepresentation::to_sample::<O>(noise.apply::<DefaultRepresentation>(value))
            .store(output);
    });

    *state = noise.state;
    count
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
