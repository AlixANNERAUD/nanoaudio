mod common;

use common::*;
use nanoaudio_core::codec::{Decoder, Progress};
use nanoaudio_core::{Configuration, Error, Format};
use nanoaudio_opus::{OPUS_SAMPLE_RATE, OpusDecoder, OpusHead};

fn reference(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| i16::from_le_bytes(*bytes))
        .collect()
}

fn snr(samples: &[i16], expected: &[i16]) -> (f64, i32) {
    let mut signal = 0.0;
    let mut noise = 0.0;
    let mut maximum = 0;

    for (&sample, &expected) in samples.iter().zip(expected) {
        let difference = sample as i32 - expected as i32;
        signal += (expected as f64).powi(2);
        noise += (difference as f64).powi(2);
        maximum = maximum.max(difference.abs());
    }

    (10.0 * (signal / noise.max(1.0)).log10(), maximum)
}

fn compare(file: &[u8], expected: &[u8], channels: u8, tolerance: i32, minimum_snr: f64) {
    let mut decoder = decoder::<i16>();
    let samples = decode_with(&mut decoder, file, file.len(), 4 * 5_760).unwrap();
    let expected = reference(expected);

    assert_eq!(
        decoder.configuration(),
        Some(Configuration::new(OPUS_SAMPLE_RATE, channels, Format::I16))
    );
    assert_eq!(samples.len(), expected.len());

    let (snr, maximum) = snr(&samples, &expected);

    assert!(maximum <= tolerance, "maximum difference {maximum}");
    assert!(snr >= minimum_snr, "signal to noise ratio {snr:.1} dB");
}

#[test]
fn mono_matches_libopus() {
    compare(
        MONO,
        include_bytes!("data/mono_440_opus_decoded.bin"),
        1,
        1,
        80.0,
    );
}

#[test]
fn stereo_matches_libopus() {
    compare(
        STEREO,
        include_bytes!("data/stereo_440_1000_opus_decoded.bin"),
        2,
        1,
        80.0,
    );
}

#[test]
fn hybrid_matches_libopus() {
    compare(
        HYBRID,
        include_bytes!("data/mono_hybrid_opus_decoded.bin"),
        1,
        1,
        80.0,
    );
}

#[test]
fn mono_packets_in_stereo_stream_match_libopus() {
    compare(
        LOW_BITRATE,
        include_bytes!("data/stereo_low_bitrate_opus_decoded.bin"),
        2,
        0,
        110.0,
    );
}

#[test]
fn output_gain_matches_libopus() {
    compare(
        GAIN,
        include_bytes!("data/mono_gain_opus_decoded.bin"),
        1,
        1,
        72.0,
    );
}

#[test]
fn pre_skip_and_end_trimming() {
    for (file, channels) in [(STEREO, 2), (MONO, 1), (LONG, 2)] {
        let mut decoder = decoder::<i16>();
        let samples = decode_with(&mut decoder, file, file.len(), 4_000).unwrap();
        let frames = final_granule_position(file) - pre_skip(file);

        assert_eq!(samples.len() as u64, frames * channels);
        assert_eq!(decoder.frames(), Some(frames));
    }

    assert_eq!(final_granule_position(STEREO) - pre_skip(STEREO), 9_600);
    assert_eq!(final_granule_position(LONG) - pre_skip(LONG), 192_000);
}

#[test]
fn small_chunks_match_one_shot() {
    for file in [MONO, STEREO, HYBRID, LOW_BITRATE] {
        let reference = decode::<i16>(file);

        for (chunk, output) in [(1, 2), (7, 37), (100, 1), (4_096, 960), (333, 11_520)] {
            let samples = decode_streaming::<i16>(file, chunk, output);

            assert_eq!(samples.unwrap(), reference, "chunk {chunk} output {output}");
        }
    }
}

#[test]
fn packets_spanning_pages() {
    for file in [MONO, STEREO, LOW_BITRATE] {
        let reference = decode::<i16>(file);

        for segments_per_page in [1, 2, 3, 17] {
            let paginated = paginate(
                &packets(file),
                segments_per_page,
                final_granule_position(file),
            );

            assert_eq!(
                decode::<i16>(&paginated),
                reference,
                "{segments_per_page} segments per page"
            );
        }
    }
}

