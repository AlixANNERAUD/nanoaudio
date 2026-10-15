#![no_std]
#![doc = include_str!("../README.md")]

use nanoaudio_core::codec::SeekRequest;
use nanoaudio_core::{Error, Result};

mod ima_adpcm;
mod ms_adpcm;

pub use ima_adpcm::*;
pub use ms_adpcm::*;

fn discard<S: Copy>(output: &mut [S], produced: usize, skip: &mut usize) -> usize {
    let discarded = (*skip).min(produced);

    output.copy_within(discarded..produced, 0);
    *skip -= discarded;

    produced - discarded
}

fn seek_block(
    frame: u64,
    channels: u8,
    block_align: u16,
    samples_per_block: usize,
) -> Result<(SeekRequest, usize)> {
    let samples_per_block = samples_per_block as u64;
    let skip_frames = (frame % samples_per_block) as u32;
    let byte_offset = (frame / samples_per_block)
        .checked_mul(block_align as u64)
        .ok_or(Error::InvalidParameter)?;

    Ok((
        SeekRequest {
            byte_offset,
            skip_frames,
        },
        skip_frames as usize * channels as usize,
    ))
}
