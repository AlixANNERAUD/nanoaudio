# nanoaudio-analysis

Analyzers that measure audio as it flows through a pipeline, or over a slice of samples, in fixed or floating point. It is `no_std` and does not allocate.

An analyzer implements `Analyzer` and is plugged into a pipeline with `tap`, which passes the audio on unchanged. Each analyzer keeps fixed-size state, and its results are read through its own methods, at any rate: the audio code can run once per buffer while a display reads once per refresh.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-analysis = "0.1"
nanoaudio-dsp = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_analysis::{FrameCounter, analyze};
use nanoaudio_core::Format;
use nanoaudio_dsp::{Stream, Volume, source};

let music = [0u8; 4096];
let mut output = [0u8; 4096];
let mut counter = FrameCounter::new();

source(&music, Format::I16, 2)
    .unwrap()
    .tap(&mut counter)
    .volume(Volume::from_percent(80))
    .write_bytes(&mut output, Format::I16);

assert_eq!(counter.frames(), 1024);

analyze(&[0i16; 64], 2, &mut counter).unwrap();

assert_eq!(counter.frames(), 1056);
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
