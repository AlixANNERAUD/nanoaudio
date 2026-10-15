use nanoaudio_buffer::{Consumer, Producer};
use nanoaudio_core::{Configuration, Error, Format, I24, MAXIMUM_CHANNELS, Result, Sample};

const LARGEST_FRAME: usize = MAXIMUM_CHANNELS * 4;

fn check(configuration: Configuration) -> Result<Configuration> {
    if configuration.channels == 0 || configuration.channels as usize > MAXIMUM_CHANNELS {
        return Err(Error::TooManyChannels);
    }

    if configuration.sample_rate == 0 {
        return Err(Error::InvalidParameter);
    }

    Ok(configuration)
}

/// Moves interleaved PCM from a [`Consumer`] to planar `f32` channels, the layout of a Web Audio render quantum.
///
/// It plays silence for the frames the consumer is missing and counts each short call as an underrun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renderer {
    configuration: Configuration,
    underruns: u32,
}

impl Renderer {
    /// Creates a renderer for a stream in `configuration`.
    ///
    /// # Errors
    ///
    /// [`Error::TooManyChannels`] for 0 or more than [`MAXIMUM_CHANNELS`] channels, and [`Error::InvalidParameter`] for a sample rate of 0.
    pub fn new(configuration: Configuration) -> Result<Self> {
        Ok(Self {
            configuration: check(configuration)?,
            underruns: 0,
        })
    }

    /// Returns the configuration of the stream.
    pub fn configuration(&self) -> Configuration {
        self.configuration
    }

    /// Returns the number of calls to [`render`](Self::render) that ran out of frames.
    pub fn underruns(&self) -> u32 {
        self.underruns
    }

    /// Fills `output` with as many whole frames as `consumer` holds, and silence after them.
    ///
    /// `output` holds the channels one after the other, `output.len() / channels` samples each. Returns the number of frames read.
    pub fn render<const N: usize>(
        &mut self,
        consumer: &mut Consumer<'_, N>,
        output: &mut [f32],
    ) -> usize {
        let channels = self.configuration.channels as usize;
        let frame_size = self.configuration.frame_size();
        let frames = output.len() / channels;
        let mut done = 0;

        while done < frames {
            let readable = consumer.readable();
            let count = (readable.len() / frame_size).min(frames - done);

            if count > 0 {
                let bytes = count * frame_size;
                self.deinterleave(&readable[..bytes], frames, done, output);
                consumer.release(bytes);
                done += count;
            } else if consumer.available() >= frame_size {
                let mut frame = [0; LARGEST_FRAME];
                consumer.read(&mut frame[..frame_size]);
                self.deinterleave(&frame[..frame_size], frames, done, output);
                done += 1;
            } else {
                break;
            }
        }

        if done < frames {
            for channel in output.chunks_exact_mut(frames) {
                channel[done..].fill(0.0);
            }

            self.underruns = self.underruns.wrapping_add(1);
        }

        done
    }

    fn deinterleave(&self, input: &[u8], frames: usize, offset: usize, output: &mut [f32]) {
        let channels = self.configuration.channels as usize;

        match self.configuration.format {
            Format::U8 => deinterleave::<u8>(input, channels, frames, offset, output),
            Format::I16 => deinterleave::<i16>(input, channels, frames, offset, output),
            Format::I24 => deinterleave::<I24>(input, channels, frames, offset, output),
            Format::I32 => deinterleave::<i32>(input, channels, frames, offset, output),
            Format::F32 => deinterleave::<f32>(input, channels, frames, offset, output),
        }
    }
}

fn deinterleave<S: Sample>(
    input: &[u8],
    channels: usize,
    frames: usize,
    offset: usize,
    output: &mut [f32],
) {
    for (index, frame) in input.chunks_exact(S::SIZE * channels).enumerate() {
        for (channel, sample) in frame.chunks_exact(S::SIZE).enumerate() {
            output[channel * frames + offset + index] = S::read_le(sample).to_f32();
        }
    }
}

