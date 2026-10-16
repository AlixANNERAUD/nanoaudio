use std::fs;

use nanoaudio_core::Format;
use nanoaudio_dsp::{Stream, Volume};
use nanoaudio_synth::{Adsr, FilterSettings, Patch, Synth, Waveform};

const SAMPLE_RATE: u32 = 44_100;

fn header(data: usize) -> [u8; 44] {
    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data as u32).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes());
    header[22..24].copy_from_slice(&2u16.to_le_bytes());
    header[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    header[28..32].copy_from_slice(&(SAMPLE_RATE * 4).to_le_bytes());
    header[32..34].copy_from_slice(&4u16.to_le_bytes());
    header[34..36].copy_from_slice(&16u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&(data as u32).to_le_bytes());
    header
}

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "chord.wav".into());
    let mut synth: Synth = Synth::new(SAMPLE_RATE);
    let mut patch = Patch::new(
        Waveform::Saw,
        Adsr::from_millis(20, 400, 45_000, 800, SAMPLE_RATE),
    );
    patch.filter = Some(FilterSettings::low_pass(1_800.0, 0.4));

    for note in [48, 60, 64, 67, 71] {
        synth.note_on(0, note, 80, &patch);
    }

    let held = SAMPLE_RATE as usize * 2;
    let total = held + SAMPLE_RATE as usize;
    let mut pcm = vec![0u8; total * 4];
    let (first, second) = pcm.split_at_mut(held * 4);

    let mut pipeline = (&mut synth)
        .volume(Volume::from_percent(25))
        .to_channels(2)
        .unwrap();
    pipeline.write_bytes(first, Format::I16);
    drop(pipeline);

    synth.release_all();
    (&mut synth)
        .volume(Volume::from_percent(25))
        .to_channels(2)
        .unwrap()
        .write_bytes(second, Format::I16);

    let mut file = header(pcm.len()).to_vec();
    file.extend_from_slice(&pcm);
    fs::write(&path, file)?;
    println!("Wrote {total} frames to {path}");

    Ok(())
}