#[test]
fn float_output_matches_integer_output() {
    let integers = decode::<i16>(STEREO);
    let floats = decode::<f32>(STEREO);

    assert_eq!(floats.len(), integers.len());

    for (float, integer) in floats.iter().zip(&integers) {
        assert!((float * 32_768.0 - *integer as f32).abs() <= 1.0);
    }
}

#[test]
fn header_information() {
    let mut decoder = decoder::<i16>();
    let mut output = vec![0i16; 2];

    assert_eq!(decoder.configuration(), None);
    assert_eq!(decoder.head(), None);
    assert_eq!(decoder.minimum_output(), 2);
    assert_eq!(decoder.minimum_input(), 65_307);

    let progress = decoder.decode(&MONO[..100], &mut output).unwrap();

    assert_eq!(progress, Progress::new(47, 0));
    assert_eq!(
        decoder.configuration(),
        Some(Configuration::new(48_000, 1, Format::I16))
    );
    assert_eq!(
        decoder.head(),
        Some(OpusHead {
            channels: 1,
            pre_skip: pre_skip(MONO) as u16,
            input_sample_rate: 48_000,
            output_gain: 0,
            mapping_family: 0,
        })
    );
    assert_eq!(decoder.minimum_output(), 1);

    decoder.reset();

    assert_eq!(decoder.configuration(), None);
    assert_eq!(
        decoder.decode(MONO, &mut output[..1]),
        Err(Error::BufferTooSmall)
    );
}

#[test]
fn const_decoder_with_short_packets() {
    static DECODER: std::sync::Mutex<OpusDecoder<i16, 960>> =
        std::sync::Mutex::new(OpusDecoder::new());

    let mut decoder = DECODER.lock().unwrap();
    let mut output = vec![0; 4_000];
    let mut produced = Vec::new();
    let mut input = STEREO;

    decoder.finish();

    loop {
        let progress = decoder.decode(input, &mut output).unwrap();
        input = &input[progress.consumed..];
        produced.extend_from_slice(&output[..progress.produced]);

        if progress.is_stalled() {
            break;
        }
    }

    assert_eq!(produced, decode::<i16>(STEREO));
}

#[test]
fn rejects_long_packets_in_a_short_decoder() {
    let mut packets = packets(MONO);
    let toc = packets[2][0];
    packets[2] = vec![(toc & !3) | 1, 0, 0];

    let file = paginate(&packets, 255, final_granule_position(MONO));
    let mut slot = Box::new(std::mem::MaybeUninit::<OpusDecoder<i16, 120>>::uninit());
    let decoder = OpusDecoder::init(&mut slot);
    let mut output = vec![0; 4_000];

    decoder.finish();

    let error = (0..10).find_map(|_| decoder.decode(&file, &mut output).err());

    assert_eq!(error, Some(Error::UnsupportedFormat));
}

#[test]
fn bad_checksum_is_concealed() {
    let reference = decode::<i16>(LONG);
    let page = pages(LONG)[4];
    let mut file = LONG.to_vec();
    file[page.0 + page.1 / 2] ^= 0x40;

    let samples = decode_streaming::<i16>(&file, 4_096, 4_000).unwrap();

    assert_eq!(samples.len(), reference.len());
    assert_eq!(samples[..60_000], reference[..60_000]);
    assert_ne!(samples, reference);
}

#[test]
fn lost_pages_are_concealed() {
    let reference = decode::<i16>(LONG);
    let pages = pages(LONG);
    let lost = pages[3];
    let mut file = LONG[..lost.0].to_vec();
    file.extend_from_slice(&LONG[lost.0 + lost.1..]);

    let samples = decode_streaming::<i16>(&file, 4_096, 4_000).unwrap();
    let start = 2 * (granule_position(LONG, pages[2]) - pre_skip(LONG)) as usize;
    let end = 2 * (granule_position(LONG, pages[3]) - pre_skip(LONG)) as usize;

    assert_eq!(samples.len(), reference.len());
    assert_eq!(samples[..start], reference[..start]);
    assert!(
        samples[start..end]
            .iter()
            .zip(&reference[start..end])
            .any(|(a, b)| a != b)
    );
    let (near, _) = snr(&samples[end + 9_600..], &reference[end + 9_600..]);
    let (far, _) = snr(&samples[end + 96_000..], &reference[end + 96_000..]);

    assert!(
        near > 30.0,
        "signal to noise ratio {near:.1} dB 100 ms after the loss"
    );
    assert!(
        far > 100.0,
        "signal to noise ratio {far:.1} dB 1 s after the loss"
    );
}

