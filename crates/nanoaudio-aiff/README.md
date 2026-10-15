# nanoaudio-aiff

An AIFF and AIFF-C header parser that works on partial buffers, and a decoder for PCM, float, A-law and µ-law files. It is `no_std` and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-aiff = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_aiff::{AiffDecoder, AiffHeader};
use nanoaudio_core::{Result, codec::Decoder};

fn decode(file: &[u8], output: &mut [i16]) -> Result<usize> {
    let header = AiffHeader::parse(file)?;
    let mut decoder = AiffDecoder::<i16>::new(&header)?;
    let progress = decoder.decode(&file[header.data_offset..], output)?;
    Ok(progress.produced)
}
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
