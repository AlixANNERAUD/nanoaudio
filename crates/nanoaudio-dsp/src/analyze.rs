use nanoaudio_core::{Format, Result, Sample};

use crate::{ByteSource, DefaultRepresentation, Representation, SampleSource, Stream};

type Value<S> = <<S as Stream>::Representation as Representation>::Value;

/// Something that measures audio frame by frame, such as a level meter or a tone detector.
///
/// It receives every frame in the representation `R`, without a copy, and keeps fixed-size state. Results are read through the analyzer's own methods, at any rate, for example once per display refresh.
///
/// [`Representation::to_sample`] converts values to a common domain, such as `i32` or `f32`, without depending on `R`. An analyzer can also be implemented for [`Fixed`](crate::Fixed) and [`Float`](crate::Float) separately, to work on `i32` and `f32` directly.
///
/// ```
/// use nanoaudio_dsp::{Analyzer, Representation, Stream, samples};
///
/// #[derive(Default)]
/// struct Loudest(u32);
///
/// impl<R: Representation> Analyzer<R> for Loudest {
///     fn analyze(&mut self, frame: &[R::Value]) {
///         for value in frame {
///             self.0 = self.0.max(R::to_sample::<i32>(*value).unsigned_abs());
///         }
///     }
/// }
///
/// let mut loudest = Loudest::default();
/// let mut output = [0i16; 4];
///
/// samples(&[100i16, -3000, 2000, 50], 2)
///     .unwrap()
///     .tap(&mut loudest)
///     .write_samples(&mut output);
///
/// assert_eq!(loudest.0, 3000 << 16);
/// assert_eq!(output, [100, -3000, 2000, 50]);
/// ```
pub trait Analyzer<R: Representation> {
    /// Measures one interleaved frame, one value per channel.
    fn analyze(&mut self, frame: &[R::Value]);
}

impl<R: Representation, A: Analyzer<R> + ?Sized> Analyzer<R> for &mut A {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        (**self).analyze(frame)
    }
}

/// The stage returned by [`Stream::tap`], which passes frames through unchanged and gives each one to an [`Analyzer`].
pub struct Tap<St: Stream, A> {
    stream: St,
    analyzer: A,
}

impl<St: Stream, A> Tap<St, A> {
    pub(crate) fn new(stream: St, analyzer: A) -> Self {
        Self { stream, analyzer }
    }
}

impl<St, A> Stream for Tap<St, A>
where
    St: Stream,
    A: Analyzer<St::Representation>,
{
    type Representation = St::Representation;

    fn channels(&self) -> u8 {
        self.stream.channels()
    }

    #[inline]
    fn next_frame(&mut self, frame: &mut [Value<Self>]) -> bool {
        if !self.stream.next_frame(frame) {
            return false;
        }

        self.analyzer.analyze(frame);
        true
    }
}

fn drain<St: Stream, A: Analyzer<St::Representation>>(stream: St, mut analyzer: A) -> usize {
    let channels = stream.channels() as usize;
    let mut stream = Tap::new(stream, &mut analyzer);
    let mut frame =
        [<St::Representation as Representation>::SILENCE; nanoaudio_core::MAXIMUM_CHANNELS];
    let mut frames = 0;

    while stream.next_frame(&mut frame[..channels]) {
        frames += 1;
    }

    frames
}

/// Gives every frame of interleaved `samples` to `analyzer`, with the [`DefaultRepresentation`], and returns the number of frames analyzed.
///
/// It gives the same frames as a [`tap`](Stream::tap) on [`samples`](crate::samples), and ignores a trailing incomplete frame. Returns [`Error::TooManyChannels`](nanoaudio_core::Error::TooManyChannels) if `channels` is 0 or above [`MAXIMUM_CHANNELS`](nanoaudio_core::MAXIMUM_CHANNELS).
pub fn analyze<S, A>(samples: &[S], channels: u8, analyzer: A) -> Result<usize>
where
    S: Sample,
    A: Analyzer<DefaultRepresentation>,
{
    analyze_using::<DefaultRepresentation, S, A>(samples, channels, analyzer)
}

/// Like [`analyze`], with the representation `R`.
pub fn analyze_using<R, S, A>(samples: &[S], channels: u8, analyzer: A) -> Result<usize>
where
    R: Representation,
    S: Sample,
    A: Analyzer<R>,
{
    Ok(drain(
        SampleSource::<S, R>::new(samples, channels)?,
        analyzer,
    ))
}

/// Like [`analyze`], on interleaved little-endian samples in `format`.
pub fn analyze_bytes<A>(bytes: &[u8], format: Format, channels: u8, analyzer: A) -> Result<usize>
where
    A: Analyzer<DefaultRepresentation>,
{
    analyze_bytes_using::<DefaultRepresentation, A>(bytes, format, channels, analyzer)
}

