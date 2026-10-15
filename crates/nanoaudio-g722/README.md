# nanoaudio-g722

A G.722 wideband speech encoder and decoder at 64, 56 and 48 kbit/s. It is `no_std` and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-g722 = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::{Result, codec::Decoder};
use nanoaudio_g722::{G722BitRate, G722Decoder};

fn decode(input: &[u8], output: &mut [i16]) -> Result<usize> {
    let mut decoder = G722Decoder::new(G722BitRate::Rate64000);
    let progress = decoder.decode(input, output)?;
    Ok(progress.produced)
}
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
