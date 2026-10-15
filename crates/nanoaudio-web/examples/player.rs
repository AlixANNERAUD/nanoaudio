#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod web {
    use core::f32::consts::TAU;

    use js_sys::{Array, Function, Uint8Array};
    use nanoaudio_core::{Sample, codec::Decoder};
    use nanoaudio_wav::{WavDecoder, WavHeader};
    use nanoaudio_web::{
        CAPACITY, Configuration, Consumer, Format, Input, Options, Output, Port, Producer, Quality,
        Sink, Source, Worklet,
    };
    use wasm_bindgen::prelude::*;

    const SINE: Configuration = Configuration::new(48_000, 2, Format::F32);
    const LEVEL: Configuration = Configuration::new(48_000, 1, Format::F32);
    const TARGET_FRAMES: usize = 256;

    fn missing(producer: &Producer<'_, CAPACITY>, configuration: Configuration) -> usize {
        let buffered = CAPACITY - producer.free();
        configuration
            .frames_to_bytes(TARGET_FRAMES)
            .saturating_sub(buffered)
    }

    struct Sine {
        phase: f32,
    }

    impl Source for Sine {
        fn fill(&mut self, producer: &mut Producer<'_, CAPACITY>, _: &Port) {
            let frames = SINE.bytes_to_frames(missing(producer, SINE));
            let step = TAU * 440.0 / SINE.sample_rate as f32;

            for _ in 0..frames {
                let mut frame = [0; 8];
                let sample = 0.2 * self.phase.sin();
                sample.write_le(&mut frame[..4]);
                sample.write_le(&mut frame[4..]);
                producer.write(&frame);
                self.phase = (self.phase + step) % TAU;
            }
        }
    }

    #[wasm_bindgen]
    pub fn sine() -> Worklet {
        Worklet::output(Sine { phase: 0.0 })
    }

    #[derive(Default)]
    struct Wav {
        file: Vec<u8>,
        position: usize,
        decoder: Option<WavDecoder<i16>>,
        configuration: Option<Configuration>,
        samples: Vec<i16>,
        pending: Vec<u8>,
        written: usize,
    }

    impl Source for Wav {
        fn message(&mut self, data: JsValue, _: &Port) {
            let file = Uint8Array::new(&data).to_vec();

            if let Ok(header) = WavHeader::parse(&file)
                && let Ok(decoder) = WavDecoder::new(&header)
            {
                self.samples = vec![0; decoder.minimum_output().max(1024)];
                self.configuration = decoder.configuration();
                self.decoder = Some(decoder);
                self.position = header.data_offset;
                self.file = file;
            }
        }

        fn fill(&mut self, producer: &mut Producer<'_, CAPACITY>, _: &Port) {
            let (Some(decoder), Some(configuration)) = (&mut self.decoder, self.configuration)
            else {
                return;
            };
            let mut missing = missing(producer, configuration);

            while missing > 0 {
                if self.written == self.pending.len() {
                    let input = &self.file[self.position.min(self.file.len())..];
                    let Ok(progress) = decoder.decode(input, &mut self.samples) else {
                        return;
                    };

                    if progress.produced == 0 {
                        return;
                    }

                    self.position += progress.consumed;
                    self.pending.clear();
                    self.written = 0;

                    for sample in &self.samples[..progress.produced] {
                        self.pending.extend_from_slice(&sample.to_le_bytes());
                    }
                }

                let end = self.pending.len().min(self.written + missing);
                let count = producer.write(&self.pending[self.written..end]);
                self.written += count;
                missing -= count;

                if count == 0 {
                    return;
                }
            }
        }
    }

    #[wasm_bindgen]
    pub fn wav() -> Worklet {
        Worklet::output(Wav::default())
    }

    #[derive(Default)]
    struct Level {
        peak: f32,
        frames: usize,
    }

    impl Sink for Level {
        fn drain(&mut self, consumer: &mut Consumer<'_, CAPACITY>, port: &Port) {
            let mut bytes = [0; 512];

            loop {
                let count = consumer.read(&mut bytes);

                if count == 0 {
                    break;
                }

                for sample in bytes[..count].as_chunks::<4>().0 {
                    self.peak = self.peak.max(f32::from_le_bytes(*sample).abs());
                }

                self.frames += count / 4;

                if self.frames >= LEVEL.sample_rate as usize / 10 {
                    let _ = port.post(&JsValue::from(self.peak));
                    self.peak = 0.0;
                    self.frames = 0;
                }
            }
        }
    }

    #[wasm_bindgen]
    pub fn level() -> Worklet {
        Worklet::input(Level::default())
    }

    #[wasm_bindgen]
    pub struct Playback(Output);

    #[wasm_bindgen]
    impl Playback {
        pub fn underruns(&self) -> u32 {
            self.0.underruns()
        }

        pub fn sample_rate(&self) -> u32 {
            self.0.configuration().sample_rate
        }

        pub fn device_sample_rate(&self) -> u32 {
            self.0.device_sample_rate()
        }
    }

    #[wasm_bindgen]
    pub async fn play_sine() -> Result<Playback, JsValue> {
        Ok(Playback(Output::open(SINE, "sine").await?))
    }

    #[wasm_bindgen]
    pub async fn play_wav(
        file: Vec<u8>,
        context_rate: u32,
        linear: bool,
    ) -> Result<Playback, JsValue> {
        let header = WavHeader::parse(&file).map_err(|error| JsError::new(&error.to_string()))?;
        let configuration = Configuration::new(
            header.configuration.sample_rate,
            header.configuration.channels,
            Format::I16,
        );
        let options = Options {
            quality: if linear {
                Quality::Linear
            } else {
                Quality::Sinc
            },
            sample_rate: (context_rate != 0).then_some(context_rate),
        };
        let output = Output::open_with(configuration, "wav", options).await?;
        let bytes = Uint8Array::from(file.as_slice());
        output.post_with_transfer(&bytes, &Array::of1(&bytes.buffer()))?;
        Ok(Playback(output))
    }

    #[wasm_bindgen]
    pub struct Capture(Input);

    #[wasm_bindgen]
    impl Capture {
        pub fn overruns(&self) -> u32 {
            self.0.overruns()
        }
    }

    #[wasm_bindgen]
    pub async fn listen(on_level: Function) -> Result<Capture, JsValue> {
        let input = Input::open(LEVEL, "level").await?;
        input.set_on_message(move |level| {
            let _ = on_level.call1(&JsValue::NULL, &level);
        });
        Ok(Capture(input))
    }
}
