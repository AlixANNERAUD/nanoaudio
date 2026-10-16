use std::alloc::{GlobalAlloc, Layout, System};
use std::f64::consts::TAU;
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use dasp::Signal;
use dasp::sample::Sample as _;
use nanoaudio_adpcm::{ImaAdpcm, MsAdpcm, MsAdpcmCoefficients};
use nanoaudio_analysis::Spectrum;
use nanoaudio_core::codec::{Decoder, Encoder};
use nanoaudio_dsp::{
    Fixed, Float, Interpolation, Linear, Representation, Resampler, Sinc, Volume, analyze_using,
    convert, mix_using,
};
use nanoaudio_flac::FlacDecoder;
use nanoaudio_mp3::Mp3Decoder;
use nanoaudio_qoa::{QoaDecoder, QoaEncoder};
use nanoaudio_wav::{WavDecoder, WavHeader};
use realfft::RealFftPlanner;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, Fft, FixedAsync, FixedSync, PolynomialDegree,
    SincInterpolationParameters, WindowFunction,
};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::codecs::{decode_all, encode_all};
use crate::emit;
use crate::harness::{Settings, measure};
use crate::signal;

pub struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let current = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(current, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

fn peak_heap<F: FnMut() -> usize>(function: &mut F) -> usize {
    let start = CURRENT.load(Ordering::Relaxed);
    PEAK.store(start, Ordering::Relaxed);
    std::hint::black_box(function());
    PEAK.load(Ordering::Relaxed) - start
}

type Decode = Box<dyn FnMut(&[u8], &mut [i16]) -> usize>;

struct Library {
    name: &'static str,
    state: Option<usize>,
    decode: Decode,
}

fn library(name: &'static str, state: Option<usize>, decode: Decode) -> Library {
    Library {
        name,
        state,
        decode,
    }
}

fn agreement(reference: &[i16], other: &[i16], channels: usize) -> (String, Option<f64>) {
    let frames = reference.len() / channels;
    let others = other.len() / channels;
    let window = 8_192.min(frames / 4);
    let start = 4_096.min(frames / 4);
    let mut best = (f64::MIN, 0isize);

    for lag in -4_096isize..=4_096 {
        let mut correlation = 0.0;

        for frame in start..start + window {
            let shifted = frame as isize + lag;

            if shifted < 0 || shifted as usize >= others {
                continue;
            }

            correlation +=
                reference[frame * channels] as f64 * other[shifted as usize * channels] as f64;
        }

        if correlation > best.0 {
            best = (correlation, lag);
        }
    }

    let lag = best.1;
    let mut signal = 0.0;
    let mut noise = 0.0;
    let mut largest = 0i32;

    for frame in 0..frames {
        let shifted = frame as isize + lag;

        if shifted < 0 || shifted as usize >= others {
            continue;
        }

        for channel in 0..channels {
            let original = reference[frame * channels + channel];
            let value = other[shifted as usize * channels + channel];
            let difference = original as i32 - value as i32;
            signal += (original as f64).powi(2);
            noise += (difference as f64).powi(2);
            largest = largest.max(difference.abs());
        }
    }

    let offset = if lag == 0 {
        String::new()
    } else {
        format!(", offset by {lag} frames")
    };

    if largest == 0 {
        (format!("bit-exact{offset}"), None)
    } else {
        let snr = 10.0 * (signal / noise).log10();
        (
            format!("off by up to {largest} LSB{offset}"),
            Some(snr),
        )
    }
}

fn run_decoders(
    settings: Settings,
    group: &str,
    configuration: &str,
    encoded: &[u8],
    reference: &[i16],
    exact_reference: bool,
    channels: usize,
    libraries: Vec<Library>,
) {
    let samples = reference.len();
    let mut output = vec![0i16; samples * 2 + 65_536];
    let mut first: Option<Vec<i16>> = None;

    for mut library in libraries {
        let heap = peak_heap(&mut || (library.decode)(encoded, &mut output));
        let produced = (library.decode)(encoded, &mut output);
        let decoded = output[..produced].to_vec();
        let (agreement, snr) = match (&first, exact_reference) {
            (_, true) => agreement(reference, &decoded, channels),
            (Some(first), false) => agreement(first, &decoded, channels),
            (None, false) => ("reference".to_string(), None),
        };

        if first.is_none() {
            first = Some(decoded);
        }

        let measurement = measure(settings, || (library.decode)(encoded, &mut output));
        emit(vec![
            ("kind", "decode".into()),
            ("group", group.into()),
            ("configuration", configuration.into()),
            ("library", library.name.into()),
            ("samples", samples.into()),
            ("produced", produced.into()),
            ("median_ns", measurement.median.into()),
            ("low_ns", measurement.low.into()),
            ("high_ns", measurement.high.into()),
            ("state_bytes", library.state.into()),
            ("heap_bytes", heap.into()),
            ("agreement", agreement.into()),
            ("snr_db", snr.map_or(f64::NAN, |value| value).into()),
        ]);
    }
}

fn symphonia(extension: &'static str) -> Decode {
    Box::new(move |input, output| {
        let source = MediaSourceStream::new(Box::new(Cursor::new(input)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension(extension);
        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                source,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .expect("symphonia cannot read the file");
        let track = format
            .default_track(TrackType::Audio)
            .expect("no audio track");
        let track_id = track.id;
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(
                track.codec_params.as_ref().unwrap().audio().unwrap(),
                &AudioDecoderOptions::default(),
            )
            .expect("symphonia has no decoder for the file");
        let mut produced = 0;

        while let Ok(Some(packet)) = format.next_packet() {
            if packet.track_id != track_id {
                continue;
            }

            if let Ok(buffer) = decoder.decode(&packet) {
                let count = buffer.samples_interleaved();
                buffer.copy_to_slice_interleaved(&mut output[produced..produced + count]);
                produced += count;
            }
        }

        produced
    })
}

fn nanoaudio<D: Decoder<Sample = i16> + 'static>(make: fn() -> D) -> Decode {
    Box::new(move |input, output| decode_all(&mut make(), input, output))
}

fn nanoaudio_wav() -> Decode {
    Box::new(|input, output| {
        let header = WavHeader::parse(input).unwrap();
        let mut decoder = WavDecoder::<i16>::new(&header).unwrap();
        let end = (header.data_offset + header.data_size as usize).min(input.len());
        decode_all(&mut decoder, &input[header.data_offset..end], output)
    })
}

fn nanomp3_raw() -> Decode {
    Box::new(|input, output| {
        let mut decoder = nanomp3::Decoder::new();
        let mut consumed = 0;
        let mut produced = 0;

        while consumed < input.len() {
            let (bytes, result) =
                decoder.decode::<i16>(&input[consumed..], &mut output[produced..]);

            if bytes == 0 {
                break;
            }

            consumed += bytes;

            if let Ok(info) = result {
                produced += info.samples_produced * info.channels.num() as usize;
            }
        }

        produced
    })
}

fn minimp3_raw() -> Decode {
    use minimp3::ffi;

    Box::new(|input, output| {
        let mut decoder: ffi::mp3dec_t = unsafe { std::mem::zeroed() };
        unsafe { ffi::mp3dec_init(&mut decoder) };
        let mut consumed = 0;
        let mut produced = 0;

        while consumed < input.len() {
            let mut info: ffi::mp3dec_frame_info_t = unsafe { std::mem::zeroed() };
            let rest = &input[consumed..];
            let target = &mut output[produced..];
            assert!(target.len() >= ffi::MINIMP3_MAX_SAMPLES_PER_FRAME as usize);
            let frames = unsafe {
                ffi::mp3dec_decode_frame(
                    &mut decoder,
                    rest.as_ptr(),
                    rest.len() as i32,
                    target.as_mut_ptr(),
                    &mut info,
                )
            };

            if info.frame_bytes == 0 {
                break;
            }

            consumed += info.frame_bytes as usize;
            produced += frames as usize * info.channels as usize;
        }

        produced
    })
}

fn claxon() -> Decode {
    Box::new(|input, output| {
        let mut reader = claxon::FlacReader::new(input).unwrap();
        let mut blocks = reader.blocks();
        let mut buffer = Vec::new();
        let mut produced = 0;

        while let Some(block) = blocks.read_next_or_eof(buffer).unwrap() {
            for (left, right) in block.stereo_samples() {
                output[produced] = left as i16;
                output[produced + 1] = right as i16;
                produced += 2;
            }

            buffer = block.into_buffer();
        }

        produced
    })
}

fn hound() -> Decode {
    Box::new(|input, output| {
        let mut reader = hound::WavReader::new(input).unwrap();
        let mut produced = 0;

        for (output, sample) in output.iter_mut().zip(reader.samples::<i16>()) {
            *output = sample.unwrap();
            produced += 1;
        }

        produced
    })
}

fn qoaudio() -> Decode {
    Box::new(|input, output| {
        let decoder = qoaudio::QoaDecoder::new(input).unwrap();
        let mut produced = 0;

        for item in decoder {
            if let qoaudio::QoaItem::Sample(sample) = item.unwrap() {
                output[produced] = sample;
                produced += 1;
            }
        }

        produced
    })
}

fn bytes(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

fn wav(format: u16, channels: u16, sample_rate: u32, block_align: u16, bits: u16, extra: &[u8], fact: Option<u32>, data: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::new();
    let bytes_per_second = match format {
        1 => sample_rate * block_align as u32,
        _ => sample_rate * block_align as u32 / 2,
    };
    chunk.extend_from_slice(&format.to_le_bytes());
    chunk.extend_from_slice(&channels.to_le_bytes());
    chunk.extend_from_slice(&sample_rate.to_le_bytes());
    chunk.extend_from_slice(&bytes_per_second.to_le_bytes());
    chunk.extend_from_slice(&block_align.to_le_bytes());
    chunk.extend_from_slice(&bits.to_le_bytes());

    if format != 1 {
        chunk.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        chunk.extend_from_slice(extra);
    }

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
    file.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
    file.extend_from_slice(&chunk);

    if let Some(frames) = fact {
        file.extend_from_slice(b"fact");
        file.extend_from_slice(&4u32.to_le_bytes());
        file.extend_from_slice(&frames.to_le_bytes());
    }

    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(data);

    if data.len() % 2 == 1 {
        file.push(0);
    }

    let size = (file.len() - 8) as u32;
    file[4..8].copy_from_slice(&size.to_le_bytes());
    file
}

fn decoders(settings: Settings, data: Option<&Path>, filter: Option<&str>) {
    let source = signal::generate(44_100, 2);
    let wanted = |key: &str| filter.is_none_or(|filter| filter.split(',').any(|item| item == key));

    if let Some(data) = data {
        if wanted("mp3") {
            if let Ok(encoded) = std::fs::read(data.join("signal.mp3")) {
                let reference = {
                    let mut output = vec![0i16; source.len() * 2];
                    let produced = (nanoaudio(Mp3Decoder::<i16>::new))(&encoded, &mut output);
                    output.truncate(produced);
                    output
                };
                run_decoders(
                    settings,
                    "MP3",
                    "44.1 kHz stereo, 128 kbit/s CBR",
                    &encoded,
                    &reference,
                    false,
                    2,
                    vec![
                        library(
                            "nanoaudio-mp3",
                            Some(size_of::<Mp3Decoder<i16>>()),
                            nanoaudio(Mp3Decoder::<i16>::new),
                        ),
                        library(
                            "nanomp3 0.2 (frame decoder)",
                            Some(size_of::<nanomp3::Decoder>()),
                            nanomp3_raw(),
                        ),
                        library(
                            "minimp3 (C, through minimp3 0.6)",
                            Some(size_of::<minimp3::ffi::mp3dec_t>()),
                            minimp3_raw(),
                        ),
                        library("symphonia 0.6", None, symphonia("mp3")),
                    ],
                );
            }
        }

        if wanted("flac") {
            if let Ok(encoded) = std::fs::read(data.join("signal.flac")) {
                run_decoders(
                    settings,
                    "FLAC",
                    "44.1 kHz stereo 16-bit, level 5",
                    &encoded,
                    &source,
                    true,
                    2,
                    vec![
                        library(
                            "nanoaudio-flac",
                            Some(size_of::<FlacDecoder<i16>>()),
                            nanoaudio(FlacDecoder::<i16>::new),
                        ),
                        library("claxon 0.4", None, claxon()),
                        library("symphonia 0.6", None, symphonia("flac")),
                    ],
                );
            }
        }
    }

    if wanted("wav") {
        let file = wav(1, 2, 44_100, 4, 16, &[], None, &bytes(&source));
        run_decoders(
            settings,
            "WAV",
            "44.1 kHz stereo 16-bit PCM",
            &file,
            &source,
            true,
            2,
            vec![
                library(
                    "nanoaudio-wav",
                    Some(size_of::<WavDecoder<i16>>()),
                    nanoaudio_wav(),
                ),
                library("hound 3.5", None, hound()),
                library("symphonia 0.6", None, symphonia("wav")),
            ],
        );
    }

    if wanted("qoa") {
        let mut encoded = vec![0u8; source.len() * 2];
        let length = encode_all(
            &mut QoaEncoder::<i16>::new(44_100, 2, (source.len() / 2) as u32).unwrap(),
            &source,
            &mut encoded,
        );
        encoded.truncate(length);
        let reference = {
            let mut output = vec![0i16; source.len() * 2];
            let produced = (nanoaudio(QoaDecoder::<i16>::new))(&encoded, &mut output);
            output.truncate(produced);
            output
        };
        run_decoders(
            settings,
            "QOA",
            "44.1 kHz stereo",
            &encoded,
            &reference,
            false,
            2,
            vec![
                library(
                    "nanoaudio-qoa",
                    Some(size_of::<QoaDecoder<i16>>()),
                    nanoaudio(QoaDecoder::<i16>::new),
                ),
                library(
                    "qoaudio 0.7",
                    Some(size_of::<qoaudio::QoaDecoder<&[u8]>>()),
                    qoaudio(),
                ),
            ],
        );
    }

    if wanted("adpcm") {
        let ima = ImaAdpcm::new(44_100, 2, 2_048).unwrap();
        let frames_per_block = Decoder::minimum_output(&ima) / 2;
        let block = Encoder::minimum_input(&ima);
        let trimmed = &source[..source.len() - source.len() % block];
        let mut encoded = vec![0u8; trimmed.len()];
        let length = encode_all(&mut ima.clone(), trimmed, &mut encoded);
        encoded.truncate(length);
        let file = wav(
            0x11,
            2,
            44_100,
            2_048,
            4,
            &(frames_per_block as u16).to_le_bytes(),
            Some((trimmed.len() / 2) as u32),
            &encoded,
        );
        let reference = {
            let mut output = vec![0i16; trimmed.len() + 65_536];
            let produced = (nanoaudio_wav())(&file, &mut output);
            output.truncate(produced);
            output
        };
        run_decoders(
            settings,
            "IMA ADPCM",
            "WAV, 44.1 kHz stereo, 2048-byte blocks",
            &file,
            &reference,
            false,
            2,
            vec![
                library(
                    "nanoaudio-wav",
                    Some(size_of::<WavDecoder<i16>>()),
                    nanoaudio_wav(),
                ),
                library("symphonia 0.6", None, symphonia("wav")),
            ],
        );

        let ms = MsAdpcm::new(44_100, 2, 2_048).unwrap();
        let frames_per_block = ms.samples_per_block();
        let block = Encoder::minimum_input(&ms);
        let trimmed = &source[..source.len() - source.len() % block];
        let mut encoded = vec![0u8; trimmed.len()];
        let length = encode_all(&mut ms.clone(), trimmed, &mut encoded);
        encoded.truncate(length);
        let mut extra = Vec::new();
        extra.extend_from_slice(&(frames_per_block as u16).to_le_bytes());
        let pairs = MsAdpcmCoefficients::STANDARD;
        extra.extend_from_slice(&(pairs.as_slice().len() as u16).to_le_bytes());

        for pair in pairs.as_slice() {
            extra.extend_from_slice(&pair[0].to_le_bytes());
            extra.extend_from_slice(&pair[1].to_le_bytes());
        }

        let file = wav(
            0x02,
            2,
            44_100,
            2_048,
            4,
            &extra,
            Some((trimmed.len() / 2) as u32),
            &encoded,
        );
        let reference = {
            let mut output = vec![0i16; trimmed.len() + 65_536];
            let produced = (nanoaudio_wav())(&file, &mut output);
            output.truncate(produced);
            output
        };
        run_decoders(
            settings,
            "MS ADPCM",
            "WAV, 44.1 kHz stereo, 2048-byte blocks",
            &file,
            &reference,
            false,
            2,
            vec![
                library(
                    "nanoaudio-wav",
                    Some(size_of::<WavDecoder<i16>>()),
                    nanoaudio_wav(),
                ),
                library("symphonia 0.6", None, symphonia("wav")),
            ],
        );
    }
}

type Process = Box<dyn FnMut(&[f32], &mut [f32]) -> usize>;

struct Converter {
    library: &'static str,
    setting: String,
    state: Option<usize>,
    fixed: bool,
    make: Box<dyn Fn(u32, u32) -> Process>,
}

fn ours_float<K: Interpolation<Float, 2> + 'static>(input_rate: u32, output_rate: u32) -> Process {
    let mut resampler = Resampler::<K, Float>::new(2, input_rate, output_rate).unwrap();

    Box::new(move |input, output| {
        resampler.reset();
        resampler.process(input, output).produced
    })
}

fn ours_fixed<K: Interpolation<Fixed, 2> + 'static>(input_rate: u32, output_rate: u32) -> Process {
    let mut resampler = Resampler::<K, Fixed>::new(2, input_rate, output_rate).unwrap();
    let mut samples = Vec::new();
    let mut produced_samples = Vec::new();

    Box::new(move |input, output| {
        samples.resize(input.len(), 0i16);
        produced_samples.resize(output.len(), 0i16);
        convert::<f32, i16>(input, &mut samples);
        resampler.reset();
        let produced = resampler.process(&samples, &mut produced_samples).produced;
        convert::<i16, f32>(&produced_samples[..produced], output);
        produced
    })
}

fn ours_fixed_native<K: Interpolation<Fixed, 2> + 'static>(
    input_rate: u32,
    output_rate: u32,
) -> Box<dyn FnMut(&[i16], &mut [i16]) -> usize> {
    let mut resampler = Resampler::<K, Fixed>::new(2, input_rate, output_rate).unwrap();

    Box::new(move |input, output| {
        resampler.reset();
        resampler.process(input, output).produced
    })
}

