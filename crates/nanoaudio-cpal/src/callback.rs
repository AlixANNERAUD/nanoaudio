use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use nanoaudio_buffer::{Consumer, Producer};
use nanoaudio_core::Format;
use nanoaudio_dsp::{Float, Stream, samples_using};

use crate::{
    convert::{CHUNK, Converter, Encoding, Layout},
    resample::Rate,
};

#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) dropouts: AtomicU32,
    pub(crate) errors: AtomicU32,
}

impl Counters {
    pub(crate) fn dropouts(&self) -> u32 {
        self.dropouts.load(Ordering::Relaxed)
    }

    pub(crate) fn errors(&self) -> u32 {
        self.errors.load(Ordering::Relaxed)
    }

    pub(crate) fn error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    fn dropout(&self) {
        self.dropouts.fetch_add(1, Ordering::Relaxed);
    }
}

fn format(layout: Layout) -> Format {
    match layout.encoding {
        Encoding::Packed(format) => format,
        Encoding::Padded24 => Format::I32,
    }
}

fn direct(device: Layout, stream: Layout) -> Option<Format> {
    match device.encoding {
        Encoding::Packed(format) if device.channels == stream.channels => Some(format),
        _ => None,
    }
}

fn floats(stream: Layout) -> Layout {
    Layout {
        encoding: Encoding::Packed(Format::F32),
        channels: stream.channels,
    }
}

pub(crate) struct Playback<'a, const N: usize> {
    consumer: Consumer<'a, N>,
    stream: Layout,
    converter: Converter,
    resampler: Option<Rate>,
    staging: [u8; CHUNK * 4],
    resampled: [u8; CHUNK * 4],
    counters: Arc<Counters>,
}

impl<'a, const N: usize> Playback<'a, N> {
    pub(crate) fn new(
        consumer: Consumer<'a, N>,
        stream: Layout,
        device: Layout,
        resampler: Option<Rate>,
        counters: Arc<Counters>,
    ) -> Self {
        let from = if resampler.is_some() {
            floats(stream)
        } else {
            stream
        };

        Self {
            consumer,
            stream,
            converter: Converter::new(from, device),
            resampler,
            staging: [0; CHUNK * 4],
            resampled: [0; CHUNK * 4],
            counters,
        }
    }

    pub(crate) fn render(&mut self, output: &mut [u8]) {
        let output_frame = self.converter.to.frame_size();
        let frames = output.len() / output_frame;
        let done = if self.resampler.is_some() {
            self.resample(output, frames)
        } else if self.converter.is_identity() {
            let done = frames.min(self.consumer.available() / output_frame);
            self.consumer.read(&mut output[..done * output_frame]);
            done
        } else {
            self.convert(output, frames)
        };

        if done < frames {
            output[done * output_frame..].fill(self.converter.to.encoding.silence());
            self.counters.dropout();
        }
    }

    fn convert(&mut self, output: &mut [u8], frames: usize) -> usize {
        let input_frame = self.converter.from.frame_size();
        let output_frame = self.converter.to.frame_size();
        let chunk = self.converter.chunk_frames();
        let mut done = 0;

        while done < frames {
            let count = (frames - done)
                .min(chunk)
                .min(self.consumer.available() / input_frame);

            if count == 0 {
                break;
            }

            let staged = &mut self.staging[..count * input_frame];
            self.consumer.read(staged);
            self.converter.convert(
                staged,
                &mut output[done * output_frame..(done + count) * output_frame],
            );
            done += count;
        }

        done
    }

    fn resample(&mut self, output: &mut [u8], frames: usize) -> usize {
        let Some(resampler) = self.resampler.as_mut() else {
            return 0;
        };
        let input_frame = self.stream.frame_size();
        let output_frame = self.converter.to.frame_size();
        let channels = self.stream.channels;
        let from = format(self.stream);
        let direct = direct(self.converter.to, self.stream);
        let capacity = self.staging.len() / input_frame;
        let chunk = self.converter.chunk_frames();
        let mut done = 0;

        while done < frames {
            let available = (self.consumer.available() / input_frame).min(capacity);
            let count = (frames - done)
                .min(chunk)
                .min(resampler.output_frames(available));

            if count == 0 {
                break;
            }

            let staged = &mut self.staging[..resampler.input_frames(count) * input_frame];
            self.consumer.read(staged);
            let target = &mut output[done * output_frame..(done + count) * output_frame];

            match direct {
                Some(to) => {
                    resampler.convert(staged, from, channels, target, to);
                }
                None => {
                    let resampled = &mut self.resampled[..count * channels as usize * 4];
                    resampler.convert(staged, from, channels, resampled, Format::F32);
                    self.converter.convert(resampled, target);
                }
            }

            done += count;
        }

        done
    }
}

