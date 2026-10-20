#![allow(dead_code)]

use std::mem::MaybeUninit;

use nanoaudio_core::codec::Decoder;
use nanoaudio_core::{Error, Sample};
use nanoaudio_opus::OpusDecoder;

pub const MONO: &[u8] = include_bytes!("../data/mono_440.opus");
pub const STEREO: &[u8] = include_bytes!("../data/stereo_440_1000.opus");
pub const HYBRID: &[u8] = include_bytes!("../data/mono_hybrid.opus");
pub const LOW_BITRATE: &[u8] = include_bytes!("../data/stereo_low_bitrate.opus");
pub const GAIN: &[u8] = include_bytes!("../data/mono_gain.opus");
pub const LONG: &[u8] = include_bytes!("../data/stereo_long.opus");

pub fn decoder<S: Sample>() -> Box<OpusDecoder<S>> {
    let mut slot = Box::new(MaybeUninit::<OpusDecoder<S>>::uninit());
    OpusDecoder::init(&mut slot);

    unsafe { Box::from_raw(Box::into_raw(slot).cast()) }
}

pub fn decode_with<S: Sample>(
    decoder: &mut OpusDecoder<S>,
    file: &[u8],
    chunk: usize,
    output: usize,
) -> Result<Vec<S>, Error> {
    let mut output = vec![S::SILENCE; output.max(2)];
    let mut pending = Vec::new();
    let mut offset = 0;
    let mut decoded = Vec::new();

    loop {
        let progress = decoder.decode(&pending, &mut output)?;

        pending.drain(..progress.consumed);
        decoded.extend_from_slice(&output[..progress.produced]);

        if progress.is_stalled() {
            if offset == file.len() {
                return Ok(decoded);
            }

            let end = (offset + chunk).min(file.len());
            pending.extend_from_slice(&file[offset..end]);
            offset = end;

            if offset == file.len() {
                decoder.finish();
            }
        }
    }
}

pub fn decode_streaming<S: Sample>(
    file: &[u8],
    chunk: usize,
    output: usize,
) -> Result<Vec<S>, Error> {
    decode_with(&mut decoder::<S>(), file, chunk, output)
}

pub fn decode<S: Sample>(file: &[u8]) -> Vec<S> {
    decode_streaming::<S>(file, file.len(), 4 * 5_760).unwrap()
}

pub fn crc(bytes: &[u8]) -> u32 {
    let mut value = 0u32;

    for &byte in bytes {
        value ^= (byte as u32) << 24;

        for _ in 0..8 {
            value = if value & 0x8000_0000 != 0 {
                (value << 1) ^ 0x04C1_1DB7
            } else {
                value << 1
            };
        }
    }

    value
}

pub fn pages(file: &[u8]) -> Vec<(usize, usize)> {
    let mut pages = Vec::new();
    let mut position = 0;

    while position < file.len() {
        let count = file[position + 26] as usize;
        let body: usize = file[position + 27..position + 27 + count]
            .iter()
            .map(|&lacing| lacing as usize)
            .sum();
        let size = 27 + count + body;

        pages.push((position, size));
        position += size;
    }

    pages
}

pub fn packets(file: &[u8]) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut current = Vec::new();

    for (position, _) in pages(file) {
        let count = file[position + 26] as usize;
        let mut body = position + 27 + count;

        for &lacing in &file[position + 27..position + 27 + count] {
            current.extend_from_slice(&file[body..body + lacing as usize]);
            body += lacing as usize;

            if lacing < 255 {
                packets.push(std::mem::take(&mut current));
            }
        }
    }

    packets
}

pub fn granule_position(file: &[u8], page: (usize, usize)) -> u64 {
    u64::from_le_bytes(file[page.0 + 6..page.0 + 14].try_into().unwrap())
}

pub fn final_granule_position(file: &[u8]) -> u64 {
    granule_position(file, *pages(file).last().unwrap())
}

pub fn pre_skip(file: &[u8]) -> u64 {
    let head = &packets(file)[0];

    u16::from_le_bytes([head[10], head[11]]) as u64
}

pub fn page(
    flags: u8,
    granule_position: u64,
    sequence: u32,
    lacing: &[u8],
    body: &[u8],
) -> Vec<u8> {
    let mut page = Vec::new();
    page.extend_from_slice(b"OggS");
    page.push(0);
    page.push(flags);
    page.extend_from_slice(&granule_position.to_le_bytes());
    page.extend_from_slice(&0x1234_5678u32.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]);
    page.push(lacing.len() as u8);
    page.extend_from_slice(lacing);
    page.extend_from_slice(body);

    let checksum = crc(&page);
    page[22..26].copy_from_slice(&checksum.to_le_bytes());
    page
}

pub fn packet_frames(packet: &[u8]) -> u64 {
    let Some(&toc) = packet.first() else {
        return 0;
    };
    let configuration = toc >> 3;
    let frame = match configuration {
        0..=11 => [480, 960, 1_920, 2_880][(configuration & 3) as usize],
        12..=15 => [480, 960][(configuration & 1) as usize],
        _ => [120, 240, 480, 960][(configuration & 3) as usize],
    };

    frame
        * match toc & 3 {
            0 => 1,
            1 | 2 => 2,
            _ => packet.get(1).map_or(0, |count| (count & 0x3F) as u64),
        }
}

pub fn paginate(
    packets: &[Vec<u8>],
    segments_per_page: usize,
    final_granule_position: u64,
) -> Vec<u8> {
    let mut segments = Vec::new();
    let mut granule = 0;
    let mut ends = Vec::new();

    for (index, packet) in packets.iter().enumerate() {
        if index >= 2 {
            granule += packet_frames(packet);
        }

        ends.push(granule);

        let mut offset = 0;

        loop {
            let length = (packet.len() - offset).min(255);
            segments.push((index, &packet[offset..offset + length]));
            offset += length;

            if length < 255 {
                break;
            }
        }
    }

    let mut pages: Vec<Vec<(usize, &[u8])>> = vec![segments[..1].to_vec()];
    let mut rest = &segments[1..];

    while !rest.is_empty() {
        let count = if pages.len() == 1 {
            rest.iter()
                .position(|(index, _)| *index == 2)
                .unwrap_or(rest.len())
        } else {
            segments_per_page.min(rest.len())
        };

        pages.push(rest[..count].to_vec());
        rest = &rest[count..];
    }

    let mut file = Vec::new();
    let mut continued = false;

    for (sequence, segments) in pages.iter().enumerate() {
        let lacing: Vec<u8> = segments
            .iter()
            .map(|(_, bytes)| bytes.len() as u8)
            .collect();
        let body: Vec<u8> = segments
            .iter()
            .flat_map(|(_, bytes)| bytes.iter().copied())
            .collect();
        let last = sequence == pages.len() - 1;
        let mut flags = if continued { 1 } else { 0 };

        if sequence == 0 {
            flags |= 2;
        }

        if last {
            flags |= 4;
        }

        let complete = segments
            .iter()
            .filter(|(_, bytes)| bytes.len() < 255)
            .map(|(index, _)| *index)
            .next_back();
        let granule_position = match (last, complete) {
            (true, _) => final_granule_position,
            (false, Some(index)) => ends[index],
            (false, None) => u64::MAX,
        };

        file.extend(page(
            flags,
            granule_position,
            sequence as u32,
            &lacing,
            &body,
        ));
        continued = *lacing.last().unwrap() == 255;
    }

    file
}
