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

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
