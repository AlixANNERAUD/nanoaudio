use nanoaudio_core::{Format, MAXIMUM_CHANNELS, codec::Progress};
use nanoaudio_dsp::{Float, Linear, Resampler, Sinc, Stream, source_using};

const HISTORY: usize = 2048;

/// The largest ratio between the stream and device sample rates that [`Quality::Sinc`] converts down.
///
/// Downsampling by more needs more history than the resampler keeps. Upsampling, and [`Quality::Linear`], have no limit.
pub const MAXIMUM_RATIO: u32 = 31;

/// How the stream is resampled when the device runs at another sample rate.
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

pub(crate) enum Rate {
    Linear(Resampler<Linear, Float, MAXIMUM_CHANNELS>),
    Sinc(Box<Resampler<Sinc<HISTORY>, Float, MAXIMUM_CHANNELS>>),
}

impl Rate {
    pub(crate) fn new(
        quality: Quality,
        channels: u8,
        input_rate: u32,
        output_rate: u32,
    ) -> nanoaudio_core::Result<Self> {
        Ok(match quality {
            Quality::Linear => Self::Linear(Resampler::new(channels, input_rate, output_rate)?),
            Quality::Sinc => {
                Self::Sinc(Box::new(Resampler::new(channels, input_rate, output_rate)?))
            }
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
        from: Format,
        channels: u8,
        output: &mut [u8],
        to: Format,
    ) -> usize {
        let Ok(source) = source_using::<Float>(input, from, channels) else {
            return 0;
        };

        match self {
            Self::Linear(resampler) => source
                .resample(resampler)
                .map_or(0, |mut stream| stream.write_bytes(output, to)),
            Self::Sinc(resampler) => source
                .resample(&mut **resampler)
                .map_or(0, |mut stream| stream.write_bytes(output, to)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_documented_ratios() {
        for quality in [Quality::Sinc, Quality::Linear] {
            assert!(Rate::new(quality, 2, 8_000 * MAXIMUM_RATIO, 8_000).is_ok());
            assert!(Rate::new(quality, 2, 8_000, 384_000).is_ok());
            assert!(Rate::new(quality, MAXIMUM_CHANNELS as u8, 44_100, 48_000).is_ok());
            assert!(Rate::new(quality, MAXIMUM_CHANNELS as u8 + 1, 44_100, 48_000).is_err());
        }

        assert!(Rate::new(Quality::Sinc, 2, 8_000 * (MAXIMUM_RATIO + 1), 8_000).is_err());
    }
}
