#![no_std]
#![no_main]

use core::hint::black_box;
use core::panic::PanicInfo;

use nanoaudio_adpcm::{ImaAdpcm, MsAdpcm};
use nanoaudio_analysis::{
    BallisticMeter, Ballistics, BandAnalyzer, Dtmf, LevelMeter, Spectrum, StereoMeter,
    VoiceActivityDetector, analyze_using,
};
use nanoaudio_core::Format;
use nanoaudio_core::codec::{Decoder, Encoder};
use nanoaudio_dsp::{
    Fixed, Float, Interpolation, Linear, Representation, Resampler, Sinc, Stream, Volume, convert,
    convert_dithered, mix_using, source_using,
};
use nanoaudio_flac::FlacDecoder;
use nanoaudio_g711::{G711, Law};
use nanoaudio_g722::{G722BitRate, G722Decoder, G722Encoder};
use nanoaudio_mp3::Mp3Decoder;
use nanoaudio_qoa::{QoaDecoder, QoaEncoder};
use nanoaudio_sbc::{SbcDecoder, SbcEncoder, SbcParameters};

const INPUT_SIZE: usize = 96 * 1024;
const OUTPUT_SIZE: usize = 192 * 1024;
const SCRATCH_SIZE: usize = 64 * 1024;
const FAILED: u32 = u32::MAX;
const BANDS: [f32; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

#[repr(C, align(8))]
pub struct Buffer<const SIZE: usize>([u8; SIZE]);

#[unsafe(no_mangle)]
pub static mut CASE: u32 = 0;
#[unsafe(no_mangle)]
pub static mut LENGTH: u32 = 0;
#[unsafe(no_mangle)]
pub static mut RESULT: u32 = 0;
#[unsafe(no_mangle)]
pub static mut MARK: u32 = 0;
#[unsafe(no_mangle)]
pub static mut INPUT: Buffer<INPUT_SIZE> = Buffer([0; INPUT_SIZE]);
#[unsafe(no_mangle)]
pub static mut OUTPUT: Buffer<OUTPUT_SIZE> = Buffer([0; OUTPUT_SIZE]);
#[unsafe(no_mangle)]
pub static mut SCRATCH: Buffer<SCRATCH_SIZE> = Buffer([0; SCRATCH_SIZE]);

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn begin() {
    unsafe { core::ptr::write_volatile(&raw mut MARK, 1) };
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn end() {
    unsafe { core::ptr::write_volatile(&raw mut MARK, 2) };
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn finished() -> ! {
    loop {
        unsafe { core::ptr::write_volatile(&raw mut MARK, 3) };
    }
}

fn input() -> &'static [u8] {
    let length = black_box(unsafe { LENGTH }) as usize;
    unsafe { core::slice::from_raw_parts((&raw const INPUT).cast::<u8>(), length.min(INPUT_SIZE)) }
}

fn input_samples() -> &'static [i16] {
    let length = black_box(unsafe { LENGTH }) as usize / 2;
    unsafe {
        core::slice::from_raw_parts((&raw const INPUT).cast::<i16>(), length.min(INPUT_SIZE / 2))
    }
}

fn output() -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut((&raw mut OUTPUT).cast::<u8>(), OUTPUT_SIZE) }
}

fn output_samples() -> &'static mut [i16] {
    unsafe { core::slice::from_raw_parts_mut((&raw mut OUTPUT).cast::<i16>(), OUTPUT_SIZE / 2) }
}

fn scratch_floats() -> &'static mut [f32] {
    unsafe { core::slice::from_raw_parts_mut((&raw mut SCRATCH).cast::<f32>(), SCRATCH_SIZE / 4) }
}

fn decode<D: Decoder<Sample = i16>>(mut decoder: D) -> usize {
    let input = input();
    let output = output_samples();
    let mut consumed = 0;
    let mut produced = 0;
    let mut finished = false;

    begin();
    loop {
        let progress = match decoder.decode(&input[consumed..], &mut output[produced..]) {
            Ok(progress) => progress,
            Err(_) if finished => break,
            Err(_) => return FAILED as usize,
        };
        consumed += progress.consumed;
        produced += progress.produced;

        if progress.is_stalled() {
            if finished {
                break;
            }
            decoder.finish();
            finished = true;
        }
    }
    end();

    produced * 2
}