fn rubato_process(mut resampler: Box<dyn rubato::Resampler<f32>>) -> Process {
    Box::new(move |input, output| {
        resampler.reset();
        let frames = input.len() / 2;
        let needed = resampler.process_all_needed_output_len(frames);
        assert!(output.len() / 2 >= needed);
        let input = InterleavedSlice::new(input, 2, frames).unwrap();
        let capacity = output.len() / 2;
        let mut output = InterleavedSlice::new_mut(output, 2, capacity).unwrap();
        let (_, written) = resampler
            .process_all_into_buffer(&input, &mut output, frames, None)
            .unwrap();
        written * 2
    })
}

fn rubato_sinc(
    length: usize,
    window: WindowFunction,
    cutoff: Option<f32>,
) -> Box<dyn Fn(u32, u32) -> Process> {
    Box::new(move |input_rate, output_rate| {
        let mut parameters = SincInterpolationParameters::new(length, window);
        parameters.f_cutoff = cutoff;
        rubato_process(Box::new(
            Async::<f32>::new_sinc(
                output_rate as f64 / input_rate as f64,
                1.0,
                &parameters,
                1_024,
                2,
                FixedAsync::Input,
            )
            .unwrap(),
        ))
    })
}

fn rubato_poly(degree: PolynomialDegree) -> Box<dyn Fn(u32, u32) -> Process> {
    Box::new(move |input_rate, output_rate| {
        rubato_process(Box::new(
            Async::<f32>::new_poly(
                output_rate as f64 / input_rate as f64,
                1.0,
                degree,
                1_024,
                2,
                FixedAsync::Input,
            )
            .unwrap(),
        ))
    })
}