#[test]
fn junk_between_pages_is_skipped() {
    let reference = decode::<i16>(LONG);
    let pages = pages(LONG);
    let mut file = Vec::new();

    for (index, &(start, size)) in pages.iter().enumerate() {
        file.extend_from_slice(&LONG[start..start + size]);

        if index == 4 {
            file.extend_from_slice(b"OggSjunkOgg\0\0OggS");
        }
    }

    assert_eq!(
        decode_streaming::<i16>(&file, 1_000, 4_000).unwrap(),
        reference
    );
}

#[test]
fn chained_and_multiplexed_streams_keep_the_first() {
    let reference = decode::<i16>(STEREO);
    let mut other = MONO.to_vec();

    for (start, size) in pages(MONO) {
        other[start + 14..start + 18].copy_from_slice(&7u32.to_le_bytes());
        other[start + 22..start + 26].fill(0);
        let checksum = crc(&other[start..start + size]);
        other[start + 22..start + 26].copy_from_slice(&checksum.to_le_bytes());
    }

    let mut file = Vec::new();
    let stereo = pages(STEREO);
    let mono = pages(MONO);

    for (index, &(start, size)) in stereo.iter().enumerate() {
        file.extend_from_slice(&STEREO[start..start + size]);

        if let Some(&(start, size)) = mono.get(index) {
            file.extend_from_slice(&other[start..start + size]);
        }
    }

    assert_eq!(
        decode_streaming::<i16>(&file, 500, 4_000).unwrap(),
        reference
    );
}

#[test]
fn rejects_unsupported_streams() {
    let mut packets = packets(MONO);
    packets[0][18] = 2;
    let file = paginate(&packets, 255, final_granule_position(MONO));

    assert_eq!(
        decode_streaming::<i16>(&file, file.len(), 4_000),
        Err(Error::UnsupportedFormat)
    );

    let mut packets = self::packets(MONO);
    packets[0][..8].copy_from_slice(b"OpusHeat");
    let file = paginate(&packets, 255, final_granule_position(MONO));

    assert_eq!(
        decode_streaming::<i16>(&file, file.len(), 4_000),
        Err(Error::InvalidHeader)
    );

    assert_eq!(
        decode_streaming::<i16>(b"RIFF....WAVEfmt ", 16, 4_000),
        Err(Error::InvalidHeader)
    );
}

#[test]
fn accepts_plain_mapping_family_one() {
    let reference = decode::<i16>(STEREO);
    let mut packets = packets(STEREO);
    packets[0][18] = 1;
    packets[0].extend_from_slice(&[1, 1, 0, 1]);
    let file = paginate(&packets, 255, final_granule_position(STEREO));

    assert_eq!(decode::<i16>(&file), reference);

    packets[0][21] = 1;
    packets[0][22] = 0;
    let file = paginate(&packets, 255, final_granule_position(STEREO));

    assert_eq!(
        decode_streaming::<i16>(&file, file.len(), 4_000),
        Err(Error::UnsupportedFormat)
    );
}

#[test]
fn truncated_input_never_panics() {
    for file in [MONO, STEREO, LOW_BITRATE] {
        let reference = decode::<i16>(file);

        for length in (0..file.len()).step_by(97) {
            match decode_streaming::<i16>(&file[..length], 512, 1_000) {
                Ok(samples) => assert!(reference.starts_with(&samples)),
                Err(error) => assert_eq!(error, Error::Incomplete),
            }
        }
    }
}

#[test]
fn corrupt_packets_never_panic() {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for file in [MONO, STEREO, HYBRID, LOW_BITRATE] {
        let length = decode::<i16>(file).len();

        for _ in 0..40 {
            let mut packets = packets(file);

            for packet in packets.iter_mut().skip(2) {
                match random() % 3 {
                    0 => {
                        let index = random() as usize % packet.len();
                        packet[index] ^= 1 << (random() % 8);
                    }
                    1 => packet.truncate(random() as usize % (packet.len() + 1)),
                    _ => packet.iter_mut().for_each(|byte| *byte = random() as u8),
                }
            }

            let file = paginate(&packets, 255, final_granule_position(file));

            if let Ok(samples) = decode_streaming::<i16>(&file, 1_000, 2_000) {
                assert!(samples.len() <= length * 4);
            }
        }
    }
}