/// Moves planar `f32` channels, the layout of a Web Audio render quantum, to interleaved PCM in a [`Producer`].
///
/// It drops the frames that do not fit and counts each short call as an overrun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorder {
    configuration: Configuration,
    overruns: u32,
}

impl Recorder {
    /// Creates a recorder for a stream in `configuration`.
    ///
    /// # Errors
    ///
    /// [`Error::TooManyChannels`] for 0 or more than [`MAXIMUM_CHANNELS`] channels, and [`Error::InvalidParameter`] for a sample rate of 0.
    pub fn new(configuration: Configuration) -> Result<Self> {
        Ok(Self {
            configuration: check(configuration)?,
            overruns: 0,
        })
    }

    /// Returns the configuration of the stream.
    pub fn configuration(&self) -> Configuration {
        self.configuration
    }

    /// Returns the number of calls to [`record`](Self::record) that had to drop frames.
    pub fn overruns(&self) -> u32 {
        self.overruns
    }

    /// Writes as many whole frames of `input` as fit in `producer`, and returns their number.
    ///
    /// `input` holds the channels one after the other, `input.len() / channels` samples each.
    pub fn record<const N: usize>(
        &mut self,
        input: &[f32],
        producer: &mut Producer<'_, N>,
    ) -> usize {
        let channels = self.configuration.channels as usize;
        let frame_size = self.configuration.frame_size();
        let frames = input.len() / channels;
        let mut done = 0;

        while done < frames {
            let grant = producer.grant();
            let count = (grant.len() / frame_size).min(frames - done);

            if count > 0 {
                let bytes = count * frame_size;
                self.interleave(input, frames, done, &mut grant[..bytes]);
                producer.commit(bytes);
                done += count;
            } else if producer.free() >= frame_size {
                let mut frame = [0; LARGEST_FRAME];
                self.interleave(input, frames, done, &mut frame[..frame_size]);
                producer.write(&frame[..frame_size]);
                done += 1;
            } else {
                break;
            }
        }

        if done < frames {
            self.overruns = self.overruns.wrapping_add(1);
        }

        done
    }

    fn interleave(&self, input: &[f32], frames: usize, offset: usize, output: &mut [u8]) {
        let channels = self.configuration.channels as usize;

        match self.configuration.format {
            Format::U8 => interleave::<u8>(input, channels, frames, offset, output),
            Format::I16 => interleave::<i16>(input, channels, frames, offset, output),
            Format::I24 => interleave::<I24>(input, channels, frames, offset, output),
            Format::I32 => interleave::<i32>(input, channels, frames, offset, output),
            Format::F32 => interleave::<f32>(input, channels, frames, offset, output),
        }
    }
}

