# nanoaudio-adpcm

IMA ADPCM and Microsoft ADPCM encoders and decoders, with the WAV block layout. It is `no_std` and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-adpcm = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::{Result, codec::Decoder};
use nanoaudio_adpcm::ImaAdpcm;

fn decode(input: &[u8], output: &mut [i16]) -> Result<usize> {
    let mut decoder = ImaAdpcm::new(8_000, 1, 256)?;
    let progress = decoder.decode(input, output)?;
    Ok(progress.produced)
}
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
