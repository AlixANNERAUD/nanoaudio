# nanoaudio-web

Low-latency audio playback and capture in the browser, from Rust compiled to `wasm32-unknown-unknown`, through a Web Audio `AudioWorklet`. It runs on stable Rust and needs no special page headers: no `SharedArrayBuffer`, no wasm threads, no COOP/COEP.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-web = "0.1"
```

## How it Works

The worklet loads its own instance of your WebAssembly module, on the browser's audio thread. Before each render quantum of 128 frames, it calls your `Source`, which writes interleaved PCM into a `nanoaudio-buffer` [`Producer`]; the driver reads the [`Consumer`], converts to the planar `f32` Web Audio expects, plays silence for what is missing and counts underruns. Capture works the same way in reverse, with a `Sink` reading a [`Consumer`].

The two instances share no memory, so the decoder and the pipeline run inside the worklet: the page sends them compressed data or commands with `Output::post`, and they answer with `Port::post`. Playback does not depend on the page's main thread, so a busy page does not cause underruns, and the only buffering is what the source keeps ahead in the ring buffer.

The audio context is opened at the stream's sample rate. If the browser picks another rate, `Output::start` returns `Error::SampleRate`.

## Example

```rust,ignore
use nanoaudio_web::{CAPACITY, Configuration, Format, Output, Port, Producer, Source, Worklet};
use wasm_bindgen::prelude::*;

const STREAM: Configuration = Configuration::new(48_000, 1, Format::I16);

struct Noise(u32);

impl Source for Noise {
    fn fill(&mut self, producer: &mut Producer<'_, CAPACITY>, _: &Port) {
        while CAPACITY - producer.free() < STREAM.frames_to_bytes(256) {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            producer.write(&((self.0 >> 16) as i16 / 8).to_le_bytes());
        }
    }
}

#[wasm_bindgen]
pub fn noise() -> Worklet {
    Worklet::output(Noise(1))
}

#[wasm_bindgen]
pub async fn play() -> Result<(), JsValue> {
    let output = Output::start(STREAM, "noise").await?;
    std::mem::forget(output);
    Ok(())
}
```

`noise` runs in the worklet and `play` on the page, from a click handler since browsers start audio only after a user gesture. Build with `wasm-bindgen --target web`: the worklet imports the generated JavaScript module.

The conversion is available on every target and does not need a browser:

```rust
use nanoaudio_buffer::RingBuffer;
use nanoaudio_web::{Configuration, Format, Renderer};

let mut buffer = RingBuffer::<64>::new();
let (mut producer, mut consumer) = buffer.split();
let mut renderer = Renderer::new(Configuration::new(48_000, 2, Format::I16))?;

producer.write(&[0, 64, 0, 192]);

let mut quantum = [0.0; 4];
assert_eq!(renderer.render(&mut consumer, &mut quantum), 1);
assert_eq!(quantum, [0.5, 0.0, -0.5, 0.0]);
assert_eq!(renderer.underruns(), 1);
# Ok::<(), nanoaudio_core::Error>(())
```

## Demo Page

[`examples/player.rs`](https://github.com/AlixANNERAUD/nanoaudio/blob/main/crates/nanoaudio-web/examples/player.rs) plays a sine, plays a WAV file decoded in the worklet, and shows the microphone level. From the repository root, with [`wasm-bindgen-cli`](https://crates.io/crates/wasm-bindgen-cli) at the `wasm-bindgen` version in `Cargo.lock`:

```sh
cargo install wasm-bindgen-cli --version 0.2.129 --locked
cargo build -p nanoaudio-web --example player --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir crates/nanoaudio-web/examples/www/pkg target/wasm32-unknown-unknown/release/examples/player.wasm
python3 -m http.server 8000 --directory crates/nanoaudio-web/examples/www
```

Then open <http://localhost:8000>. `localhost` counts as a secure context, which the microphone needs.

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
