# nanoaudio-sbc

A Bluetooth SBC and mSBC encoder and decoder. It is `no_std` and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-sbc = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::{Result, codec::Decoder};
use nanoaudio_sbc::SbcDecoder;

fn decode(input: &[u8], output: &mut [i16]) -> Result<usize> {
    let mut decoder = SbcDecoder::new();
    let progress = decoder.decode(input, output)?;
    Ok(progress.produced)
}
```

## Seeking

`SbcDecoder` implements `Seek` once it has decoded a frame. Byte offsets are counted from the start of the stream, assuming every frame uses the parameters of the last decoded one. The decoder resumes up to nine blocks early to refill its synthesis filter and drops them, so the output matches a sequential decode exactly. The stream length is unknown: seeking past the end returns an offset past the end, and nothing is produced.

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