fn rubato_fft() -> Box<dyn Fn(u32, u32) -> Process> {
    Box::new(|input_rate, output_rate| {
        rubato_process(Box::new(
            Fft::<f32>::new(
                input_rate as usize,
                output_rate as usize,
                1_024,
                2,
                FixedSync::Both,
            )
            .unwrap(),
        ))
    })
}

fn dasp_frames(input: &[f32]) -> impl Signal<Frame = [f32; 2]> + '_ {
    dasp::signal::from_iter(input.chunks_exact(2).map(|pair| [pair[0], pair[1]]))
}

fn dasp_write<S: Signal<Frame = [f32; 2]>>(signal: S, output: &mut [f32], frames: usize) -> usize {
    let mut produced = 0;

    for (target, frame) in output.chunks_exact_mut(2).zip(signal.take(frames)) {
        target.copy_from_slice(&frame);
        produced += 2;
    }

    produced
}

fn dasp_linear() -> Box<dyn Fn(u32, u32) -> Process> {
    Box::new(|input_rate, output_rate| {
        Box::new(move |input, output| {
            let mut signal = dasp_frames(input);
            let first = signal.next();
            let second = signal.next();
            let interpolator = dasp::interpolate::linear::Linear::new(first, second);
            let frames = (input.len() / 2) * output_rate as usize / input_rate as usize;
            dasp_write(
                signal.from_hz_to_hz(interpolator, input_rate as f64, output_rate as f64),
                output,
                frames,
            )
        })
    })
}

