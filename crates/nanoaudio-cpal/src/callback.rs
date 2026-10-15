use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use nanoaudio_buffer::{Consumer, Producer};

use crate::convert::{CHUNK, Converter, Layout};

#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) dropouts: AtomicUsize,
    pub(crate) errors: AtomicUsize,
}

impl Counters {
    pub(crate) fn dropouts(&self) -> usize {
        self.dropouts.load(Ordering::Relaxed)
    }

    pub(crate) fn errors(&self) -> usize {
        self.errors.load(Ordering::Relaxed)
    }

    pub(crate) fn error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    fn dropout(&self) {
        self.dropouts.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) struct Playback<'a, const N: usize> {
    consumer: Consumer<'a, N>,
    converter: Converter,
    staging: [u8; CHUNK * 4],
    counters: Arc<Counters>,
}

impl<'a, const N: usize> Playback<'a, N> {
    pub(crate) fn new(
        consumer: Consumer<'a, N>,
        stream: Layout,
        device: Layout,
        counters: Arc<Counters>,
    ) -> Self {
        Self {
            consumer,
            converter: Converter::new(stream, device),
            staging: [0; CHUNK * 4],
            counters,
        }
    }

    pub(crate) fn render(&mut self, output: &mut [u8]) {
        let input_frame = self.converter.from.frame_size();
        let output_frame = self.converter.to.frame_size();
        let frames = output.len() / output_frame;
        let mut done = 0;

        if self.converter.is_identity() {
            done = frames.min(self.consumer.available() / input_frame);
            self.consumer.read(&mut output[..done * output_frame]);
        } else {
            let chunk = self.converter.chunk_frames();

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
        }

        if done < frames {
            output[done * output_frame..].fill(self.converter.to.encoding.silence());
            self.counters.dropout();
        }
    }
}

pub(crate) struct Capture<'a, const N: usize> {
    producer: Producer<'a, N>,
    converter: Converter,
    staging: [u8; CHUNK * 4],
    counters: Arc<Counters>,
}

impl<'a, const N: usize> Capture<'a, N> {
    pub(crate) fn new(
        producer: Producer<'a, N>,
        device: Layout,
        stream: Layout,
        counters: Arc<Counters>,
    ) -> Self {
        Self {
            producer,
            converter: Converter::new(device, stream),
            staging: [0; CHUNK * 4],
            counters,
        }
    }

    pub(crate) fn capture(&mut self, input: &[u8]) {
        let input_frame = self.converter.from.frame_size();
        let output_frame = self.converter.to.frame_size();
        let frames = input.len() / input_frame;
        let mut done = 0;

        if self.converter.is_identity() {
            done = frames.min(self.producer.free() / output_frame);
            self.producer.write(&input[..done * input_frame]);
        } else {
            let chunk = self.converter.chunk_frames();

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
        }

        if done < frames {
            self.counters.dropout();
        }
    }
}

#[cfg(test)]
mod tests {
    use nanoaudio_buffer::RingBuffer;
    use nanoaudio_core::Format;

    use super::*;
    use crate::convert::Encoding;

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
        let mut playback = Box::new(Playback::new(consumer, MONO_I16, MONO_U8, counters.clone()));

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
        let mut capture = Box::new(Capture::new(producer, MONO_I16, MONO_I16, counters.clone()));

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

    #[test]
    fn counts_errors() {
        let counters = Counters::default();

        counters.error();
        counters.error();

        assert_eq!(counters.errors(), 2);
    }
}
