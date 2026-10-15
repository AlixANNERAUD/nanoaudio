use nanoaudio_core::Format;
use nanoaudio_dsp::{
    Fixed, Float, Interpolation, Linear, Representation, Resampler, Sinc, Stream, Volume, convert,
    convert_bytes, convert_bytes_dithered, convert_dithered, mix_bytes_using, mix_using,
    source_using,
};

use crate::signal;

pub struct Data {
    pub music: Vec<i16>,
    pub music_bytes: Vec<u8>,
    pub music_i24: Vec<u8>,
    pub voice_bytes: Vec<u8>,
}

impl Data {
    pub fn new() -> Self {
        let music = signal::generate(44_100, 2);
        let music_bytes = to_bytes(&music);
        let mut music_i24 = vec![0u8; music.len() * 3];
        convert_bytes(&music_bytes, Format::I16, &mut music_i24, Format::I24);
        let voice: Vec<i16> = signal::generate(44_100, 1)
            .iter()
            .map(|sample| sample / 2)
            .collect();

        Self {
            music,
            music_bytes,
            music_i24,
            voice_bytes: to_bytes(&voice),
        }
    }

    pub fn samples(&self) -> usize {
        self.music.len()
    }
}

fn to_bytes(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

const QUIETER: Volume = Volume::from_percent(80);
const LOUDER: Volume = Volume::from_percent(125);

pub struct Operation {
    pub name: &'static str,
    pub run: Box<dyn FnMut() -> usize>,
}

fn alternate(toggle: &mut bool) -> Volume {
    *toggle = !*toggle;

    if *toggle { QUIETER } else { LOUDER }
}

fn resample<K, R>(name: &'static str, data: &Data, input_rate: u32, output_rate: u32) -> Operation
where
    R: Representation + 'static,
    K: Interpolation<R, 2> + 'static,
{
    let music = data.music.clone();
    let mut output = vec![0i16; music.len() * 2];

    Operation {
        name,
        run: Box::new(move || {
            let mut resampler = Resampler::<K, R>::new(2, input_rate, output_rate).unwrap();
            let progress = resampler.process(&music, &mut output);
            debug_assert_eq!(progress.consumed, music.len());
            progress.produced
        }),
    }
}

pub fn representation_operations<R>(data: &Data) -> Vec<Operation>
where
    R: Representation + 'static,
    Linear: Interpolation<R, 2>,
    Sinc: Interpolation<R, 2>,
{
    let mut volume_samples = data.music.clone();
    let mut volume_bytes = data.music_bytes.clone();
    let mut volume_i24 = data.music_i24.clone();
    let (mut toggle_samples, mut toggle_bytes, mut toggle_i24) = (false, false, false);
    let mix_input = data.music.iter().rev().copied().collect::<Vec<i16>>();
    let mix_base = data.music.clone();
    let mut mix_output = data.music.clone();
    let mix_bytes_input = to_bytes(&mix_input);
    let mix_bytes_base = data.music_bytes.clone();
    let mut mix_bytes_output = data.music_bytes.clone();
    let music = data.music_bytes.clone();
    let voice = data.voice_bytes.clone();
    let mut pipeline_output = vec![0u8; music.len()];
    let samples = data.samples();

    vec![
        Operation {
            name: "Volume, i16 slice",
            run: Box::new(move || {
                alternate(&mut toggle_samples).apply_to_using::<R, i16>(&mut volume_samples);
                volume_samples.len()
            }),
        },
        Operation {
            name: "Volume, 16-bit bytes",
            run: Box::new(move || {
                alternate(&mut toggle_bytes)
                    .apply_to_bytes_using::<R>(&mut volume_bytes, Format::I16);
                volume_bytes.len() / 2
            }),
        },
        Operation {
            name: "Volume, 24-bit bytes",
            run: Box::new(move || {
                alternate(&mut toggle_i24).apply_to_bytes_using::<R>(&mut volume_i24, Format::I24);
                volume_i24.len() / 3
            }),
        },
        Operation {
            name: "Mix, i16 slice (includes a copy to reset the output)",
            run: Box::new(move || {
                mix_output.copy_from_slice(&mix_base);
                mix_using::<R, i16>(&mut mix_output, &mix_input, QUIETER)
            }),
        },
        Operation {
            name: "Mix, 16-bit bytes (includes a copy to reset the output)",
            run: Box::new(move || {
                mix_bytes_output.copy_from_slice(&mix_bytes_base);
                mix_bytes_using::<R>(
                    &mut mix_bytes_output,
                    &mix_bytes_input,
                    Format::I16,
                    QUIETER,
                )
            }),
        },
        Operation {
            name: "Pipeline: stereo 16-bit + volume + mono voice upmixed, to 16-bit bytes",
            run: Box::new(move || {
                let voice = source_using::<R>(&voice, Format::I16, 1)
                    .unwrap()
                    .to_channels(2)
                    .unwrap();
                let frames = source_using::<R>(&music, Format::I16, 2)
                    .unwrap()
                    .volume(QUIETER)
                    .mix(voice)
                    .write_bytes(&mut pipeline_output, Format::I16);
                debug_assert_eq!(frames * 2, samples);
                frames * 2
            }),
        },
        resample::<Linear, R>(
            "Resample 44.1 to 48 kHz, linear, stereo i16 slice",
            data,
            44_100,
            48_000,
        ),
        resample::<Sinc, R>(
            "Resample 44.1 to 48 kHz, sinc, stereo i16 slice",
            data,
            44_100,
            48_000,
        ),
        resample::<Sinc, R>(
            "Resample 48 to 44.1 kHz, sinc, stereo i16 slice",
            data,
            48_000,
            44_100,
        ),
        resample::<Sinc, R>(
            "Resample 48 to 16 kHz, sinc, stereo i16 slice",
            data,
            48_000,
            16_000,
        ),
    ]
}

pub fn conversion_operations(data: &Data) -> Vec<Operation> {
    let music = data.music.clone();
    let music_bytes = data.music_bytes.clone();
    let music_i24 = data.music_i24.clone();
    let mut floats = vec![0f32; music.len()];
    let mut float_bytes = vec![0u8; music.len() * 4];
    let mut i16_bytes = vec![0u8; music.len() * 2];
    let mut i24_bytes = vec![0u8; music.len() * 3];
    let mut back = vec![0i16; music.len()];
    let mut dithered = vec![0i16; music.len()];
    let mut state = 1;
    let dither_source = {
        let mut values = vec![0f32; music.len()];
        convert::<i16, f32>(&music, &mut values);
        values
    };
    let float_source = {
        let mut values = vec![0f32; music.len()];
        convert::<i16, f32>(&music, &mut values);
        values
    };

    vec![
        Operation {
            name: "i16 slice to f32 slice",
            run: Box::new(move || convert::<i16, f32>(&music, &mut floats)),
        },
        Operation {
            name: "f32 slice to i16 slice",
            run: Box::new(move || convert::<f32, i16>(&float_source, &mut back)),
        },
        Operation {
            name: "16-bit bytes to 32-bit float bytes",
            run: Box::new(move || {
                convert_bytes(&music_bytes, Format::I16, &mut float_bytes, Format::F32)
            }),
        },
        Operation {
            name: "16-bit bytes to 24-bit bytes",
            run: Box::new({
                let music_bytes = data.music_bytes.clone();
                move || convert_bytes(&music_bytes, Format::I16, &mut i24_bytes, Format::I24)
            }),
        },
        Operation {
            name: "24-bit bytes to 16-bit bytes",
            run: Box::new(move || {
                convert_bytes(&music_i24, Format::I24, &mut i16_bytes, Format::I16)
            }),
        },
        Operation {
            name: "f32 slice to i16 slice, dithered",
            run: Box::new(move || {
                convert_dithered::<f32, i16>(&dither_source, &mut dithered, &mut state)
            }),
        },
        Operation {
            name: "24-bit bytes to 16-bit bytes, dithered",
            run: Box::new({
                let music_i24 = data.music_i24.clone();
                let mut i16_bytes = vec![0u8; data.music.len() * 2];
                let mut state = 1;
                move || {
                    convert_bytes_dithered(
                        &music_i24,
                        Format::I24,
                        &mut i16_bytes,
                        Format::I16,
                        &mut state,
                    )
                }
            }),
        },
    ]
}

pub fn representations(data: &Data) -> Vec<(&'static str, Vec<Operation>)> {
    vec![
        ("Fixed", representation_operations::<Fixed>(data)),
        ("Float", representation_operations::<Float>(data)),
    ]
}
