# nanoaudio-flac

A FLAC decoder for 8 to 32-bit streams. It is `no_std` and does not allocate: the block buffer size and channel count are const generics.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-flac = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::{Result, codec::Decoder};
use nanoaudio_flac::FlacDecoder;

fn decode(input: &[u8], output: &mut [i16]) -> Result<usize> {
    let mut decoder = FlacDecoder::<i16>::new();
    let progress = decoder.decode(input, output)?;
    Ok(progress.produced)
}
```

## Seeking

`FlacDecoder` implements `Seek` once it has read the metadata. Byte offsets are counted from `fLaC`. Frame headers carry their sample number, so the decoder resynchronises on the next valid frame wherever it resumes and lands exactly on the requested frame.

The first request uses the SEEKTABLE when there is one (up to `FLAC_SEEK_POINTS` points are kept, a regular subset of larger tables), otherwise an interpolation from the frames already seen. When it lands too far from the target, `decode` returns `Error::SeekTo(offset)`: drop buffered input and resume from `offset`. Each round narrows the search, at worst by half every two rounds, and it usually takes zero to three. `set_stream_length` lets the first request interpolate over the whole stream. Without it, the decoder probes forward until it finds the end, which the caller signals by calling `finish` once it runs out of input.

```rust
use nanoaudio_core::codec::{Decoder, Seek};
use nanoaudio_core::{Error, Result};
use nanoaudio_flac::FlacDecoder;

fn seek(decoder: &mut FlacDecoder, file: &[u8], frame: u64, output: &mut [i16]) -> Result<usize> {
    decoder.set_stream_length(file.len() as u64);
    let mut offset = decoder.seek(frame)?.byte_offset as usize;
    loop {
        decoder.finish();
        match decoder.decode(&file[offset.min(file.len())..], output) {
            Err(Error::SeekTo(next)) => offset = next as usize,
            result => return result.map(|progress| progress.produced),
        }
    }
}
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
