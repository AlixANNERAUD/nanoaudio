use std::{thread, time::Duration};

use nanoaudio_buffer::RingBuffer;
use nanoaudio_core::{Configuration, Format, codec::Decoder};
use nanoaudio_cpal::{Output, Quality, default_output};
use nanoaudio_wav::{WavDecoder, WavHeader};

const CAPACITY: usize = 65_536;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: play_wav <file.wav> [sinc|linear]")?;
    let quality = match arguments.next().as_deref() {
        None | Some("sinc") => Quality::Sinc,
        Some("linear") => Quality::Linear,
        Some(_) => return Err("the quality is sinc or linear".into()),
    };
    let file = std::fs::read(path)?;
    let header = WavHeader::parse(&file)?;
    let mut decoder = WavDecoder::<i16>::new(&header)?;
    let configuration = Configuration::new(
        header.configuration.sample_rate,
        header.configuration.channels,
        Format::I16,
    );

    let buffer = Box::leak(Box::new(RingBuffer::<CAPACITY>::new()));
    let (mut producer, consumer) = buffer.split();
    let output = Output::open_device(&default_output()?, consumer, configuration, quality)?;

    println!("File:    {:?}", header.configuration);
    println!("Device:  {:?}", output.device_configuration());

    if output.device_configuration().sample_rate != configuration.sample_rate {
        println!("Resampling with {quality:?}");
    }

    let end = (header.data_offset + header.data_size as usize).min(file.len());
    let mut input = &file[header.data_offset..end];
    let mut samples = vec![0i16; decoder.minimum_output().max(4_096)];
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    let mut playing = false;
    let mut finished = false;

    loop {
        let progress = decoder.decode(input, &mut samples)?;
        input = &input[progress.consumed..];

        if progress.is_stalled() {
            if finished {
                break;
            }

            decoder.finish();
            finished = true;
            continue;
        }

        bytes.clear();
        bytes.extend(
            samples[..progress.produced]
                .iter()
                .flat_map(|sample| sample.to_le_bytes()),
        );

        let mut pending = bytes.as_slice();

        while !pending.is_empty() {
            let count = producer.write(pending);
            pending = &pending[count..];

            if !pending.is_empty() {
                if !playing {
                    output.play()?;
                    playing = true;
                }

                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    if !playing {
        output.play()?;
    }

    let underruns = output.underruns();

    while producer.free() < CAPACITY {
        thread::sleep(Duration::from_millis(5));
    }

    println!("Done with {underruns} underruns");

    Ok(())
}
