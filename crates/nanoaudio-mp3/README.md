# nanoaudio-mp3

An MP3 decoder built on [`nanomp3-core`](https://crates.io/crates/nanomp3-core). It is `no_std` and does not allocate. The `simd` feature enables SIMD and `layer12` adds MPEG Layer I and II.

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

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
