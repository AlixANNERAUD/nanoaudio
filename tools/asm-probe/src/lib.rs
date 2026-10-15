#![no_std]

use nanoaudio_core::codec::{Decoder, Encoder};
use nanoaudio_core::Format;
use nanoaudio_core::I24;
use nanoaudio_dsp::{
    convert, convert_dithered, mix_using, samples_using, Fixed, Float, Linear, Resampler, Sinc,
    Stream, Volume,
};
use nanoaudio_g711::{Law, G711};
use nanoaudio_qoa::{QoaDecoder, QoaEncoder};
use nanoaudio_sbc::{SbcDecoder, SbcEncoder};

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_volume_i16(samples: &mut [i16], volume: u16) {
    Volume::from_raw(volume).apply_to_using::<Fixed, i16>(samples);
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_mix_i16(output: &mut [i16], input: &[i16], volume: u16) -> usize {
    mix_using::<Fixed, i16>(output, input, Volume::from_raw(volume))
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_volume_u8(samples: &mut [u8], volume: u16) {
    Volume::from_raw(volume).apply_to_using::<Fixed, u8>(samples);
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_volume_i24(samples: &mut [I24], volume: u16) {
    Volume::from_raw(volume).apply_to_using::<Fixed, I24>(samples);
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_mix_i24(output: &mut [I24], input: &[I24], volume: u16) -> usize {
    mix_using::<Fixed, I24>(output, input, Volume::from_raw(volume))
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_volume_i32(samples: &mut [i32], volume: u16) {
    Volume::from_raw(volume).apply_to_using::<Fixed, i32>(samples);
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_mix_i32(output: &mut [i32], input: &[i32], volume: u16) -> usize {
    mix_using::<Fixed, i32>(output, input, Volume::from_raw(volume))
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_stream_i16(input: &[i16], output: &mut [i16], volume: u16) -> usize {
    match samples_using::<Fixed, i16>(input, 2) {
        Ok(stream) => stream
            .volume(Volume::from_raw(volume))
            .write_samples(output),
        Err(_) => 0,
    }
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_stream_i24(input: &[I24], output: &mut [I24], volume: u16) -> usize {
    match samples_using::<Fixed, I24>(input, 2) {
        Ok(stream) => stream
            .volume(Volume::from_raw(volume))
            .write_samples(output),
        Err(_) => 0,
    }
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_stream_i32(input: &[i32], output: &mut [i32], volume: u16) -> usize {
    match samples_using::<Fixed, i32>(input, 2) {
        Ok(stream) => stream
            .volume(Volume::from_raw(volume))
            .write_samples(output),
        Err(_) => 0,
    }
}

#[inline(never)]
#[no_mangle]
pub fn probe_float_volume_i16(samples: &mut [i16], volume: u16) {
    Volume::from_raw(volume).apply_to_using::<Float, i16>(samples);
}

#[inline(never)]
#[no_mangle]
pub fn probe_float_mix_i16(output: &mut [i16], input: &[i16], volume: u16) -> usize {
    mix_using::<Float, i16>(output, input, Volume::from_raw(volume))
}

#[inline(never)]
#[no_mangle]
pub fn probe_float_convert_i16(input: &[i16], output: &mut [f32]) -> usize {
    convert::<i16, f32>(input, output)
}

#[inline(never)]
#[no_mangle]
pub fn probe_float_stream_i16(input: &[i16], output: &mut [i16], volume: u16) -> usize {
    match samples_using::<Float, i16>(input, 2) {
        Ok(stream) => stream
            .volume(Volume::from_raw(volume))
            .write_samples(output),
        Err(_) => 0,
    }
}

#[inline(never)]
#[no_mangle]
pub fn probe_g711_decode_mu_law(input: &[u8], output: &mut [i16]) -> usize {
    decode(Law::Mu, input, output)
}

#[inline(never)]
#[no_mangle]
pub fn probe_g711_decode_a_law(input: &[u8], output: &mut [i16]) -> usize {
    decode(Law::A, input, output)
}

#[inline(never)]
#[no_mangle]
pub fn probe_g711_encode_mu_law(input: &[i16], output: &mut [u8]) -> usize {
    encode(Law::Mu, input, output)
}

#[inline(never)]
#[no_mangle]
pub fn probe_g711_encode_a_law(input: &[i16], output: &mut [u8]) -> usize {
    encode(Law::A, input, output)
}

fn decode(law: Law, input: &[u8], output: &mut [i16]) -> usize {
    match G711::new(law, 8000, 1) {
        Ok(mut codec) => codec
            .decode_into(input, output)
            .map_or(0, |progress| progress.produced),
        Err(_) => 0,
    }
}

fn encode(law: Law, input: &[i16], output: &mut [u8]) -> usize {
    match G711::new(law, 8000, 1) {
        Ok(mut codec) => codec
            .encode(input, output)
            .map_or(0, |progress| progress.produced),
        Err(_) => 0,
    }
}

#[inline(never)]
#[no_mangle]
pub fn probe_qoa_decode(decoder: &mut QoaDecoder, input: &[u8], output: &mut [i16]) -> usize {
    decoder
        .decode(input, output)
        .map_or(0, |progress| progress.produced)
}

#[inline(never)]
#[no_mangle]
pub fn probe_qoa_encode(encoder: &mut QoaEncoder, input: &[i16], output: &mut [u8]) -> usize {
    encoder
        .encode(input, output)
        .map_or(0, |progress| progress.produced)
}

#[inline(never)]
#[no_mangle]
pub fn probe_qoa_decode_stereo(
    decoder: &mut QoaDecoder<i16, 2>,
    input: &[u8],
    output: &mut [i16],
) -> usize {
    decoder
        .decode(input, output)
        .map_or(0, |progress| progress.produced)
}

#[inline(never)]
#[no_mangle]
pub fn probe_qoa_encode_stereo(
    encoder: &mut QoaEncoder<i16, 2>,
    input: &[i16],
    output: &mut [u8],
) -> usize {
    encoder
        .encode(input, output)
        .map_or(0, |progress| progress.produced)
}

#[inline(never)]
#[no_mangle]
pub fn probe_sbc_decode(decoder: &mut SbcDecoder, input: &[u8], output: &mut [i16]) -> usize {
    decoder
        .decode(input, output)
        .map_or(0, |progress| progress.produced)
}

#[inline(never)]
#[no_mangle]
pub fn probe_sbc_encode(encoder: &mut SbcEncoder, input: &[i16], output: &mut [u8]) -> usize {
    encoder
        .encode(input, output)
        .map_or(0, |progress| progress.produced)
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_stream_resample_linear_i16(
    resampler: &mut Resampler<Linear, Fixed, 2>,
    input: &[i16],
    output: &mut [i16],
) -> usize {
    match samples_using::<Fixed, i16>(input, 2) {
        Ok(stream) => match stream.resample(resampler) {
            Ok(mut stream) => stream.write_samples(output),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_resample_linear_i16(
    resampler: &mut Resampler<Linear, Fixed, 2>,
    input: &[i16],
    output: &mut [i16],
) -> usize {
    resampler.process(input, output).produced
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_resample_sinc_i16(
    resampler: &mut Resampler<Sinc, Fixed, 2>,
    input: &[i16],
    output: &mut [i16],
) -> usize {
    resampler.process(input, output).produced
}

#[inline(never)]
#[no_mangle]
pub fn probe_float_resample_linear_f32(
    resampler: &mut Resampler<Linear, Float, 2>,
    input: &[f32],
    output: &mut [f32],
) -> usize {
    resampler.process(input, output).produced
}

#[inline(never)]
#[no_mangle]
pub fn probe_float_resample_sinc_f32(
    resampler: &mut Resampler<Sinc, Float, 2>,
    input: &[f32],
    output: &mut [f32],
) -> usize {
    resampler.process(input, output).produced
}

#[inline(never)]
#[no_mangle]
pub fn probe_dither_i32_to_i16(input: &[i32], output: &mut [i16], state: &mut u32) -> usize {
    convert_dithered(input, output, state)
}

#[inline(never)]
#[no_mangle]
pub fn probe_dither_f32_to_i16(input: &[f32], output: &mut [i16], state: &mut u32) -> usize {
    convert_dithered(input, output, state)
}

#[inline(never)]
#[no_mangle]
pub fn probe_fixed_stream_dither_i16(input: &[i32], output: &mut [i16], seed: u32) -> usize {
    match samples_using::<Fixed, i32>(input, 2) {
        Ok(stream) => stream.dither(Format::I16, seed).write_samples(output),
        Err(_) => 0,
    }
}
