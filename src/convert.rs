use crate::{Format, Sample};

pub fn convert<I: Sample, O: Sample>(input: &[I], output: &mut [O]) -> usize {
    let count = input.len().min(output.len());

    for (output, input) in output.iter_mut().zip(input) {
        *output = input.convert();
    }

    count
}

pub fn convert_bytes(
    input: &[u8],
    input_format: Format,
    output: &mut [u8],
    output_format: Format,
) -> usize {
    match input_format {
        Format::U8 => convert_bytes_from::<u8>(input, output, output_format),
        Format::I16 => convert_bytes_from::<i16>(input, output, output_format),
        Format::I32 => convert_bytes_from::<i32>(input, output, output_format),
        Format::F32 => convert_bytes_from::<f32>(input, output, output_format),
    }
}

fn convert_bytes_from<I: Sample>(input: &[u8], output: &mut [u8], output_format: Format) -> usize {
    match output_format {
        Format::U8 => convert_bytes_typed::<I, u8>(input, output),
        Format::I16 => convert_bytes_typed::<I, i16>(input, output),
        Format::I32 => convert_bytes_typed::<I, i32>(input, output),
        Format::F32 => convert_bytes_typed::<I, f32>(input, output),
    }
}

fn convert_bytes_typed<I: Sample, O: Sample>(input: &[u8], output: &mut [u8]) -> usize {
    let mut count = 0;

    for (input, output) in input
        .chunks_exact(I::SIZE)
        .zip(output.chunks_exact_mut(O::SIZE))
    {
        I::read_le(input).convert::<O>().write_le(output);
        count += 1;
    }

    count
}

pub fn convert_channels<S: Sample>(
    input: &[S],
    input_channels: u8,
    output: &mut [S],
    output_channels: u8,
) -> usize {
    let input_channels = input_channels as usize;
    let output_channels = output_channels as usize;

    if input_channels == 0 || output_channels == 0 {
        return 0;
    }

    let mut frames = 0;

    for (input, output) in input
        .chunks_exact(input_channels)
        .zip(output.chunks_exact_mut(output_channels))
    {
        if input_channels == output_channels {
            output.copy_from_slice(input);
        } else if input_channels == 1 {
            output.fill(input[0]);
        } else if output_channels == 1 {
            let sum: i64 = input.iter().map(|sample| sample.to_i32() as i64).sum();
            output[0] = S::from_i32((sum / input_channels as i64) as i32);
        } else {
            for (index, output) in output.iter_mut().enumerate() {
                *output = input.get(index).copied().unwrap_or(S::SILENCE);
            }
        }

        frames += 1;
    }

    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed() {
        let input = [i16::MIN, 0, i16::MAX];
        let mut output = [0u8; 2];

        assert_eq!(convert(&input, &mut output), 2);
        assert_eq!(output, [0x00, 0x80]);
    }

    #[test]
    fn bytes() {
        let input = [0x00u8, 0x80, 0xFF];
        let mut output = [0u8; 6];

        assert_eq!(
            convert_bytes(&input, Format::U8, &mut output, Format::I16),
            3
        );
        assert_eq!(i16::read_le(&output[0..]), i16::MIN);
        assert_eq!(i16::read_le(&output[2..]), 0);
        assert_eq!(i16::read_le(&output[4..]), 0x7F00);
    }

    #[test]
    fn bytes_stop_at_shortest_buffer() {
        let input = [0u8; 8];
        let mut output = [0u8; 3];

        assert_eq!(
            convert_bytes(&input, Format::F32, &mut output, Format::I16),
            1
        );
    }

    #[test]
    fn mono_to_stereo() {
        let mut output = [0i16; 6];

        assert_eq!(convert_channels(&[1i16, 2, 3], 1, &mut output, 2), 3);
        assert_eq!(output, [1, 1, 2, 2, 3, 3]);
    }

    #[test]
    fn stereo_to_mono() {
        let mut output = [0i16; 2];

        assert_eq!(
            convert_channels(&[100i16, 200, i16::MAX, i16::MAX], 2, &mut output, 1),
            2
        );
        assert_eq!(output, [150, i16::MAX]);
    }

    #[test]
    fn surround_to_stereo_keeps_front_channels() {
        let mut output = [0i16; 2];

        assert_eq!(
            convert_channels(&[1i16, 2, 3, 4, 5, 6], 6, &mut output, 2),
            1
        );
        assert_eq!(output, [1, 2]);
    }

    #[test]
    fn stereo_to_quad_pads_with_silence() {
        let mut output = [9u8; 4];

        assert_eq!(convert_channels(&[1u8, 2], 2, &mut output, 4), 1);
        assert_eq!(output, [1, 2, 0x80, 0x80]);
    }
}
