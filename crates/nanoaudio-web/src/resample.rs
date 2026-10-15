use nanoaudio_core::{Format, MAXIMUM_CHANNELS, Result, codec::Progress};
use nanoaudio_dsp::{Float, Linear, Resampler, Sinc, Stream, source_using};

const HISTORY: usize = 2048;

/// The largest ratio between the stream and context sample rates that [`Quality::Sinc`] converts down.
///
/// Downsampling by more needs more history than the resampler keeps. Upsampling, and [`Quality::Linear`], have no limit.
pub const MAXIMUM_RATIO: u32 = 31;

/// How the stream is resampled when the audio context runs at another sample rate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Quality {
    /// A 64-tap windowed-sinc filter, transparent for music.
    ///
    /// It costs 64 multiply-adds per output sample when upsampling, and more when downsampling, in proportion to the ratio.
    #[default]
    Sinc,
    /// Linear interpolation, a few operations per sample.
    ///
    /// It lets images and aliasing through, which is audible on music: use it for voice or to save CPU.
    Linear,
}

impl From<Quality> for u8 {
    fn from(quality: Quality) -> Self {
        match quality {
            Quality::Sinc => 0,
            Quality::Linear => 1,
        }
    }
}

impl From<u8> for Quality {
    fn from(value: u8) -> Self {
        match value {
            1 => Quality::Linear,
            _ => Quality::Sinc,
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum Rate {
    Linear(Resampler<Linear, Float, MAXIMUM_CHANNELS>),
    Sinc(Resampler<Sinc<HISTORY>, Float, MAXIMUM_CHANNELS>),
}

impl Rate {
    pub(crate) fn new(
        quality: Quality,
        channels: u8,
        input_rate: u32,
        output_rate: u32,
    ) -> Result<Self> {
        Ok(match quality {
            Quality::Linear => Self::Linear(Resampler::new(channels, input_rate, output_rate)?),
            Quality::Sinc => Self::Sinc(Resampler::new(channels, input_rate, output_rate)?),
        })
    }

    pub(crate) fn input_frames(&self, output_frames: usize) -> usize {
        match self {
            Self::Linear(resampler) => resampler.input_frames(output_frames),
            Self::Sinc(resampler) => resampler.input_frames(output_frames),
        }
    }

    pub(crate) fn output_frames(&self, input_frames: usize) -> usize {
        match self {
            Self::Linear(resampler) => resampler.output_frames(input_frames),
            Self::Sinc(resampler) => resampler.output_frames(input_frames),
        }
    }

    pub(crate) fn process(&mut self, input: &[f32], output: &mut [f32]) -> Progress {
        match self {
            Self::Linear(resampler) => resampler.process(input, output),
            Self::Sinc(resampler) => resampler.process(input, output),
        }
    }

    pub(crate) fn convert(
        &mut self,
        input: &[u8],
        format: Format,
        channels: u8,
        output: &mut [f32],
    ) -> usize {
        let Ok(source) = source_using::<Float>(input, format, channels) else {
            return 0;
        };

        match self {
            Self::Linear(resampler) => source
                .resample(resampler)
                .map_or(0, |mut stream| stream.write_samples(output)),
            Self::Sinc(resampler) => source
                .resample(resampler)
                .map_or(0, |mut stream| stream.write_samples(output)),
        }
    }
}
