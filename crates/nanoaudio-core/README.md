# nanoaudio-core

The shared building blocks of nanoaudio: sample formats, stream configuration, errors, the `Decoder` and `Encoder` traits every codec implements, and the bit reader and CRC helpers the codecs share. It is `no_std`, has no dependency and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::{Configuration, Format, Sample};

let configuration = Configuration::new(48_000, 2, Format::I16);

assert_eq!(configuration.format, i16::FORMAT);
assert_eq!(Format::I16.sample_size(), 2);
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