fn dasp_sinc(taps: usize) -> Box<dyn Fn(u32, u32) -> Process> {
    Box::new(move |input_rate, output_rate| {
        Box::new(move |input, output| {
            let signal = dasp_frames(input);
            let ring = dasp::ring_buffer::Fixed::from(vec![[0.0f32; 2]; taps]);
            let interpolator = dasp::interpolate::sinc::Sinc::new(ring);
            let frames = (input.len() / 2) * output_rate as usize / input_rate as usize;
            dasp_write(
                signal.from_hz_to_hz(interpolator, input_rate as f64, output_rate as f64),
                output,
                frames,
            )
        })
    })
}

fn converters() -> Vec<Converter> {
    vec![
        Converter {
            library: "nanoaudio-dsp",
            setting: "`Sinc`, `Float`".into(),
            state: Some(size_of::<Resampler<Sinc, Float>>()),
            fixed: false,
            make: Box::new(ours_float::<Sinc>),
        },
        Converter {
            library: "nanoaudio-dsp",
            setting: "`Sinc`, `Fixed`, i16 in and out".into(),
            state: Some(size_of::<Resampler<Sinc, Fixed>>()),
            fixed: true,
            make: Box::new(ours_fixed::<Sinc>),
        },
        Converter {
            library: "nanoaudio-dsp",
            setting: "`Linear`, `Float`".into(),
            state: Some(size_of::<Resampler<Linear, Float>>()),
            fixed: false,
            make: Box::new(ours_float::<Linear>),
        },
        Converter {
            library: "rubato 5.0",
            setting: "`Async` sinc, 96 taps, Blackman2, cutoff 0.9, cubic".into(),
            state: None,
            fixed: false,
            make: rubato_sinc(96, WindowFunction::Blackman2, Some(0.9)),
        },
        Converter {
            library: "rubato 5.0",
            setting: "`Async` sinc, 64 taps, BlackmanHarris2, cubic".into(),
            state: None,
            fixed: false,
            make: rubato_sinc(64, WindowFunction::BlackmanHarris2, None),
        },
        Converter {
            library: "rubato 5.0",
            setting: "`Async` sinc, 256 taps, BlackmanHarris2, cubic (default)".into(),
            state: None,
            fixed: false,
            make: rubato_sinc(256, WindowFunction::BlackmanHarris2, None),
        },
        Converter {
            library: "rubato 5.0",
            setting: "`Fft`, 1024-frame chunks".into(),
            state: None,
            fixed: false,
            make: rubato_fft(),
        },
        Converter {
            library: "rubato 5.0",
            setting: "`Async` polynomial, linear".into(),
            state: None,
            fixed: false,
            make: rubato_poly(PolynomialDegree::Linear),
        },
        Converter {
            library: "rubato 5.0",
            setting: "`Async` polynomial, cubic".into(),
            state: None,
            fixed: false,
            make: rubato_poly(PolynomialDegree::Cubic),
        },
        Converter {
            library: "dasp 0.11",
            setting: "`Linear`".into(),
            state: None,
            fixed: false,
            make: dasp_linear(),
        },
        Converter {
            library: "dasp 0.11",
            setting: "`Sinc`, 64 frames".into(),
            state: None,
            fixed: false,
            make: dasp_sinc(64),
        },
    ]
}