fn interleave<S: Sample>(
    input: &[f32],
    channels: usize,
    frames: usize,
    offset: usize,
    output: &mut [u8],
) {
    for (index, frame) in output.chunks_exact_mut(S::SIZE * channels).enumerate() {
        for (channel, sample) in frame.chunks_exact_mut(S::SIZE).enumerate() {
            S::from_f32(input[channel * frames + offset + index]).write_le(sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nanoaudio_buffer::RingBuffer;

    fn stereo(format: Format) -> Configuration {
        Configuration::new(48_000, 2, format)
    }

    #[test]
    fn rejects_invalid_configurations() {
        assert_eq!(
            Renderer::new(Configuration::new(48_000, 0, Format::I16)),
            Err(Error::TooManyChannels)
        );
        assert_eq!(
            Recorder::new(Configuration::new(48_000, 9, Format::I16)),
            Err(Error::TooManyChannels)
        );
        assert_eq!(
            Renderer::new(Configuration::new(0, 2, Format::I16)),
            Err(Error::InvalidParameter)
        );
    }

    #[test]
    fn renders_interleaved_samples_to_planar_channels() {
        let mut buffer = RingBuffer::<64>::new();
        let (mut producer, mut consumer) = buffer.split();
        let mut renderer = Renderer::new(stereo(Format::I16)).unwrap();

        for sample in [16384i16, -16384, 8192, -8192] {
            producer.write(&sample.to_le_bytes());
        }

        let mut output = [1.0; 4];

        assert_eq!(renderer.render(&mut consumer, &mut output), 2);
        assert_eq!(output, [0.5, 0.25, -0.5, -0.25]);
        assert_eq!(renderer.underruns(), 0);
    }

    #[test]
    fn plays_silence_and_counts_underruns() {
        let mut buffer = RingBuffer::<64>::new();
        let (mut producer, mut consumer) = buffer.split();
        let mut renderer = Renderer::new(stereo(Format::F32)).unwrap();

        producer.write(&0.5f32.to_le_bytes());
        producer.write(&(-0.5f32).to_le_bytes());
        producer.write(&0.25f32.to_le_bytes());

        let mut output = [1.0; 6];

        assert_eq!(renderer.render(&mut consumer, &mut output), 1);
        assert_eq!(output, [0.5, 0.0, 0.0, -0.5, 0.0, 0.0]);
        assert_eq!(renderer.underruns(), 1);
        assert_eq!(consumer.available(), 4);

        assert_eq!(renderer.render(&mut consumer, &mut output), 0);
        assert_eq!(output, [0.0; 6]);
        assert_eq!(renderer.underruns(), 2);
    }

    #[test]
    fn renders_frames_across_the_end_of_the_storage() {
        let mut buffer = RingBuffer::<10>::new();
        let (mut producer, mut consumer) = buffer.split();
        let mut renderer = Renderer::new(stereo(Format::I24)).unwrap();
        let mut output = [0.0; 2];

        for round in 0..20 {
            let value = I24::new((round - 10) << 19);
            let mut frame = [0; 6];
            value.write_le(&mut frame[..3]);
            I24::new(-value.value()).write_le(&mut frame[3..]);
            assert_eq!(producer.write(&frame), 6);

            assert_eq!(renderer.render(&mut consumer, &mut output), 1);
            assert_eq!(output, [value.to_f32(), -value.to_f32()]);
        }

        assert_eq!(renderer.underruns(), 0);
    }

    #[test]
    fn round_trips_through_the_recorder() {
        for format in [
            Format::U8,
            Format::I16,
            Format::I24,
            Format::I32,
            Format::F32,
        ] {
            let mut buffer = RingBuffer::<14>::new();
            let (mut producer, mut consumer) = buffer.split();
            let mut recorder = Recorder::new(Configuration::new(8_000, 1, format)).unwrap();
            let mut renderer = Renderer::new(Configuration::new(8_000, 1, format)).unwrap();
            let input = [0.5, -0.5, 0.25];
            let mut output = [0.0; 3];

            for _ in 0..10 {
                assert_eq!(recorder.record(&input, &mut producer), 3);
                assert_eq!(renderer.render(&mut consumer, &mut output), 3);
                assert_eq!(output, input, "{format:?}");
            }

            assert_eq!(recorder.overruns(), 0);
            assert_eq!(renderer.underruns(), 0);
        }
    }

    #[test]
    fn drops_frames_and_counts_overruns() {
        let mut buffer = RingBuffer::<10>::new();
        let (mut producer, mut consumer) = buffer.split();
        let mut recorder = Recorder::new(stereo(Format::I16)).unwrap();

        assert_eq!(
            recorder.record(&[0.5, 0.5, 0.5, -0.5, -0.5, -0.5], &mut producer),
            2
        );
        assert_eq!(recorder.overruns(), 1);
        assert_eq!(consumer.available(), 8);

        let mut bytes = [0; 8];
        consumer.read(&mut bytes);
        assert_eq!(bytes, [0, 64, 0, 192, 0, 64, 0, 192]);
    }
}
