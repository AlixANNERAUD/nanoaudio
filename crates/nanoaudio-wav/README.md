# nanoaudio-wav

A WAV header parser that works on partial buffers, and a decoder for PCM, G.711, IMA ADPCM and Microsoft ADPCM files. It is `no_std` and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-wav = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::{Result, codec::Decoder};
use nanoaudio_wav::{WavDecoder, WavHeader};

fn decode(file: &[u8], output: &mut [i16]) -> Result<usize> {
    let header = WavHeader::parse(file)?;
    let mut decoder = WavDecoder::<i16>::new(&header)?;
    let progress = decoder.decode(&file[header.data_offset..], output)?;
    Ok(progress.produced)
}
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
