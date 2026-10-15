use nanoaudio_dsp::{Analyzer, Representation};

/// Counts the frames it is given, a minimal [`Analyzer`].
///
/// It works with any representation. Divide [`frames`](Self::frames) by the sample rate to get a duration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FrameCounter {
    frames: u64,
    channels: u8,
}

impl FrameCounter {
    /// Creates a counter at zero.
    pub const fn new() -> Self {
        Self {
            frames: 0,
            channels: 0,
        }
    }

    /// Returns the number of frames counted.
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// Returns the number of channels of the last frame, or 0 before the first one.
    pub const fn channels(&self) -> u8 {
        self.channels
    }

    /// Returns the number of samples counted, over every channel, assuming the channel count did not change.
    pub const fn samples(&self) -> u64 {
        self.frames * self.channels as u64
    }

    /// Sets the counter back to zero.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl<R: Representation> Analyzer<R> for FrameCounter {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        self.frames += 1;
        self.channels = frame.len() as u8;
    }
}

#[cfg(test)]
mod tests {
    use nanoaudio_core::{Format, MAXIMUM_CHANNELS};
    use nanoaudio_dsp::{Fixed, Float, Stream, Volume, analyze_bytes_using, samples_using};

    use super::*;

    fn counts<R: Representation>() {
        let input = [1000i16; 2 * 3 * 5 * 7 + 1];

        for channels in 1..=MAXIMUM_CHANNELS as u8 {
            let mut counter = FrameCounter::new();
            let mut output = [0i16; 256];
            let mut expected = [0i16; 256];

            let frames = samples_using::<R, _>(&input, channels)
                .unwrap()
                .tap(&mut counter)
                .volume(Volume::from_percent(50))
                .write_samples(&mut output);
            samples_using::<R, _>(&input, channels)
                .unwrap()
                .volume(Volume::from_percent(50))
                .write_samples(&mut expected);

            assert_eq!(output, expected);
            assert_eq!(counter.frames(), frames as u64);
            assert_eq!(counter.channels(), channels);
            assert_eq!(frames, input.len() / channels as usize);

            let mut sliced = FrameCounter::new();
            let bytes = [0u8; 2 * (2 * 3 * 5 * 7 + 1)];

            assert_eq!(
                analyze_bytes_using::<R, _>(&bytes, Format::I16, channels, &mut sliced),
                Ok(frames)
            );
            assert_eq!(sliced, counter);
            assert_eq!(sliced.samples(), (frames * channels as usize) as u64);

            sliced.reset();
            assert_eq!(sliced, FrameCounter::default());
        }
    }

    #[test]
    fn fixed() {
        counts::<Fixed>();
    }

    #[test]
    fn float() {
        counts::<Float>();
    }
}
