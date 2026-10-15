use core::fmt;

use nanoaudio_buffer::{Consumer, Producer};
use nanoaudio_core::{Configuration, Error, Format, I24, MAXIMUM_CHANNELS, Result, Sample};
use nanoaudio_dsp::{Float, Stream, samples_using};

use crate::{Quality, resample::Rate};

const LARGEST_FRAME: usize = MAXIMUM_CHANNELS * 4;
const CHUNK: usize = 1024;

fn resampler(
    configuration: Configuration,
    quality: Quality,
    input_rate: u32,
    output_rate: u32,
) -> Result<Option<Rate>> {
    if input_rate == output_rate {
        return Ok(None);
    }

    Rate::new(quality, configuration.channels, input_rate, output_rate).map(Some)
}

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
/// It plays silence for the frames the consumer is missing and counts each short call as an underrun. Created with [`with_rate`](Self::with_rate), it also resamples the stream to the rate of the render quantum.
pub struct Renderer {
    configuration: Configuration,
    underruns: u32,
    resampler: Option<Rate>,
    staging: [u8; CHUNK * 4],
    resampled: [f32; CHUNK],
}

impl fmt::Debug for Renderer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Renderer")
            .field("configuration", &self.configuration)
            .field("underruns", &self.underruns)
            .field("resampling", &self.resampler.is_some())
            .finish()
    }
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
            resampler: None,
            staging: [0; CHUNK * 4],
            resampled: [0.0; CHUNK],
        })
    }

    /// Creates a renderer that resamples a stream in `configuration` to `rate` in Hz, with `quality`.
    ///
    /// # Errors
    ///
    /// Like [`new`](Self::new), and [`Error::InvalidParameter`] for a rate of 0 or a ratio above [`MAXIMUM_RATIO`](crate::MAXIMUM_RATIO) with [`Quality::Sinc`].
    pub fn with_rate(configuration: Configuration, rate: u32, quality: Quality) -> Result<Self> {
        let mut renderer = Self::new(configuration)?;
        renderer.resampler = resampler(configuration, quality, configuration.sample_rate, rate)?;
        Ok(renderer)
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
    /// `output` holds the channels one after the other, `output.len() / channels` samples each. Returns the number of frames written before the silence, at the rate of `output`.
    pub fn render<const N: usize>(
        &mut self,
        consumer: &mut Consumer<'_, N>,
        output: &mut [f32],
    ) -> usize {
        let channels = self.configuration.channels as usize;
        let frames = output.len() / channels;
        let done = if self.resampler.is_some() {
            self.resample(consumer, output, frames)
        } else {
            self.copy(consumer, output, frames)
        };

        if done < frames {
            for channel in output.chunks_exact_mut(frames) {
                channel[done..].fill(0.0);
            }

            self.underruns = self.underruns.wrapping_add(1);
        }

        done
    }

    fn resample<const N: usize>(
        &mut self,
        consumer: &mut Consumer<'_, N>,
        output: &mut [f32],
        frames: usize,
    ) -> usize {
        let Some(resampler) = &mut self.resampler else {
            return 0;
        };
        let channels = self.configuration.channels;
        let frame_size = self.configuration.frame_size();
        let capacity = self.staging.len() / frame_size;
        let chunk = CHUNK / channels as usize;
        let mut done = 0;

        while done < frames {
            let available = (consumer.available() / frame_size).min(capacity);
            let count = (frames - done)
                .min(chunk)
                .min(resampler.output_frames(available));

            if count == 0 {
                break;
            }

            let staged = &mut self.staging[..resampler.input_frames(count) * frame_size];
            consumer.read(staged);

            let resampled = &mut self.resampled[..count * channels as usize];
            resampler.convert(staged, self.configuration.format, channels, resampled);

            for (index, frame) in resampled.chunks_exact(channels as usize).enumerate() {
                for (channel, sample) in frame.iter().enumerate() {
                    output[channel * frames + done + index] = *sample;
                }
            }

            done += count;
        }

        done
    }

    fn copy<const N: usize>(
        &self,
        consumer: &mut Consumer<'_, N>,
        output: &mut [f32],
        frames: usize,
    ) -> usize {
        let frame_size = self.configuration.frame_size();
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
/// It drops the frames that do not fit and counts each short call as an overrun. Created with [`with_rate`](Self::with_rate), it also resamples the render quantum to the rate of the stream.
pub struct Recorder {
    configuration: Configuration,
    overruns: u32,
    resampler: Option<Rate>,
    staging: [u8; CHUNK * 4],
    input: [f32; CHUNK],
    output: [f32; CHUNK],
}

impl fmt::Debug for Recorder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Recorder")
            .field("configuration", &self.configuration)
            .field("overruns", &self.overruns)
            .field("resampling", &self.resampler.is_some())
            .finish()
    }
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
            resampler: None,
            staging: [0; CHUNK * 4],
            input: [0.0; CHUNK],
            output: [0.0; CHUNK],
        })
    }

    /// Creates a recorder that resamples input at `rate` in Hz to a stream in `configuration`, with `quality`.
    ///
    /// # Errors
    ///
    /// Like [`new`](Self::new), and [`Error::InvalidParameter`] for a rate of 0 or a ratio above [`MAXIMUM_RATIO`](crate::MAXIMUM_RATIO) with [`Quality::Sinc`].
    pub fn with_rate(configuration: Configuration, rate: u32, quality: Quality) -> Result<Self> {
        let mut recorder = Self::new(configuration)?;
        recorder.resampler = resampler(configuration, quality, rate, configuration.sample_rate)?;
        Ok(recorder)
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
    /// `input` holds the channels one after the other, `input.len() / channels` samples each. When resampling, the frames written are at the rate of the stream.
    pub fn record<const N: usize>(
        &mut self,
        input: &[f32],
        producer: &mut Producer<'_, N>,
    ) -> usize {
        let channels = self.configuration.channels as usize;
        let frames = input.len() / channels;
        let (done, complete) = if self.resampler.is_some() {
            self.resample(input, frames, producer)
        } else {
            let done = self.copy(input, frames, producer);
            (done, done == frames)
        };

        if !complete {
            self.overruns = self.overruns.wrapping_add(1);
        }

        done
    }

    fn resample<const N: usize>(
        &mut self,
        input: &[f32],
        frames: usize,
        producer: &mut Producer<'_, N>,
    ) -> (usize, bool) {
        let Some(resampler) = &mut self.resampler else {
            return (0, false);
        };
        let channels = self.configuration.channels as usize;
        let frame_size = self.configuration.frame_size();
        let chunk = CHUNK / channels;
        let room = chunk.min(self.staging.len() / frame_size) * channels;
        let mut written = 0;
        let mut complete = true;
        let mut start = 0;

        while start < frames {
            let count = (frames - start).min(chunk);

            for (index, frame) in self.input[..count * channels]
                .chunks_exact_mut(channels)
                .enumerate()
            {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    *sample = input[channel * frames + start + index];
                }
            }

            let mut pending = &self.input[..count * channels];

            loop {
                let progress = resampler.process(pending, &mut self.output[..room]);
                pending = &pending[progress.consumed..];

                let bytes = progress.produced / channels * frame_size;
                let encoded = &mut self.staging[..bytes];

                if let Ok(mut source) =
                    samples_using::<Float, f32>(&self.output[..progress.produced], channels as u8)
                {
                    source.write_bytes(encoded, self.configuration.format);
                }

                let fits = (producer.free() / frame_size * frame_size).min(bytes);
                producer.write(&encoded[..fits]);
                written += fits / frame_size;
                complete &= fits == bytes;

                if pending.is_empty() && progress.produced < room {
                    break;
                }
            }

            start += count;
        }

        (written, complete)
    }

    fn copy<const N: usize>(
        &self,
        input: &[f32],
        frames: usize,
        producer: &mut Producer<'_, N>,
    ) -> usize {
        let frame_size = self.configuration.frame_size();
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
    extern crate std;

    use std::{vec, vec::Vec};

    use super::*;
    use nanoaudio_buffer::RingBuffer;

    fn stereo(format: Format) -> Configuration {
        Configuration::new(48_000, 2, format)
    }

    #[test]
    fn rejects_invalid_configurations() {
        assert_eq!(
            Renderer::new(Configuration::new(48_000, 0, Format::I16)).err(),
            Some(Error::TooManyChannels)
        );
        assert_eq!(
            Recorder::new(Configuration::new(48_000, 9, Format::I16)).err(),
            Some(Error::TooManyChannels)
        );
        assert_eq!(
            Renderer::new(Configuration::new(0, 2, Format::I16)).err(),
            Some(Error::InvalidParameter)
        );
        assert_eq!(
            Renderer::with_rate(stereo(Format::I16), 0, Quality::Sinc).err(),
            Some(Error::InvalidParameter)
        );
        assert_eq!(
            Recorder::with_rate(
                Configuration::new(8_000, 1, Format::I16),
                8_000 * 32,
                Quality::Sinc
            )
            .err(),
            Some(Error::InvalidParameter)
        );
        assert!(
            Recorder::with_rate(
                Configuration::new(8_000, 1, Format::I16),
                8_000 * 31,
                Quality::Sinc
            )
            .is_ok()
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

    fn cases<T>(cases: impl IntoIterator<Item = T>) -> impl Iterator<Item = T> {
        cases
            .into_iter()
            .take(if cfg!(miri) { 1 } else { usize::MAX })
    }

    const QUANTA: usize = if cfg!(miri) { 8 } else { 1_000 };

    fn tone(frame: usize, rate: u32) -> f32 {
        (0.5 * (core::f64::consts::TAU * 1_000.0 * frame as f64 / rate as f64).sin()) as f32
    }

    fn frequency(samples: &[f32], rate: u32) -> f64 {
        let mut first = None;
        let mut last = 0.0;
        let mut crossings = 0;

        for (index, pair) in samples.windows(2).enumerate() {
            if pair[0] < 0.0 && pair[1] >= 0.0 {
                let position = index as f64 + (pair[0] / (pair[0] - pair[1])) as f64;
                first.get_or_insert(position);
                last = position;
                crossings += 1;
            }
        }

        (crossings - 1) as f64 * rate as f64 / (last - first.unwrap())
    }

    #[test]
    fn resamples_quanta_with_exact_frame_accounting() {
        for quality in [Quality::Sinc, Quality::Linear] {
            for (stream, context) in cases([(44_100, 48_000), (48_000, 44_100), (96_000, 16_000)]) {
                let configuration = Configuration::new(stream, 2, Format::I16);
                let mut buffer = RingBuffer::<16_384>::new();
                let (mut producer, mut consumer) = buffer.split();
                let mut renderer = Renderer::with_rate(configuration, context, quality).unwrap();
                let reference = Rate::new(quality, 2, stream, context).unwrap();
                let mut quantum = [0.0; 256];
                let mut written = 0;

                for _ in 0..QUANTA {
                    while producer.free() >= 4 {
                        producer.write(&[1, 0, 2, 0]);
                        written += 1;
                    }

                    assert_eq!(renderer.render(&mut consumer, &mut quantum), 128);
                }

                let consumed = written - consumer.available() / 4;

                assert_eq!(renderer.underruns(), 0, "{quality:?} {stream} -> {context}");
                assert_eq!(consumed, reference.input_frames(128 * QUANTA));
            }
        }
    }

    #[test]
    fn resampled_tones_keep_their_frequency() {
        for quality in [Quality::Sinc, Quality::Linear] {
            for (stream, context) in cases([(44_100, 48_000), (22_050, 48_000), (48_000, 44_100)]) {
                let configuration = Configuration::new(stream, 1, Format::F32);
                let mut buffer = RingBuffer::<8_192>::new();
                let (mut producer, mut consumer) = buffer.split();
                let mut renderer = Renderer::with_rate(configuration, context, quality).unwrap();
                let mut quantum = [0.0; 128];
                let mut output = Vec::new();
                let mut frame = 0;

                for _ in 0..QUANTA {
                    while producer.free() >= 4 {
                        producer.write(&tone(frame, stream).to_le_bytes());
                        frame += 1;
                    }

                    renderer.render(&mut consumer, &mut quantum);
                    output.extend_from_slice(&quantum);
                }

                let steady = &output[256..];
                let peak = steady
                    .iter()
                    .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                let measured = frequency(steady, context);

                assert_eq!(renderer.underruns(), 0);
                assert!(
                    (measured - 1_000.0).abs() < 0.5,
                    "{quality:?} {stream} -> {context}: {measured} Hz"
                );
                assert!((peak - 0.5).abs() < 0.01, "{quality:?}: peak {peak}");
            }
        }
    }

    #[test]
    fn resampled_underruns_play_silence() {
        let mut buffer = RingBuffer::<1024>::new();
        let (mut producer, mut consumer) = buffer.split();
        let mut renderer = Renderer::with_rate(
            Configuration::new(44_100, 1, Format::I16),
            48_000,
            Quality::Sinc,
        )
        .unwrap();

        producer.write(&[0x10; 100]);

        let mut quantum = [1.0; 128];
        let done = renderer.render(&mut consumer, &mut quantum);

        assert_eq!(done, 55);
        assert!(quantum[done..].iter().all(|sample| *sample == 0.0));
        assert_eq!(renderer.underruns(), 1);
    }

    #[test]
    fn records_resampled_frames() {
        for quality in [Quality::Sinc, Quality::Linear] {
            for (context, stream, channels) in
                cases([(48_000, 16_000, 1), (8_000, 48_000, 2), (44_100, 48_000, 2)])
            {
                let configuration = Configuration::new(stream, channels, Format::I16);
                let mut buffer = RingBuffer::<65_536>::new();
                let (mut producer, mut consumer) = buffer.split();
                let mut recorder = Recorder::with_rate(configuration, context, quality).unwrap();
                let quantum = vec![0.25; 128 * channels as usize];
                let mut written = 0;

                for _ in 0..QUANTA {
                    written += recorder.record(&quantum, &mut producer);
                    consumer.clear();
                }

                let expected = (128 * QUANTA as u64 * stream as u64).div_ceil(context as u64);

                assert_eq!(
                    written as u64, expected,
                    "{quality:?} {context} -> {stream}"
                );
                assert_eq!(recorder.overruns(), 0);
            }
        }
    }

    #[test]
    fn resampled_recording_counts_overruns() {
        let mut buffer = RingBuffer::<64>::new();
        let (mut producer, consumer) = buffer.split();
        let mut recorder = Recorder::with_rate(
            Configuration::new(16_000, 1, Format::I16),
            8_000,
            Quality::Linear,
        )
        .unwrap();

        assert_eq!(recorder.record(&[0.5; 40], &mut producer), 32);
        assert_eq!(consumer.available(), 64);
        assert_eq!(recorder.overruns(), 1);
    }
}
