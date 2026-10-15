# nanoaudio-metadata

Reads ID3v1, ID3v2, FLAC, RIFF and AIFF tags and MP3 Xing, LAME and VBRI headers from whatever part of a file you have in memory. It is `no_std`, does not allocate and has no dependency; the `core` feature adds a conversion to `nanoaudio_core::Error`.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-metadata = "0.1"
```

```rust
use nanoaudio_metadata::{Container, detect};

let flac = matches!(detect(b"OggS", 0), Ok(Container::Flac(_)));

assert!(!flac);
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
