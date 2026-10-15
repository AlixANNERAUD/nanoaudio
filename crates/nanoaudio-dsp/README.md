# nanoaudio-dsp

PCM format conversion, volume, mixing, sample rate conversion and single-pass pipelines, in fixed or floating point. It is `no_std` and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-dsp = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::Format;
use nanoaudio_dsp::{Stream, Volume, source};

let music = [0u8; 4096];
let mut output = [0u8; 4096];

let frames = source(&music, Format::I16, 2)
    .unwrap()
    .volume(Volume::from_percent(80))
    .write_bytes(&mut output, Format::I16);

assert_eq!(frames, 1024);
```

A `Resampler` converts the sample rate, with linear interpolation or a windowed-sinc filter, and keeps its place between buffers:

```rust
use nanoaudio_core::Format;
use nanoaudio_dsp::{Resampler, Sinc, Stream, source};

let mut resampler = Resampler::<Sinc>::new(2, 44_100, 48_000).unwrap();
let music = [0u8; 4 * 441];
let mut output = [0f32; 2 * 480];

let frames = source(&music, Format::I16, 2)
    .unwrap()
    .resample(&mut resampler)
    .unwrap()
    .write_samples(&mut output);

assert_eq!(frames, 480);
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