fn tone(rate: u32, frequency: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let value = (0.5 * (TAU * frequency * frame as f64 / rate as f64).sin()) as f32;
            [value, value]
        })
        .collect()
}

fn middle(output: &[f32], produced: usize) -> &[f32] {
    let frames = produced / 2;
    &output[frames / 4 * 2..frames * 3 / 4 * 2]
}

fn rms_db(samples: &[f32]) -> f64 {
    let power: f64 = samples
        .iter()
        .step_by(2)
        .map(|&value| (value as f64).powi(2))
        .sum::<f64>()
        / (samples.len() / 2) as f64;
    10.0 * (power / 0.125).log10()
}

fn residual_db(samples: &[f32], rate: u32, frequency: f64) -> f64 {
    let frames = samples.len() / 2;
    let (mut cosine, mut sine) = (0.0, 0.0);

    for frame in 0..frames {
        let phase = TAU * frequency * frame as f64 / rate as f64;
        cosine += samples[frame * 2] as f64 * phase.cos();
        sine += samples[frame * 2] as f64 * phase.sin();
    }

    cosine *= 2.0 / frames as f64;
    sine *= 2.0 / frames as f64;
    let mut noise = 0.0;

    for frame in 0..frames {
        let phase = TAU * frequency * frame as f64 / rate as f64;
        let fitted = cosine * phase.cos() + sine * phase.sin();
        noise += (samples[frame * 2] as f64 - fitted).powi(2);
    }

    10.0 * (noise / frames as f64 / 0.125).log10()
}

