use crate::{Configuration, Result, Sample};

/// How much a [`Decoder`] or [`Encoder`] call consumed and produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Progress {
    /// Input consumed: bytes for a decoder, samples for an encoder.
    pub consumed: usize,
    /// Output written: samples for a decoder, bytes for an encoder.
    pub produced: usize,
}

impl Progress {
    /// Creates a progress report.
    pub const fn new(consumed: usize, produced: usize) -> Self {
        Self { consumed, produced }
    }

    /// Returns `true` if nothing was consumed or produced.
    ///
    /// Before [`Decoder::finish`] or [`Encoder::flush`], it means the call needs more input. After it, the stream has ended.
    pub const fn is_stalled(&self) -> bool {
        self.consumed == 0 && self.produced == 0
    }
}

/// A push-style decoder from encoded bytes to interleaved samples.
///
/// Each call to [`decode`](Self::decode) consumes what it can from the input, keeps any partial state, and writes what it can to the output. When the [`Progress`] is stalled, it needs more input. Once the input has ended, call [`finish`](Self::finish) and keep decoding until it stalls again.
///
/// ```
/// use nanoaudio_core::Result;
/// use nanoaudio_core::codec::Decoder;
///
/// fn decode_all<D: Decoder>(
///     decoder: &mut D,
///     mut input: &[u8],
///     output: &mut [D::Sample],
///     mut play: impl FnMut(&[D::Sample]),
/// ) -> Result<()> {
///     let mut finished = false;
///
///     loop {
///         let progress = decoder.decode(input, output)?;
///         input = &input[progress.consumed..];
///         play(&output[..progress.produced]);
///
///         if progress.is_stalled() {
///             if finished {
///                 return Ok(());
///             }
///
///             decoder.finish();
///             finished = true;
///         }
///     }
/// }
/// ```
pub trait Decoder {
    /// The sample type written to the output.
    type Sample: Sample;

    /// Returns the configuration of the decoded stream, or `None` until the decoder has read enough of it.
    fn configuration(&self) -> Option<Configuration>;

    /// Returns the smallest output, in samples, that [`decode`](Self::decode) accepts.
    fn minimum_output(&self) -> usize;

    /// Returns the number of input bytes the decoder may need before it makes progress, to size the input buffer.
    fn minimum_input(&self) -> usize {
        1
    }

    /// Tells the decoder that the input has ended, so it decodes what it still holds instead of waiting for more.
    fn finish(&mut self) {}

    /// Decodes from `input` into `output`, and reports the bytes consumed and the samples produced.
    ///
    /// # Errors
    ///
    /// [`Error::BufferTooSmall`](crate::Error::BufferTooSmall) if `output` is shorter than [`minimum_output`](Self::minimum_output), and [`Error::SeekTo`](crate::Error::SeekTo) when a seek needs input from another offset.
    fn decode(&mut self, input: &[u8], output: &mut [Self::Sample]) -> Result<Progress>;
}

/// A push-style encoder from interleaved samples to encoded bytes.
///
/// It follows the same contract as [`Decoder`]: [`encode`](Self::encode) consumes what it can, and [`flush`](Self::flush) tells it the input has ended so it writes out what it still holds.
pub trait Encoder {
    /// The sample type read from the input.
    type Sample: Sample;

    /// Returns the configuration of the encoded stream.
    fn configuration(&self) -> Configuration;

    /// Returns the number of input samples the encoder may need before it makes progress.
    fn minimum_input(&self) -> usize;

    /// Returns the smallest output, in bytes, that [`encode`](Self::encode) accepts.
    fn minimum_output(&self) -> usize;

    /// Tells the encoder that the input has ended, so it encodes what it still holds instead of waiting for a full block.
    fn flush(&mut self) {}

    /// Encodes from `input` into `output`, and reports the samples consumed and the bytes produced.
    ///
    /// # Errors
    ///
    /// [`Error::BufferTooSmall`](crate::Error::BufferTooSmall) if `output` is shorter than [`minimum_output`](Self::minimum_output).
    fn encode(&mut self, input: &[Self::Sample], output: &mut [u8]) -> Result<Progress>;
}

/// Where to resume feeding input after a seek.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SeekRequest {
    /// Byte offset to resume from, counted from the first byte the decoder was ever fed: the start of the sound data for WAV and AIFF, the start of the file for QOA.
    pub byte_offset: u64,
    /// Frames the decoder drops by itself after resuming, so the next frame produced is the requested one; reported for information only.
    pub skip_frames: u32,
}

/// A decoder that can jump to an arbitrary frame.
///
/// After [`seek`](Self::seek), drop the buffered input and feed the decoder from [`SeekRequest::byte_offset`]. If `decode` then returns [`Error::SeekTo`](crate::Error::SeekTo), do the same with the offset it carries, until decoding resumes.
///
/// ```
/// use nanoaudio_core::codec::{Decoder, Seek};
/// use nanoaudio_core::{Error, Result};
///
/// fn decode_at<D: Decoder + Seek>(
///     decoder: &mut D,
///     stream: &[u8],
///     frame: u64,
///     output: &mut [D::Sample],
/// ) -> Result<usize> {
///     let mut offset = decoder.seek(frame)?.byte_offset;
///
///     loop {
///         let input = stream.get(offset as usize..).unwrap_or_default();
///
///         match decoder.decode(input, output) {
///             Err(Error::SeekTo(next)) => offset = next,
///             result => return result.map(|progress| progress.produced),
///         }
///     }
/// }
/// ```
pub trait Seek {
    /// Prepares the decoder to produce `frame`, counted from the start of the audio, and returns where the caller must resume feeding input.
    ///
    /// Decoders that cannot compute the position directly may then return [`Error::SeekTo`](crate::Error::SeekTo) from `decode` until they reach it.
    fn seek(&mut self, frame: u64) -> Result<SeekRequest>;
}