fn encode<E: Encoder<Sample = i16>>(mut encoder: E) -> usize {
    let input = input_samples();
    let output = output();
    let mut consumed = 0;
    let mut produced = 0;
    let mut flushed = false;

    begin();
    loop {
        let Ok(progress) = encoder.encode(&input[consumed..], &mut output[produced..]) else {
            return FAILED as usize;
        };
        consumed += progress.consumed;
        produced += progress.produced;

        if progress.is_stalled() {
            if flushed {
                break;
            }
            encoder.flush();
            flushed = true;
        }
    }
    end();

    produced
}

fn volume<R: Representation>() -> usize {
    let input = input_samples();
    let output = &mut output_samples()[..input.len()];
    output.copy_from_slice(input);

    begin();
    Volume::from_percent(80).apply_to_using::<R, i16>(black_box(output));
    end();

    input.len() * 2
}

fn mix<R: Representation>() -> usize {
    let input = input_samples();
    let output = &mut output_samples()[..input.len()];
    for (output, input) in output.iter_mut().zip(input.iter().rev()) {
        *output = *input;
    }

    begin();
    let mixed = mix_using::<R, i16>(black_box(output), input, Volume::from_percent(80));
    end();

    mixed * 2
}

fn pipeline<R: Representation>() -> usize {
    let music = input();
    let voice = &music[..music.len() / 2];
    let output = output();

    begin();
    let frames = match (
        source_using::<R>(music, Format::I16, 2),
        source_using::<R>(voice, Format::I16, 1),
    ) {
        (Ok(music), Ok(voice)) => match voice.to_channels(2) {
            Ok(voice) => music
                .volume(Volume::from_percent(80))
                .mix(voice)
                .write_bytes(output, Format::I16),
            Err(_) => return FAILED as usize,
        },
        _ => return FAILED as usize,
    };
    end();

    frames * 4
}

fn resample<K: Interpolation<R, 2>, R: Representation>(input_rate: u32, output_rate: u32) -> usize {
    let Ok(mut resampler) = Resampler::<K, R>::new(2, input_rate, output_rate) else {
        return FAILED as usize;
    };
    let input = input_samples();
    let output = output_samples();

    begin();
    let progress = resampler.process(input, output);
    end();

    progress.produced * 2
}

fn dither() -> usize {
    let input = input_samples();
    let floats = &mut scratch_floats()[..input.len()];
    convert::<i16, f32>(input, floats);
    let mut state = 1;

    begin();
    let converted = convert_dithered::<f32, i16>(floats, output_samples(), &mut state);
    end();

    converted * 2
}

fn analyze<R: Representation, A: nanoaudio_dsp::Analyzer<R>>(mut analyzer: A) -> usize {
    let input = input_samples();

    begin();
    let result = analyze_using::<R, _, _>(input, 2, &mut analyzer);
    end();

    black_box(&analyzer);
    match result {
        Ok(frames) => frames,
        Err(_) => FAILED as usize,
    }
}

