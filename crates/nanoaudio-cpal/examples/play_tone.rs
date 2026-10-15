use std::{f32::consts::TAU, thread, time::Duration};

use nanoaudio_buffer::RingBuffer;
use nanoaudio_core::{Configuration, Format};
use nanoaudio_cpal::{Error, Output, Quality, default_output};

const CAPACITY: usize = 32_768;

fn main() -> Result<(), Error> {
    let seconds: f32 = std::env::args()
        .nth(1)
        .and_then(|argument| argument.parse().ok())
        .unwrap_or(2.0);

    let device = default_output()?;
    let preferred = Output::preferred_configuration(&device)?;
    let configuration = Configuration::new(preferred.sample_rate, preferred.channels, Format::I16);

    let buffer = Box::leak(Box::new(RingBuffer::<CAPACITY>::new()));
    let (mut producer, consumer) = buffer.split();
    let output = Output::open_device(&device, consumer, configuration, Quality::Sinc)?;

    println!("Stream:  {configuration:?}");
    println!("Device:  {:?}", output.device_configuration());

    let frame_size = configuration.frame_size();
    let total = (seconds * configuration.sample_rate as f32) as usize;
    let step = TAU * 440.0 / configuration.sample_rate as f32;
    let mut phase = 0.0f32;
    let mut written = 0;
    let mut frame = [0u8; 2 * u8::MAX as usize];

    let mut fill = |producer: &mut nanoaudio_buffer::Producer<'_, CAPACITY>| {
        while written < total && producer.free() >= frame_size {
            let sample = ((phase.sin() * 0.2) * i16::MAX as f32) as i16;

            for channel in frame[..frame_size].as_chunks_mut::<2>().0 {
                *channel = sample.to_le_bytes();
            }

            producer.write(&frame[..frame_size]);
            phase = (phase + step) % TAU;
            written += 1;
        }

        written < total
    };

    fill(&mut producer);
    output.play()?;

    while fill(&mut producer) {
        thread::sleep(Duration::from_millis(5));
    }

    let underruns = output.underruns();

    while producer.free() < CAPACITY {
        thread::sleep(Duration::from_millis(5));
    }

    println!(
        "Played {seconds} s with {underruns} underruns and {} backend errors",
        output.errors()
    );

    Ok(())
}
