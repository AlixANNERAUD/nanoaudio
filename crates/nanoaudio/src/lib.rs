#![no_std]
#![warn(missing_docs)]
#![cfg_attr(
    all(feature = "buffer", feature = "dsp", feature = "wav"),
    doc = include_str!("../README.md")
)]
#![cfg_attr(
    not(all(feature = "buffer", feature = "dsp", feature = "wav")),
    doc = "Allocation-free, `no_std` audio for embedded Rust."
)]

pub use nanoaudio_core::*;

#[cfg(feature = "aiff")]
pub use nanoaudio_aiff::{AiffEncoding, AiffHeader};
#[cfg(feature = "analysis")]
pub use nanoaudio_analysis as analysis;
#[cfg(feature = "buffer")]
pub use nanoaudio_buffer::*;
#[cfg(feature = "dsp")]
pub use nanoaudio_dsp::*;
#[cfg(feature = "metadata")]
pub use nanoaudio_metadata as metadata;
#[cfg(feature = "ogg")]
pub use nanoaudio_ogg as ogg;
#[cfg(feature = "synth")]
pub use nanoaudio_synth as synth;
#[cfg(feature = "wav")]
pub use nanoaudio_wav::{Encoding, WavHeader};

/// The [`Decoder`](codec::Decoder), [`Encoder`](codec::Encoder) and [`Seek`](codec::Seek) traits, and the codecs enabled by features.
pub mod codec {
    pub use nanoaudio_core::codec::*;

    #[cfg(feature = "adpcm")]
    pub use nanoaudio_adpcm::*;
    #[cfg(feature = "aiff")]
    pub use nanoaudio_aiff::AiffDecoder;
    #[cfg(feature = "flac")]
    pub use nanoaudio_flac::*;
    #[cfg(feature = "g711")]
    pub use nanoaudio_g711::*;
    #[cfg(feature = "g722")]
    pub use nanoaudio_g722::*;
    #[cfg(feature = "mp3")]
    pub use nanoaudio_mp3::*;
    #[cfg(feature = "opus")]
    pub use nanoaudio_opus::*;
    #[cfg(feature = "qoa")]
    pub use nanoaudio_qoa::*;
    #[cfg(feature = "sbc")]
    pub use nanoaudio_sbc::*;
    #[cfg(feature = "wav")]
    pub use nanoaudio_wav::WavDecoder;
}