fn run(case: u32) -> usize {
    let encoded = black_box(unsafe { LENGTH }) / 4;
    match case {
        0 => G711::new(Law::Mu, 8_000, 1).map_or(FAILED as usize, encode),
        1 => G711::new(Law::Mu, 8_000, 1).map_or(FAILED as usize, decode),
        2 => G711::new(Law::A, 8_000, 1).map_or(FAILED as usize, encode),
        3 => G711::new(Law::A, 8_000, 1).map_or(FAILED as usize, decode),
        4 => ImaAdpcm::new(16_000, 1, 256).map_or(FAILED as usize, encode),
        5 => ImaAdpcm::new(16_000, 1, 256).map_or(FAILED as usize, decode),
        6 => MsAdpcm::new(44_100, 2, 2_048).map_or(FAILED as usize, encode),
        7 => MsAdpcm::new(44_100, 2, 2_048).map_or(FAILED as usize, decode),
        8 => QoaEncoder::<i16>::new(44_100, 2, encoded).map_or(FAILED as usize, encode),
        9 => decode(QoaDecoder::<i16>::new()),
        10 => encode(G722Encoder::new()),
        11 => decode(G722Decoder::new(G722BitRate::Rate64000)),
        12 => SbcEncoder::new(SbcParameters::HIGH_QUALITY).map_or(FAILED as usize, encode),
        13 => decode(SbcDecoder::new()),
        14 => SbcEncoder::new(SbcParameters::MSBC).map_or(FAILED as usize, encode),
        15 => decode(SbcDecoder::new()),
        16 => decode(FlacDecoder::<i16>::new()),
        17 => decode(Mp3Decoder::<i16>::new()),
        32 => {
            let input = input_samples();
            let floats = &mut scratch_floats()[..input.len()];
            begin();
            let converted = convert::<i16, f32>(input, black_box(floats));
            end();
            converted * 2
        }
        33 => dither(),
        34 => volume::<Fixed>(),
        35 => volume::<Float>(),
        36 => mix::<Fixed>(),
        37 => mix::<Float>(),
        38 => pipeline::<Fixed>(),
        39 => pipeline::<Float>(),
        40 => resample::<Linear, Fixed>(44_100, 48_000),
        41 => resample::<Linear, Float>(44_100, 48_000),
        42 => resample::<Sinc, Fixed>(44_100, 48_000),
        43 => resample::<Sinc, Float>(44_100, 48_000),
        44 => resample::<Sinc, Fixed>(48_000, 16_000),
        45 => resample::<Sinc, Float>(48_000, 16_000),
        64 => analyze::<Fixed, _>(LevelMeter::<Fixed>::new(441)),
        65 => analyze::<Float, _>(LevelMeter::<Float>::new(441)),
        66 => analyze::<Fixed, _>(BallisticMeter::<Fixed>::new(Ballistics::PPM_EBU, 44_100)),
        67 => analyze::<Float, _>(BallisticMeter::<Float>::new(Ballistics::PPM_EBU, 44_100)),
        68 => analyze::<Fixed, _>(BandAnalyzer::<Fixed, 10>::new(44_100, BANDS, 1.0)),
        69 => analyze::<Float, _>(BandAnalyzer::<Float, 10>::new(44_100, BANDS, 1.0)),
        70 => analyze::<Fixed, _>(Dtmf::<Fixed>::new(44_100)),
        71 => analyze::<Float, _>(Dtmf::<Float>::new(44_100)),
        72 => analyze::<Fixed, _>(StereoMeter::<Fixed>::new(44_100)),
        73 => analyze::<Float, _>(StereoMeter::<Float>::new(44_100)),
        74 => analyze::<Fixed, _>(VoiceActivityDetector::new(44_100)),
        75 => analyze::<Fixed, _>(Spectrum::<Fixed, 1024>::new(44_100)),
        76 => analyze::<Float, _>(Spectrum::<Float, 1024>::new(44_100)),
        77 => analyze::<Fixed, _>(Spectrum::<Fixed, 256>::new(44_100)),
        78 => analyze::<Float, _>(Spectrum::<Float, 256>::new(44_100)),
        _ => FAILED as usize,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    #[cfg(all(target_arch = "arm", target_abi = "eabihf"))]
    unsafe {
        let cpacr = 0xE000_ED88 as *mut u32;
        core::ptr::write_volatile(cpacr, core::ptr::read_volatile(cpacr) | (0xF << 20));
    }
    let result = run(black_box(unsafe { CASE }));
    unsafe { core::ptr::write_volatile(&raw mut RESULT, result as u32) };
    begin();
    end();
    finished()
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    unsafe { core::ptr::write_volatile(&raw mut RESULT, FAILED) };
    begin();
    end();
    finished()
}
