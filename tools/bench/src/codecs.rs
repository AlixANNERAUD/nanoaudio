use std::path::Path;

use nanoaudio_adpcm::{ImaAdpcm, MsAdpcm};
use nanoaudio_core::codec::{Decoder, Encoder};
use nanoaudio_flac::FlacDecoder;
use nanoaudio_g711::{G711, Law};
use nanoaudio_g722::{G722BitRate, G722Decoder, G722Encoder};
use nanoaudio_mp3::Mp3Decoder;
use nanoaudio_opus::OpusDecoder;
use nanoaudio_qoa::{QoaDecoder, QoaEncoder};
use nanoaudio_sbc::{SbcDecoder, SbcEncoder, SbcParameters};

use crate::signal::{self, SECONDS};

type DecodeFunction = Box<dyn Fn(&[u8], &mut [i16]) -> usize>;
type EncodeFunction = Box<dyn Fn(&[i16], &mut [u8]) -> usize>;
type Builder = Box<dyn Fn() -> Option<Case>>;

pub struct Case {
    pub codec: &'static str,
    pub configuration: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub lossless: bool,
    pub source: Option<Vec<i16>>,
    pub encoded: Vec<u8>,
    pub decode: DecodeFunction,
    pub encode: Option<EncodeFunction>,
}

impl Case {
    pub fn frames(&self) -> usize {
        match &self.source {
            Some(source) => source.len() / self.channels as usize,
            None => (self.sample_rate * SECONDS) as usize,
        }
    }

    pub fn decoded(&self) -> Vec<i16> {
        let mut output = vec![0i16; self.output_capacity()];
        let produced = (self.decode)(&self.encoded, &mut output);
        output.truncate(produced);
        output
    }

    pub fn output_capacity(&self) -> usize {
        self.frames() * self.channels as usize * 2 + 65_536
    }
}

pub fn decode_all<D: Decoder<Sample = i16>>(
    decoder: &mut D,
    input: &[u8],
    output: &mut [i16],
) -> usize {
    let mut consumed = 0;
    let mut produced = 0;
    let mut finished = false;

    loop {
        let progress = match decoder.decode(&input[consumed..], &mut output[produced..]) {
            Ok(progress) => progress,
            Err(_) if finished => return produced,
            Err(error) => panic!("decoding failed: {error:?}"),
        };
        consumed += progress.consumed;
        produced += progress.produced;

        if progress.is_stalled() {
            if finished {
                return produced;
            }

            decoder.finish();
            finished = true;
        }
    }
}

pub fn encode_all<E: Encoder<Sample = i16>>(
    encoder: &mut E,
    input: &[i16],
    output: &mut [u8],
) -> usize {
    let mut consumed = 0;
    let mut produced = 0;
    let mut flushed = false;

    loop {
        let progress = encoder
            .encode(&input[consumed..], &mut output[produced..])
            .expect("encoding failed");
        consumed += progress.consumed;
        produced += progress.produced;

        if progress.is_stalled() {
            if flushed {
                return produced;
            }

            encoder.flush();
            flushed = true;
        }
    }
}

fn trimmed(mut source: Vec<i16>, block: usize) -> Vec<i16> {
    source.truncate(source.len() - source.len() % block);
    source
}

fn encoded_case(
    codec: &'static str,
    configuration: &str,
    sample_rate: u32,
    channels: u8,
    block: usize,
    encode: EncodeFunction,
    decode: DecodeFunction,
) -> Case {
    let source = trimmed(signal::generate(sample_rate, channels), block);
    let mut buffer = vec![0u8; source.len() * 2 + 65_536];
    let length = encode(&source, &mut buffer);
    buffer.truncate(length);

    Case {
        codec,
        configuration: configuration.to_string(),
        sample_rate,
        channels,
        lossless: false,
        source: Some(source),
        encoded: buffer,
        decode,
        encode: Some(encode),
    }
}

fn g711(law: Law, codec: &'static str) -> Case {
    encoded_case(
        codec,
        "8 kHz mono",
        8_000,
        1,
        1,
        Box::new(move |input, output| {
            encode_all(&mut G711::new(law, 8_000, 1).unwrap(), input, output)
        }),
        Box::new(move |input, output| {
            decode_all(&mut G711::new(law, 8_000, 1).unwrap(), input, output)
        }),
    )
}