struct Response {
    stop_band: f64,
    flat: f64,
    half_power: f64,
    residual: f64,
}

fn response(converter: &Converter) -> Response {
    let mut output = vec![0f32; 96_000 * 2];
    let mut down = (converter.make)(48_000, 44_100);
    let mut stop_band = f64::MIN;

    for frequency in [22_300.0, 22_700.0, 23_100.0, 23_500.0, 23_900.0] {
        let input = tone(48_000, frequency, 24_000);
        let produced = down(&input, &mut output);
        stop_band = stop_band.max(rms_db(middle(&output, produced)));
    }

    let mut up = (converter.make)(44_100, 48_000);
    let mut flat = f64::NAN;
    let mut half_power = f64::NAN;

    for step in 0..=120 {
        let frequency = 10_000.0 + 100.0 * step as f64;
        let input = tone(44_100, frequency, 11_025);
        let produced = up(&input, &mut output);
        let level = rms_db(middle(&output, produced));

        if flat.is_nan() && level.abs() > 0.1 {
            flat = frequency - 100.0;
        }

        if half_power.is_nan() && level < -3.0 {
            half_power = frequency - 100.0;
            break;
        }
    }

    let input = tone(44_100, 10_000.0, 22_050);
    let produced = up(&input, &mut output);
    let residual = residual_db(middle(&output, produced), 48_000, 10_000.0);

    Response {
        stop_band,
        flat,
        half_power,
        residual,
    }
}

fn resamplers(settings: Settings) {
    let music = signal::generate(44_100, 2);
    let music_48 = signal::generate(48_000, 2);
    let mut floats = vec![0f32; music.len()];
    convert::<i16, f32>(&music, &mut floats);
    let mut floats_48 = vec![0f32; music_48.len()];
    convert::<i16, f32>(&music_48, &mut floats_48);
    let mut output = vec![0f32; music_48.len() * 2];
    let mut output_i16 = vec![0i16; music_48.len() * 2];

    for converter in converters() {
        let response = response(&converter);

        for (input_rate, output_rate, input, input_i16) in [
            (44_100, 48_000, &floats, &music),
            (48_000, 44_100, &floats_48, &music_48),
        ] {
            let (state, measurement) = match converter.state {
                Some(state) if converter.fixed => {
                    let mut process = ours_fixed_native::<Sinc>(input_rate, output_rate);
                    (
                        state,
                        measure(settings, || process(input_i16, &mut output_i16)),
                    )
                }
                Some(state) => {
                    let mut process = (converter.make)(input_rate, output_rate);
                    (state, measure(settings, || process(input, &mut output)))
                }
                None => {
                    let mut process = None;
                    let state = peak_heap(&mut || {
                        let mut made = (converter.make)(input_rate, output_rate);
                        let produced = made(input, &mut output);
                        process = Some(made);
                        produced
                    });
                    let mut process = process.unwrap();
                    (state, measure(settings, || process(input, &mut output)))
                }
            };
            emit(vec![
                ("kind", "resample".into()),
                ("library", converter.library.into()),
                ("setting", converter.setting.clone().into()),
                ("input_rate", (input_rate as usize).into()),
                ("output_rate", (output_rate as usize).into()),
                ("samples", input.len().into()),
                ("median_ns", measurement.median.into()),
                ("state_bytes", state.into()),
                ("stop_band_db", response.stop_band.into()),
                ("flat_hz", response.flat.into()),
                ("half_power_hz", response.half_power.into()),
                ("residual_db", response.residual.into()),
            ]);
        }
    }
}

fn hann(points: usize) -> Vec<f32> {
    (0..points)
        .map(|index| (0.5 - 0.5 * (TAU * index as f64 / points as f64).cos()) as f32)
        .collect()
}