pub(crate) struct Capture<'a, const N: usize> {
    producer: Producer<'a, N>,
    stream: Layout,
    converter: Converter,
    resampler: Option<Rate>,
    staging: [u8; CHUNK * 4],
    converted: [u8; CHUNK * 4],
    input: [f32; CHUNK],
    output: [f32; CHUNK],
    counters: Arc<Counters>,
}

impl<'a, const N: usize> Capture<'a, N> {
    pub(crate) fn new(
        producer: Producer<'a, N>,
        device: Layout,
        stream: Layout,
        resampler: Option<Rate>,
        counters: Arc<Counters>,
    ) -> Self {
        let to = if resampler.is_some() {
            floats(stream)
        } else {
            stream
        };

        Self {
            producer,
            stream,
            converter: Converter::new(device, to),
            resampler,
            staging: [0; CHUNK * 4],
            converted: [0; CHUNK * 4],
            input: [0.0; CHUNK],
            output: [0.0; CHUNK],
            counters,
        }
    }

    pub(crate) fn capture(&mut self, input: &[u8]) {
        let input_frame = self.converter.from.frame_size();
        let frames = input.len() / input_frame;
        let complete = if self.resampler.is_some() {
            self.resample(&input[..frames * input_frame])
        } else if self.converter.is_identity() {
            let output_frame = self.converter.to.frame_size();
            let done = frames.min(self.producer.free() / output_frame);
            self.producer.write(&input[..done * input_frame]);
            done == frames
        } else {
            self.convert(input, frames)
        };

        if !complete {
            self.counters.dropout();
        }
    }

    fn convert(&mut self, input: &[u8], frames: usize) -> bool {
        let input_frame = self.converter.from.frame_size();
        let output_frame = self.converter.to.frame_size();
        let chunk = self.converter.chunk_frames();
        let mut done = 0;

        while done < frames {
            let count = (frames - done)
                .min(chunk)
                .min(self.producer.free() / output_frame);

            if count == 0 {
                break;
            }

            let staged = &mut self.staging[..count * output_frame];
            self.converter.convert(
                &input[done * input_frame..(done + count) * input_frame],
                staged,
            );
            self.producer.write(staged);
            done += count;
        }

        done == frames
    }

    fn resample(&mut self, input: &[u8]) -> bool {
        let Some(resampler) = self.resampler.as_mut() else {
            return false;
        };
        let input_frame = self.converter.from.frame_size();
        let output_frame = self.stream.frame_size();
        let channels = self.stream.channels;
        let format = format(self.stream);
        let room =
            (CHUNK / channels as usize).min(self.staging.len() / output_frame) * channels as usize;
        let mut complete = true;

        for part in input.chunks(self.converter.chunk_frames() * input_frame) {
            let samples = part.len() / input_frame * channels as usize;
            let floats = if self.converter.is_identity() {
                part
            } else {
                let converted = &mut self.converted[..samples * 4];
                self.converter.convert(part, converted);
                &*converted
            };

            for (sample, bytes) in self.input.iter_mut().zip(floats.as_chunks::<4>().0) {
                *sample = f32::from_le_bytes(*bytes);
            }

            let mut pending = &self.input[..samples];

            loop {
                let progress = resampler.process(pending, &mut self.output[..room]);
                pending = &pending[progress.consumed..];

                let frames = progress.produced / channels as usize;
                let bytes = frames * output_frame;
                let encoded = &mut self.staging[..bytes];

                if let Ok(mut source) =
                    samples_using::<Float, f32>(&self.output[..progress.produced], channels)
                {
                    source.write_bytes(encoded, format);
                }

                let fits = (self.producer.free() / output_frame * output_frame).min(bytes);
                self.producer.write(&encoded[..fits]);
                complete &= fits == bytes;

                if pending.is_empty() && progress.produced < room {
                    break;
                }
            }
        }

        complete
    }
}

#[cfg(test)]
mod tests {
    use nanoaudio_buffer::RingBuffer;
    use nanoaudio_core::Format;

    use super::*;
    use crate::resample::Quality;

    const STEREO_I16: Layout = Layout {
        encoding: Encoding::Packed(Format::I16),
        channels: 2,
    };
    const STEREO_F32: Layout = Layout {
        encoding: Encoding::Packed(Format::F32),
        channels: 2,
    };
    const MONO_I16: Layout = Layout {
        encoding: Encoding::Packed(Format::I16),
        channels: 1,
    };
    const MONO_U8: Layout = Layout {
        encoding: Encoding::Packed(Format::U8),
        channels: 1,
    };