fn ima_adpcm(sample_rate: u32, channels: u8, block_align: u16, configuration: &str) -> Case {
    let codec = ImaAdpcm::new(sample_rate, channels, block_align).unwrap();

    encoded_case(
        "IMA ADPCM",
        configuration,
        sample_rate,
        channels,
        Encoder::minimum_input(&codec),
        Box::new(move |input, output| {
            encode_all(
                &mut ImaAdpcm::new(sample_rate, channels, block_align).unwrap(),
                input,
                output,
            )
        }),
        Box::new(move |input, output| {
            decode_all(
                &mut ImaAdpcm::new(sample_rate, channels, block_align).unwrap(),
                input,
                output,
            )
        }),
    )
}

fn ms_adpcm(sample_rate: u32, channels: u8, block_align: u16, configuration: &str) -> Case {
    let codec = MsAdpcm::new(sample_rate, channels, block_align).unwrap();

    encoded_case(
        "MS ADPCM",
        configuration,
        sample_rate,
        channels,
        Encoder::minimum_input(&codec),
        Box::new(move |input, output| {
            encode_all(
                &mut MsAdpcm::new(sample_rate, channels, block_align).unwrap(),
                input,
                output,
            )
        }),
        Box::new(move |input, output| {
            decode_all(
                &mut MsAdpcm::new(sample_rate, channels, block_align).unwrap(),
                input,
                output,
            )
        }),
    )
}

fn qoa(sample_rate: u32, channels: u8, configuration: &str) -> Case {
    encoded_case(
        "QOA",
        configuration,
        sample_rate,
        channels,
        1,
        Box::new(move |input, output| {
            let frames = (input.len() / channels as usize) as u32;
            encode_all(
                &mut QoaEncoder::<i16>::new(sample_rate, channels, frames).unwrap(),
                input,
                output,
            )
        }),
        Box::new(|input, output| decode_all(&mut QoaDecoder::<i16>::new(), input, output)),
    )
}

fn g722() -> Case {
    encoded_case(
        "G.722",
        "16 kHz mono, 64 kbit/s",
        16_000,
        1,
        2,
        Box::new(|input, output| encode_all(&mut G722Encoder::new(), input, output)),
        Box::new(|input, output| {
            decode_all(&mut G722Decoder::new(G722BitRate::Rate64000), input, output)
        }),
    )
}

fn sbc(parameters: SbcParameters, configuration: &str) -> Case {
    encoded_case(
        "SBC",
        configuration,
        parameters.sample_rate,
        parameters.channels(),
        parameters.frame_samples(),
        Box::new(move |input, output| {
            encode_all(&mut SbcEncoder::new(parameters).unwrap(), input, output)
        }),
        Box::new(|input, output| decode_all(&mut SbcDecoder::new(), input, output)),
    )
}

fn file_case(
    data: &Path,
    name: &str,
    sample_rate: u32,
    codec: &'static str,
    configuration: &str,
    lossless: bool,
    decode: DecodeFunction,
) -> Option<Case> {
    let encoded = std::fs::read(data.join(name)).ok()?;
    let source = std::fs::read(data.join(format!("signal_{sample_rate}_2.raw")))
        .ok()
        .map(|bytes| {
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect()
        });

    Some(Case {
        codec,
        configuration: configuration.to_string(),
        sample_rate,
        channels: 2,
        lossless,
        source,
        encoded,
        decode,
        encode: None,
    })
}

