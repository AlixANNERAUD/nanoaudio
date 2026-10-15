# nanoaudio-cpal

Plays and records audio through [cpal](https://crates.io/crates/cpal), with a nanoaudio ring buffer between the application and the audio callback: ALSA on Linux, CoreAudio on macOS, WASAPI on Windows, and Web Audio in the browser with the `web` feature. Unlike the other nanoaudio crates, it needs `std`.

The application writes PCM to a `Producer` at its own pace, and the callback reads the `Consumer`, converts it to the device sample format and channel count, and plays silence when the buffer runs dry, counting each underrun. Capture is the mirror image. The callback does not allocate, lock or block. The device must run at the requested sample rate: resampling comes later.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-cpal = "0.1"
nanoaudio-buffer = "0.1"
nanoaudio-core = "0.1"
```

```rust,no_run
use nanoaudio_buffer::RingBuffer;
use nanoaudio_core::{Configuration, Format};
use nanoaudio_cpal::Output;

let buffer = Box::leak(Box::new(RingBuffer::<16_384>::new()));
let (mut producer, consumer) = buffer.split();
let output = Output::open(consumer, Configuration::new(48_000, 2, Format::I16))?;

producer.write(&[0; 4096]);
output.play()?;
# Ok::<(), nanoaudio_cpal::Error>(())
```

`Output::preferred_configuration` and `Input::preferred_configuration` give the device's own configuration, which needs no conversion. To pick another device, or another cpal host such as JACK or PulseAudio, use cpal (re-exported as `nanoaudio_cpal::cpal`, with its features enabled in your own `Cargo.toml`) and `open_device`.

On Linux, building needs the ALSA development files (`libasound2-dev` on Debian and Ubuntu, `alsa-lib-devel` on Fedora).

## Examples

```sh
cargo run --release -p nanoaudio-cpal --example play_tone
cargo run --release -p nanoaudio-cpal --example play_wav -- path/to/file.wav
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