    fn i16_bytes(samples: &[i16]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    fn f32_bytes(samples: &[f32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn plays_matching_layouts_unchanged() {
        let mut buffer = RingBuffer::<64>::new();
        let (mut producer, consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let mut playback = Box::new(Playback::new(
            consumer,
            STEREO_I16,
            STEREO_I16,
            None,
            counters.clone(),
        ));
        let samples = i16_bytes(&[1, 2, 3, 4, 5, 6]);

        producer.write(&samples);

        let mut output = [0xAAu8; 8];
        playback.render(&mut output);

        assert_eq!(output, samples[..8]);
        assert_eq!(counters.dropouts(), 0);
        assert_eq!(producer.free(), 60);
    }

    #[test]
    fn converts_while_playing() {
        let mut buffer = RingBuffer::<64>::new();
        let (mut producer, consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let mut playback = Box::new(Playback::new(
            consumer,
            MONO_I16,
            STEREO_F32,
            None,
            counters.clone(),
        ));

        producer.write(&i16_bytes(&[16_384, -16_384]));

        let mut output = [0u8; 16];
        playback.render(&mut output);

        assert_eq!(output.as_slice(), f32_bytes(&[0.5, 0.5, -0.5, -0.5]));
        assert_eq!(counters.dropouts(), 0);
    }

    #[test]
    fn fills_silence_and_counts_underruns() {
        let mut buffer = RingBuffer::<64>::new();
        let (mut producer, consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let mut playback = Box::new(Playback::new(
            consumer,
            MONO_I16,
            MONO_U8,
            None,
            counters.clone(),
        ));

        producer.write(&i16_bytes(&[i16::MAX, i16::MIN]));
        producer.write(&[0x12]);

        let mut output = [0u8; 4];
        playback.render(&mut output);

        assert_eq!(output, [0xFF, 0x00, 0x80, 0x80]);
        assert_eq!(counters.dropouts(), 1);

        playback.render(&mut output);

        assert_eq!(output, [0x80; 4]);
        assert_eq!(counters.dropouts(), 2);

        producer.write(&[0x34, 0x00, 0x00]);
        let mut output = [0u8; 2];
        playback.render(&mut output);

        assert_eq!(output, [0x80 + 0x34, 0x80]);
        assert_eq!(counters.dropouts(), 2);
    }

    #[test]
    fn identity_underrun_keeps_whole_frames() {
        let mut buffer = RingBuffer::<64>::new();
        let (mut producer, consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let mut playback = Box::new(Playback::new(
            consumer,
            STEREO_I16,
            STEREO_I16,
            None,
            counters.clone(),
        ));

        producer.write(&i16_bytes(&[7, 8, 9]));

        let mut output = [0xAAu8; 8];
        playback.render(&mut output);

        assert_eq!(output.as_slice(), i16_bytes(&[7, 8, 0, 0]));
        assert_eq!(counters.dropouts(), 1);
    }

    #[test]
    fn plays_across_the_wrap_and_in_chunks() {
        let mut buffer = RingBuffer::<6000>::new();
        let (mut producer, consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let mut playback = Box::new(Playback::new(
            consumer,
            MONO_I16,
            STEREO_I16,
            None,
            counters.clone(),
        ));
        let mut next = 0i16;
        let mut expected = 0i16;

        for _ in 0..if cfg!(miri) { 3 } else { 20 } {
            let samples: Vec<i16> = (0..1_499)
                .map(|_| {
                    next = next.wrapping_add(1);
                    next
                })
                .collect();

            assert_eq!(producer.write(&i16_bytes(&samples)), 2_998);

            let mut output = vec![0u8; 1_499 * 4];
            playback.render(&mut output);

            for frame in output.as_chunks::<4>().0 {
                expected = expected.wrapping_add(1);
                let left = i16::from_le_bytes([frame[0], frame[1]]);
                let right = i16::from_le_bytes([frame[2], frame[3]]);

                assert_eq!((left, right), (expected, expected));
            }
        }

        assert_eq!(counters.dropouts(), 0);
    }

    #[test]
    fn captures_with_conversion() {
        let mut buffer = RingBuffer::<64>::new();
        let (producer, mut consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let mut capture = Box::new(Capture::new(
            producer,
            STEREO_F32,
            MONO_I16,
            None,
            counters.clone(),
        ));

        capture.capture(&f32_bytes(&[0.5, 0.5, -0.25, -0.25]));

        let mut output = [0u8; 8];
        assert_eq!(consumer.read(&mut output), 4);
        assert_eq!(output[..4], i16_bytes(&[16_384, -8_192]));
        assert_eq!(counters.dropouts(), 0);
    }

    #[test]
    fn drops_and_counts_overruns() {
        let mut buffer = RingBuffer::<7>::new();
        let (producer, mut consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let mut capture = Box::new(Capture::new(
            producer,
            MONO_I16,
            MONO_I16,
            None,
            counters.clone(),
        ));

        capture.capture(&i16_bytes(&[1, 2, 3, 4, 5]));

        assert_eq!(consumer.available(), 6);
        assert_eq!(counters.dropouts(), 1);

        consumer.clear();
        capture.capture(&i16_bytes(&[6]));

        let mut output = [0u8; 2];
        assert_eq!(consumer.read(&mut output), 2);
        assert_eq!(output.as_slice(), i16_bytes(&[6]));
        assert_eq!(counters.dropouts(), 1);
    }

    fn mono(format: Format) -> Layout {
        Layout {
            encoding: Encoding::Packed(format),
            channels: 1,
        }
    }

    fn tone(frames: std::ops::Range<usize>, rate: u32, frequency: f64) -> Vec<f32> {
        frames
            .map(|frame| {
                (0.5 * (std::f64::consts::TAU * frequency * frame as f64 / rate as f64).sin())
                    as f32
            })
            .collect()
    }

    fn frequency(samples: &[f32], rate: u32) -> f64 {
        let crossings: Vec<f64> = samples
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[0] < 0.0 && pair[1] >= 0.0)
            .map(|(index, pair)| index as f64 + (pair[0] / (pair[0] - pair[1])) as f64)
            .collect();
        let first = crossings[0];
        let last = crossings[crossings.len() - 1];

        (crossings.len() - 1) as f64 * rate as f64 / (last - first)
    }

    fn cases<T>(cases: impl IntoIterator<Item = T>) -> impl Iterator<Item = T> {
        cases
            .into_iter()
            .take(if cfg!(miri) { 1 } else { usize::MAX })
    }

    const CALLBACKS: [usize; 7] = [512, 441, 480, 1, 1024, 4096, 333];

    fn play_resampled(
        quality: Quality,
        stream: Layout,
        device: Layout,
        rates: (u32, u32),
        mut sample: impl FnMut(usize) -> Vec<u8>,
        callbacks: usize,
    ) -> (Vec<u8>, usize, usize, u32) {
        let mut buffer = Box::new(RingBuffer::<131_072>::new());
        let (mut producer, consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let resampler = Rate::new(quality, stream.channels, rates.0, rates.1).unwrap();
        let mut playback = Box::new(Playback::new(
            consumer,
            stream,
            device,
            Some(resampler),
            counters.clone(),
        ));
        let mut written = 0;
        let mut pending = Vec::new();
        let mut output = Vec::new();
        let mut frames = 0;

        for callback in 0..callbacks {
            loop {
                if pending.is_empty() {
                    pending = sample(written);
                }

                if producer.free() < pending.len() {
                    break;
                }

                producer.write(&pending);
                pending.clear();
                written += 1;
            }

            let count = CALLBACKS[callback % CALLBACKS.len()];
            let mut data = vec![0xAA; count * device.frame_size()];
            playback.render(&mut data);
            output.extend_from_slice(&data);
            frames += count;
        }

        drop(playback);
        let consumed = written - producer.free().abs_diff(131_072) / stream.frame_size();

        (output, frames, consumed, counters.dropouts())
    }

    #[test]
    fn resamples_with_exact_frame_accounting() {
        let callbacks = if cfg!(miri) { 4 } else { 200 };
        let layouts = [
            (STEREO_I16, STEREO_I16),
            (MONO_I16, STEREO_F32),
            (
                STEREO_F32,
                Layout {
                    encoding: Encoding::Padded24,
                    channels: 2,
                },
            ),
        ];

        for quality in [Quality::Sinc, Quality::Linear] {
            for (stream, device) in cases(layouts) {
                for rates in cases([(44_100, 48_000), (48_000, 44_100), (48_000, 16_000)]) {
                    let frame = vec![0x11; stream.frame_size()];
                    let (_, frames, consumed, dropouts) = play_resampled(
                        quality,
                        stream,
                        device,
                        rates,
                        |_| frame.clone(),
                        callbacks,
                    );
                    let reference = Rate::new(quality, stream.channels, rates.0, rates.1).unwrap();

                    assert_eq!(dropouts, 0, "{quality:?} {rates:?}");
                    assert_eq!(
                        consumed,
                        reference.input_frames(frames),
                        "{quality:?} {rates:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn resampled_tones_keep_their_frequency() {
        let callbacks = if cfg!(miri) { 4 } else { 120 };

        for quality in [Quality::Sinc, Quality::Linear] {
            for (input, output) in cases([(44_100, 48_000), (48_000, 22_050), (22_050, 44_100)]) {
                let (data, frames, _, dropouts) = play_resampled(
                    quality,
                    mono(Format::F32),
                    mono(Format::F32),
                    (input, output),
                    |frame| {
                        tone(frame..frame + 1, input, 1_000.0)[0]
                            .to_le_bytes()
                            .to_vec()
                    },
                    callbacks,
                );
                let samples: Vec<f32> = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| f32::from_le_bytes(*bytes))
                    .collect();
                let steady = &samples[256..frames];
                let peak = steady
                    .iter()
                    .fold(0.0f32, |peak, sample| peak.max(sample.abs()));

                assert_eq!(dropouts, 0);
                assert!(
                    (frequency(steady, output) - 1_000.0).abs() < 0.5,
                    "{quality:?} {input} -> {output}: {} Hz",
                    frequency(steady, output)
                );
                assert!((peak - 0.5).abs() < 0.01, "{quality:?}: peak {peak}");
            }
        }
    }

    #[test]
    fn resampled_underruns_play_silence() {
        let mut buffer = RingBuffer::<4096>::new();
        let (mut producer, consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let resampler = Rate::new(Quality::Sinc, 1, 44_100, 48_000).unwrap();
        let mut playback = Box::new(Playback::new(
            consumer,
            MONO_I16,
            STEREO_F32,
            Some(resampler),
            counters.clone(),
        ));

        producer.write(&i16_bytes(&[1_000; 100]));

        let mut output = vec![0xAAu8; 256 * 8];
        playback.render(&mut output);

        assert_eq!(counters.dropouts(), 1);
        assert_eq!(producer.free(), 4096);
        assert!(output[200 * 8..].iter().all(|byte| *byte == 0));

        playback.render(&mut output);

        assert_eq!(counters.dropouts(), 2);
        assert!(output.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn captures_resampled_frames() {
        let rounds = if cfg!(miri) { 3 } else { 100 };

        for quality in [Quality::Sinc, Quality::Linear] {
            for (device, stream, rates) in cases([
                (STEREO_F32, MONO_I16, (48_000, 16_000)),
                (MONO_I16, MONO_I16, (8_000, 48_000)),
                (STEREO_I16, STEREO_F32, (44_100, 48_000)),
            ]) {
                let mut buffer = Box::new(RingBuffer::<65_536>::new());
                let (producer, mut consumer) = buffer.split();
                let counters = Arc::new(Counters::default());
                let resampler = Rate::new(quality, stream.channels, rates.0, rates.1).unwrap();
                let mut capture = Box::new(Capture::new(
                    producer,
                    device,
                    stream,
                    Some(resampler),
                    counters.clone(),
                ));
                let mut input = 0;
                let mut output = 0;

                for round in 0..rounds {
                    let count = CALLBACKS[round % CALLBACKS.len()];
                    capture.capture(&vec![0; count * device.frame_size()]);
                    input += count;
                    output += consumer.available() / stream.frame_size();
                    consumer.clear();
                }

                let expected = (input as u64 * rates.1 as u64).div_ceil(rates.0 as u64);

                assert_eq!(output as u64, expected, "{quality:?} {rates:?}");
                assert_eq!(counters.dropouts(), 0);
            }
        }
    }

    #[test]
    fn resampled_capture_counts_overruns() {
        let mut buffer = RingBuffer::<64>::new();
        let (producer, consumer) = buffer.split();
        let counters = Arc::new(Counters::default());
        let resampler = Rate::new(Quality::Linear, 1, 8_000, 16_000).unwrap();
        let mut capture = Box::new(Capture::new(
            producer,
            MONO_I16,
            MONO_I16,
            Some(resampler),
            counters.clone(),
        ));

        capture.capture(&i16_bytes(&[100; 40]));

        assert_eq!(consumer.available(), 64);
        assert_eq!(counters.dropouts(), 1);
    }

    #[test]
    fn counts_errors() {
        let counters = Counters::default();

        counters.error();
        counters.error();

        assert_eq!(counters.errors(), 2);
    }
}