fn spectrum_ours<R: nanoaudio_analysis::Spectral + Representation, const N: usize>(
    input: &[i16],
) -> usize
where
    nanoaudio_analysis::Points<N>: nanoaudio_analysis::Size,
    for<'a> &'a mut Spectrum<R, N>: nanoaudio_dsp::Analyzer<R>,
{
    let mut spectrum = Box::new(Spectrum::<R, N>::new(44_100));
    analyze_using::<R, _, _>(input, 1, &mut *spectrum).unwrap();
    std::hint::black_box(&spectrum);
    input.len() / N
}

fn microfft_power(block: &[i16], window: &[f32], power: &mut [f32]) {
    fn fill<const N: usize>(block: &[i16], window: &[f32]) -> [f32; N] {
        let mut buffer = [0f32; N];

        for ((target, &sample), &weight) in buffer.iter_mut().zip(block).zip(window) {
            *target = sample as f32 * (weight / 32_768.0);
        }

        buffer
    }

    let mut store = |spectrum: &[microfft::Complex32]| {
        for (target, value) in power.iter_mut().zip(spectrum) {
            *target = value.norm_sqr();
        }
    };

    if block.len() == 256 {
        let mut buffer = fill::<256>(block, window);
        store(microfft::real::rfft_256(&mut buffer));
    } else {
        let mut buffer = fill::<1024>(block, window);
        store(microfft::real::rfft_1024(&mut buffer));
    }
}

fn transforms(settings: Settings) {
    let voice = signal::generate(44_100, 1);

    for points in [256usize, 1_024] {
        let blocks = voice.len() / points;
        let window = hann(points);
        let mut rows: Vec<(&str, Box<dyn FnMut() -> usize>)> = Vec::new();
        let input = voice.clone();

        if points == 256 {
            rows.push((
                "nanoaudio-analysis `Spectrum`, `Fixed`",
                Box::new(move || spectrum_ours::<Fixed, 256>(&input)),
            ));
            let input = voice.clone();
            rows.push((
                "nanoaudio-analysis `Spectrum`, `Float`",
                Box::new(move || spectrum_ours::<Float, 256>(&input)),
            ));
        } else {
            rows.push((
                "nanoaudio-analysis `Spectrum`, `Fixed`",
                Box::new(move || spectrum_ours::<Fixed, 1024>(&input)),
            ));
            let input = voice.clone();
            rows.push((
                "nanoaudio-analysis `Spectrum`, `Float`",
                Box::new(move || spectrum_ours::<Float, 1024>(&input)),
            ));
        }

        {
            let input = voice.clone();
            let window = window.clone();
            let fft = RealFftPlanner::<f32>::new().plan_fft_forward(points);
            let mut buffer = fft.make_input_vec();
            let mut spectrum = fft.make_output_vec();
            let mut scratch = fft.make_scratch_vec();
            let mut power = vec![0f32; points / 2 + 1];
            rows.push((
                "realfft 3.5",
                Box::new(move || {
                    for block in input.chunks_exact(points) {
                        for ((target, &sample), &weight) in
                            buffer.iter_mut().zip(block).zip(&window)
                        {
                            *target = sample as f32 * (weight / 32_768.0);
                        }

                        fft.process_with_scratch(&mut buffer, &mut spectrum, &mut scratch)
                            .unwrap();

                        for (target, value) in power.iter_mut().zip(&spectrum) {
                            *target = value.norm_sqr();
                        }

                        std::hint::black_box(&power);
                    }

                    blocks
                }),
            ));
        }

        {
            let input = voice.clone();
            let window = window.clone();
            let mut power = vec![0f32; points / 2 + 1];
            rows.push((
                "microfft 0.6, the FFT inside `Float`",
                Box::new(move || {
                    for block in input.chunks_exact(points) {
                        microfft_power(block, &window, &mut power);
                        std::hint::black_box(&power);
                    }

                    blocks
                }),
            ));
        }

        {
            let input = voice.clone();
            let window = window.clone();
            let fft = FftPlanner::<f32>::new().plan_fft_forward(points);
            let mut buffer = vec![Complex::new(0f32, 0f32); points];
            let mut scratch = vec![Complex::new(0f32, 0f32); fft.get_inplace_scratch_len()];
            let mut power = vec![0f32; points / 2 + 1];
            rows.push((
                "rustfft 6.4 (complex FFT of the real block)",
                Box::new(move || {
                    for block in input.chunks_exact(points) {
                        for ((target, &sample), &weight) in
                            buffer.iter_mut().zip(block).zip(&window)
                        {
                            *target = Complex::new(sample as f32 * (weight / 32_768.0), 0.0);
                        }

                        fft.process_with_scratch(&mut buffer, &mut scratch);

                        for (target, value) in power.iter_mut().zip(&buffer) {
                            *target = value.norm_sqr();
                        }

                        std::hint::black_box(&power);
                    }

                    blocks
                }),
            ));
        }

        for (name, mut run) in rows {
            let measurement = measure(settings, &mut run);
            emit(vec![
                ("kind", "fft".into()),
                ("library", name.into()),
                ("points", points.into()),
                ("blocks", blocks.into()),
                ("median_ns", measurement.median.into()),
            ]);
        }
    }
}

