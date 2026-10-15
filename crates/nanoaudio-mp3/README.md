# nanoaudio-mp3

An MP3 decoder built on [`nanomp3-core`](https://crates.io/crates/nanomp3-core). It is `no_std` and does not allocate. The `simd` feature, on by default, enables explicit SIMD on x86, AArch64 and WebAssembly with `simd128`, and costs nothing elsewhere. `layer12` adds MPEG Layer I and II.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-mp3 = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::{Result, codec::Decoder};
use nanoaudio_mp3::Mp3Decoder;

fn decode(input: &[u8], output: &mut [i16]) -> Result<usize> {
    let mut decoder = Mp3Decoder::<i16>::new();
    let progress = decoder.decode(input, output)?;
    Ok(progress.produced)
}
```

## Seeking

`Mp3Decoder` implements `Seek` once it has found the first frame. Byte offsets are counted from the first byte fed to the decoder, so an ID3v2 tag the caller fed counts. Frame positions count the frames the decoder produces, so they follow gapless trimming.

- Constant bitrate streams seek exactly: the offset comes from the frame size, and the frame reached is identified from its byte position.
- Variable bitrate streams use the Xing table of contents or the VBRI table from the first frame. MP3 frames carry no number, so the position is approximate: within about 1 % of the duration.
- A stream with neither a table nor a constant bitrate falls back to its average bitrate so far.

In both cases the decoder resumes a few frames early to refill the bit reservoir and the synthesis filters, then drops them, so the output matches a sequential decode from the frame reached. Seeking past the end returns `Error::InvalidParameter` when a tag gives the frame count. Otherwise, the offset returned is past the end and nothing is produced.

## Gapless playback

MP3 encoders add silence at the start and the end of the audio, and the decoder adds a delay of its own. When the first frame holds a LAME or FFmpeg tag, `Mp3Decoder` drops the tag frame, the encoder delay plus the 529-sample decoder delay at the start, and the encoder padding at the end, so the output is the audio that was encoded, sample for sample. Seek positions and `frames()` then count the same samples. After a seek in a variable bitrate stream, the end is trimmed at the estimated position, so it can move by as much as the seek error. Trimming costs nothing measurable and is on by default; `set_gapless(false)` turns it off.

Streams without that tag are not trimmed: frame positions then include the Xing, Info or VBRI frame, which decodes as silence, and the encoder delay.

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