pub fn cases(data: Option<&Path>, filter: Option<&str>) -> Vec<Case> {
    let mut builders: Vec<(&str, Builder)> = vec![
        ("g711", Box::new(|| Some(g711(Law::Mu, "G.711 µ-law")))),
        ("g711", Box::new(|| Some(g711(Law::A, "G.711 A-law")))),
        (
            "ima",
            Box::new(|| Some(ima_adpcm(16_000, 1, 256, "16 kHz mono, 256-byte blocks"))),
        ),
        (
            "ima",
            Box::new(|| {
                Some(ima_adpcm(
                    44_100,
                    2,
                    2_048,
                    "44.1 kHz stereo, 2048-byte blocks",
                ))
            }),
        ),
        (
            "ms",
            Box::new(|| {
                Some(ms_adpcm(
                    44_100,
                    2,
                    2_048,
                    "44.1 kHz stereo, 2048-byte blocks",
                ))
            }),
        ),
        ("qoa", Box::new(|| Some(qoa(16_000, 1, "16 kHz mono")))),
        ("qoa", Box::new(|| Some(qoa(44_100, 2, "44.1 kHz stereo")))),
        ("g722", Box::new(|| Some(g722()))),
        (
            "sbc",
            Box::new(|| Some(sbc(SbcParameters::MSBC, "mSBC, 16 kHz mono (HFP)"))),
        ),
        (
            "sbc",
            Box::new(|| {
                Some(sbc(
                    SbcParameters {
                        bitpool: 35,
                        ..SbcParameters::HIGH_QUALITY
                    },
                    "44.1 kHz joint stereo, bitpool 35",
                ))
            }),
        ),
        (
            "sbc",
            Box::new(|| {
                Some(sbc(
                    SbcParameters::HIGH_QUALITY,
                    "44.1 kHz joint stereo, bitpool 53",
                ))
            }),
        ),
    ];

    if let Some(data) = data {
        let data = data.to_path_buf();
        let flac = data.clone();
        builders.push((
            "flac",
            Box::new(move || {
                file_case(
                    &flac,
                    "signal.flac",
                    44_100,
                    "FLAC",
                    "44.1 kHz stereo 16-bit, level 5",
                    true,
                    Box::new(|input, output| {
                        decode_all(&mut *Box::new(FlacDecoder::<i16>::new()), input, output)
                    }),
                )
            }),
        ));
        let flac = data.clone();
        builders.push((
            "flac",
            Box::new(move || {
                file_case(
                    &flac,
                    "signal.flac",
                    44_100,
                    "FLAC",
                    "44.1 kHz stereo 16-bit, level 5, i16 blocks",
                    true,
                    Box::new(|input, output| {
                        decode_all(
                            &mut *Box::new(FlacDecoder::<i16, 4608, 2, i16>::new()),
                            input,
                            output,
                        )
                    }),
                )
            }),
        ));
        let opus = data.clone();
        builders.push((
            "opus",
            Box::new(move || {
                file_case(
                    &opus,
                    "signal.opus",
                    48_000,
                    "Opus",
                    "48 kHz stereo, 128 kbit/s VBR",
                    false,
                    Box::new(|input, output| {
                        let mut slot = Box::new(std::mem::MaybeUninit::<OpusDecoder>::uninit());
                        decode_all(OpusDecoder::init(&mut slot), input, output)
                    }),
                )
            }),
        ));
        builders.push((
            "mp3",
            Box::new(move || {
                file_case(
                    &data,
                    "signal.mp3",
                    44_100,
                    "MP3",
                    "44.1 kHz stereo, 128 kbit/s CBR",
                    false,
                    Box::new(|input, output| {
                        decode_all(&mut *Box::new(Mp3Decoder::<i16>::new()), input, output)
                    }),
                )
            }),
        ));
    }

    builders
        .into_iter()
        .filter(|(key, _)| filter.is_none_or(|filter| filter.split(',').any(|item| item == *key)))
        .filter_map(|(_, builder)| builder())
        .collect()
}

pub fn snr(source: &[i16], decoded: &[i16], channels: usize) -> (f64, usize) {
    let frames = (source.len() / channels).min(decoded.len() / channels);
    let window = 16_384.min(frames / 2);
    let start = 2_048.min(frames / 4);
    let maximum_lag = 2_048.min(frames - start - window);
    let mut best = (f64::MIN, 0);

    for lag in 0..=maximum_lag {
        let mut correlation = 0.0;

        for frame in start..start + window {
            correlation +=
                source[frame * channels] as f64 * decoded[(frame + lag) * channels] as f64;
        }

        if correlation > best.0 {
            best = (correlation, lag);
        }
    }

    let lag = best.1;
    let mut signal = 0.0;
    let mut noise = 0.0;

    for frame in start..frames - lag {
        for channel in 0..channels {
            let original = source[frame * channels + channel] as f64;
            let difference = original - decoded[(frame + lag) * channels + channel] as f64;
            signal += original * original;
            noise += difference * difference;
        }
    }

    if noise == 0.0 {
        (f64::INFINITY, lag)
    } else {
        (10.0 * (signal / noise).log10(), lag)
    }
}