const QUIETER: f32 = 0.8;
const LOUDER: f32 = 1.25;

fn sample_operations(settings: Settings) {
    let music = signal::generate(44_100, 2);
    let reversed: Vec<i16> = music.iter().rev().copied().collect();
    let mut floats = vec![0f32; music.len()];
    convert::<i16, f32>(&music, &mut floats);
    let samples = music.len();
    let mut rows: Vec<(&str, &str, Box<dyn FnMut() -> usize>)> = Vec::new();

    fn volume<R: Representation + 'static>(music: &[i16]) -> Box<dyn FnMut() -> usize> {
        let mut buffer = music.to_vec();
        let mut toggle = false;
        Box::new(move || {
            toggle = !toggle;
            let volume = if toggle {
                Volume::from_percent(80)
            } else {
                Volume::from_percent(125)
            };
            volume.apply_to_using::<R, i16>(&mut buffer);
            buffer.len()
        })
    }

    fn mix<R: Representation + 'static>(music: &[i16], reversed: &[i16]) -> Box<dyn FnMut() -> usize> {
        let base = music.to_vec();
        let mut buffer = music.to_vec();
        let input = reversed.to_vec();
        Box::new(move || {
            buffer.copy_from_slice(&base);
            mix_using::<R, i16>(&mut buffer, &input, Volume::from_percent(80))
        })
    }

    rows.push(("Volume, i16 slice", "nanoaudio-dsp, `Fixed`", volume::<Fixed>(&music)));
    rows.push(("Volume, i16 slice", "nanoaudio-dsp, `Float`", volume::<Float>(&music)));
    {
        let mut buffer = music.clone();
        let mut toggle = false;
        rows.push((
            "Volume, i16 slice",
            "dasp 0.11, `mul_amp`",
            Box::new(move || {
                toggle = !toggle;
                let amplitude = if toggle { QUIETER } else { LOUDER };
                for sample in buffer.iter_mut() {
                    *sample = sample.mul_amp(amplitude);
                }
                buffer.len()
            }),
        ));
    }
    rows.push((
        "Mix, i16 slice, second input at 80 % (includes a copy to reset the output)",
        "nanoaudio-dsp, `Fixed`",
        mix::<Fixed>(&music, &reversed),
    ));
    rows.push((
        "Mix, i16 slice, second input at 80 % (includes a copy to reset the output)",
        "nanoaudio-dsp, `Float`",
        mix::<Float>(&music, &reversed),
    ));
    {
        let base = music.clone();
        let mut buffer = music.clone();
        let input = reversed.clone();
        rows.push((
            "Mix, i16 slice, second input at 80 % (includes a copy to reset the output)",
            "dasp 0.11, `add_amp` + `mul_amp`",
            Box::new(move || {
                buffer.copy_from_slice(&base);
                for (output, &input) in buffer.iter_mut().zip(&input) {
                    *output = output.add_amp(input.mul_amp(QUIETER));
                }
                buffer.len()
            }),
        ));
    }
    {
        let input = music.clone();
        let mut output = vec![0f32; samples];
        rows.push((
            "i16 slice to f32 slice",
            "nanoaudio-dsp `convert`",
            Box::new(move || convert::<i16, f32>(&input, &mut output)),
        ));
    }
    {
        let input = music.clone();
        let mut output = vec![0f32; samples];
        rows.push((
            "i16 slice to f32 slice",
            "dasp 0.11, `to_sample`",
            Box::new(move || {
                for (output, &input) in output.iter_mut().zip(&input) {
                    *output = input.to_sample::<f32>();
                }
                output.len()
            }),
        ));
    }
    {
        let input = floats.clone();
        let mut output = vec![0i16; samples];
        rows.push((
            "f32 slice to i16 slice",
            "nanoaudio-dsp `convert`",
            Box::new(move || convert::<f32, i16>(&input, &mut output)),
        ));
    }
    {
        let input = floats.clone();
        let mut output = vec![0i16; samples];
        rows.push((
            "f32 slice to i16 slice",
            "dasp 0.11, `to_sample`",
            Box::new(move || {
                for (output, &input) in output.iter_mut().zip(&input) {
                    *output = input.to_sample::<i16>();
                }
                output.len()
            }),
        ));
    }

    for (operation, library, mut run) in rows {
        let measurement = measure(settings, &mut run);
        emit(vec![
            ("kind", "sample".into()),
            ("operation", operation.into()),
            ("library", library.into()),
            ("samples", samples.into()),
            ("median_ns", measurement.median.into()),
        ]);
    }
}

pub fn run(settings: Settings, data: Option<&Path>, filter: Option<&str>) {
    let wanted = |key: &str| filter.is_none_or(|filter| filter.split(',').any(|item| item == key));
    decoders(settings, data, filter);

    if wanted("resample") {
        resamplers(settings);
    }

    if wanted("fft") {
        transforms(settings);
    }

    if wanted("sample") {
        sample_operations(settings);
    }
}