/// Like [`analyze_bytes`], with the representation `R`.
pub fn analyze_bytes_using<R, A>(
    bytes: &[u8],
    format: Format,
    channels: u8,
    analyzer: A,
) -> Result<usize>
where
    R: Representation,
    A: Analyzer<R>,
{
    Ok(drain(
        ByteSource::<R>::new(bytes, format, channels)?,
        analyzer,
    ))
}

#[cfg(test)]
mod tests {
    use core::marker::PhantomData;

    use nanoaudio_core::MAXIMUM_CHANNELS;

    use super::*;
    use crate::bytes::testing::{FORMATS, LENGTH, fill};
    use crate::{Fixed, Float, Volume, samples_using, source_using};

    struct Recorder<R: Representation> {
        values: [i32; LENGTH],
        length: usize,
        frames: usize,
        channels: usize,
        consistent: bool,
        representation: PhantomData<R>,
    }

    impl<R: Representation> Recorder<R> {
        fn new() -> Self {
            Self {
                values: [0; LENGTH],
                length: 0,
                frames: 0,
                channels: 0,
                consistent: true,
                representation: PhantomData,
            }
        }

        fn values(&self) -> &[i32] {
            &self.values[..self.length]
        }
    }

    impl<R: Representation> Analyzer<R> for Recorder<R> {
        fn analyze(&mut self, frame: &[R::Value]) {
            if self.frames > 0 && frame.len() != self.channels {
                self.consistent = false;
            }

            self.channels = frame.len();
            self.frames += 1;

            for value in frame {
                if self.length < LENGTH {
                    self.values[self.length] = R::to_sample(*value);
                    self.length += 1;
                }
            }
        }
    }

    fn passes_through_unchanged<R: Representation>() {
        let mut input = [0u8; LENGTH];
        let volume = Volume::from_percent(70);

        for channels in [1, 2, 3, MAXIMUM_CHANNELS as u8] {
            for input_format in FORMATS {
                for output_format in FORMATS {
                    fill(&mut input, input_format, channels as u64, 3);
                    let input = &input[..input.len() / 2];
                    let mut expected = [0u8; 2 * LENGTH];
                    let mut actual = [0u8; 2 * LENGTH];
                    let mut recorder = Recorder::<R>::new();

                    let expected_frames = source_using::<R>(input, input_format, channels)
                        .unwrap()
                        .volume(volume)
                        .write_bytes(&mut expected, output_format);
                    let frames = source_using::<R>(input, input_format, channels)
                        .unwrap()
                        .tap(&mut recorder)
                        .volume(volume)
                        .write_bytes(&mut actual, output_format);

                    assert_eq!(frames, expected_frames);
                    assert_eq!(
                        frames,
                        input.len() / (channels as usize * input_format.sample_size())
                    );
                    assert_eq!(actual, expected);
                    assert_eq!(recorder.frames, frames);
                    assert_eq!(recorder.channels, channels as usize);
                    assert!(recorder.consistent);

                    let mut sliced = Recorder::<R>::new();
                    let analyzed =
                        analyze_bytes_using::<R, _>(input, input_format, channels, &mut sliced)
                            .unwrap();

                    assert_eq!(analyzed, frames);
                    assert_eq!(sliced.values(), recorder.values());
                }
            }
        }
    }

    fn sees_the_frames_of_its_position<R: Representation>() {
        let input = [1000i16, -2000, 3000, -4000, 5000];
        let mut before = Recorder::<R>::new();
        let mut after = Recorder::<R>::new();
        let mut output = [0i16; 2];

        let frames = samples_using::<R, _>(&input, 1)
            .unwrap()
            .tap(&mut before)
            .to_channels(2)
            .unwrap()
            .volume(Volume::from_percent(50))
            .tap(&mut after)
            .write_samples(&mut output);

        assert_eq!(frames, 1);
        assert_eq!(before.frames, 1);
        assert_eq!(before.channels, 1);
        assert_eq!(after.channels, 2);
        assert_eq!(after.values()[1], 500 << 16);

        let mut sliced = Recorder::<R>::new();

        assert_eq!(analyze_using::<R, _, _>(&input, 2, &mut sliced), Ok(2));
        assert_eq!(sliced.frames, 2);
        assert_eq!(sliced.channels, 2);
        assert_eq!(sliced.values()[3], -4000 << 16);
    }

    #[test]
    fn fixed() {
        passes_through_unchanged::<Fixed>();
        sees_the_frames_of_its_position::<Fixed>();
    }

    #[test]
    fn float() {
        passes_through_unchanged::<Float>();
        sees_the_frames_of_its_position::<Float>();
    }

    #[test]
    fn rejects_invalid_channel_counts() {
        let mut recorder = Recorder::<DefaultRepresentation>::new();

        assert!(analyze(&[0i16], 0, &mut recorder).is_err());
        assert!(analyze_bytes(&[0u8], Format::U8, 9, &mut recorder).is_err());
        assert_eq!(recorder.frames, 0);
        assert_eq!(analyze(&[0i16; 3], 2, &mut recorder), Ok(1));
    }
}
