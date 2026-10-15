# nanoaudio

Allocation-free, `no_std` building blocks for PCM audio on microcontrollers and beyond.

`nanoaudio` does not own a pipeline or talk to hardware. It provides small, composable pieces that an audio driver, a mixer task or an operating system can assemble with the memory it already has.

## Features

- Chainable, single-pass processing pipeline (`Stream`)
- Sample formats (`U8`, `I16`, `I32`, `F32`) and stream configuration (`#[repr(C)]`, ready to cross an ABI)
- Sample conversion between formats and channel layouts, on typed slices or raw little-endian bytes
- Volume (Q4.12 control value), applied in place
- Saturating mixing, and a fixed-size mixer with per-source and master volumes
- Single-producer/single-consumer ring buffer with contiguous grants, for DMA and audio callbacks
- WAV header parser that works on a partial buffer and reports when it needs more bytes
- Codecs (`codec` module), behind allocation-free `Decoder` and `Encoder` traits: G.711 A-law and µ-law, IMA ADPCM (WAV block layout), and `WavDecoder` for PCM, A-law, µ-law and IMA ADPCM WAV files, writing any sample type
- MP3 decoding (`mp3` feature) with `Mp3Decoder`, built on [`nanomp3-core`](https://crates.io/crates/nanomp3-core)

## Codecs

Decoders implement `codec::Decoder`: call `decode(input, output)` with whatever input is available and an output of at least `minimum_output()` samples. It returns how many input bytes were consumed and how many samples were produced, so it fits a ring buffer or a file read loop. Call `finish()` once the end of the input has been reached, so that decoders that wait for more input (MP3) drain what is left.

| Codec | Type | Feature | Memory | Notes |
|---|---|---|---|---|
| PCM WAV | `WavDecoder` | | a few bytes | Any `Format`, converted to the output sample type |
| G.711 A-law / µ-law | `G711`, `WavDecoder` | | a few bytes | Encoder and decoder, bit-exact with the reference implementation |
| IMA ADPCM | `ImaAdpcm`, `WavDecoder` | | 40 bytes | Encoder and decoder, WAV block layout, whole blocks only |
| MP3 | `Mp3Decoder` | `mp3` | ~22 KiB, plus input (16 KiB by default) and one frame of output | Decodes in floating point; `mp3-simd` enables SIMD, `mpeg-layer12` adds MP1/MP2 |

The output of a decoder can be fed to `samples(...)` to continue in a pipeline.

## Number representation

Volume and mixing are computed with a `Representation`:

| Representation | Values | Gain | Best for |
|---|---|---|---|
| `Fixed` | `i32` (Q31), saturating | `i32` (Q4.12) | Cores without a floating-point unit |
| `Float` | `f32`, clipped on output | `f32` | Cores with a single-precision floating-point unit |

Every API is generic and monomorphized, so picking a representation costs nothing per sample:

- At build time, `DefaultRepresentation` (used by `mix`, `Volume::apply_to`, `Mixer<N>`, …) is chosen from the target: `Float` on x86, AArch64, WebAssembly, `eabihf` ARM, RISC-V with the `F` extension, ESP32 and ESP32-S3; `Fixed` otherwise (Cortex-M0/M3, ESP32-C3/C6, ESP32-S2, …). The `fixed` and `float` features override this choice.
- Explicitly, with the `_using::<R>` functions or `Mixer<N, Fixed>` / `Mixer<N, Float>`.
- At runtime, with `RepresentationKind` and the `_with` functions, which dispatch once per buffer, not per sample. Both implementations are then linked in.

## Design rules

- `no_std`, no `alloc`, no `unsafe` outside of the ring buffer
- Caller-provided storage, no allocation on the hot path
- Synchronous, non-blocking APIs: operations process what fits and report how much they did
- Codecs are push-based: feed any amount of input, get back how much was consumed and produced

## Pipelines

Operations can be chained into a `Stream`. Each stage pulls one frame at a time from the previous one, so the whole chain runs in a single pass, without intermediate buffers, and samples are converted to the processing representation only once on the way in and once on the way out:

```rust
use nanoaudio::{Format, Stream, Volume, source};

let music = [0u8; 1024];
let notification = [0u8; 512];
let mut output = [0u8; 2048];

let frames = source(&music, Format::I16, 2)?
    .volume(Volume::from_percent(80))
    .mix(source(&notification, Format::U8, 1)?.to_channels(2)?)
    .write_bytes(&mut output, Format::I16);
```

- Sources: `source` / `ByteSource` (raw little-endian bytes in any `Format`) and `samples` / `SampleSource` (typed slices). `remaining()` returns what has not been consumed yet, for example to release it from a ring buffer.
- Stages: `volume`, `to_channels` (duplicates mono, averages down to mono, otherwise keeps or pads channels), `mix` (adds another stream; it goes on until both have ended, and channels missing from one side are left as is) and `map`.
- Sinks: `write_bytes` / `write_samples` overwrite the output, `mix_into_bytes` / `mix_into_samples` add to it. They stop when the output is full or the stream has ended, and return the number of frames processed.
- Frames hold up to `MAXIMUM_CHANNELS` (8) channels.

The single-shot functions (`convert_bytes`, `convert_channels`, `Volume::apply_to`, `mix`, …) remain available for simple cases.

## Example

```rust
use nanoaudio::{Format, Volume, convert_bytes};

let input = [0x00u8, 0x80, 0xFF];
let mut output = [0u8; 6];

let samples = convert_bytes(&input, Format::U8, &mut output, Format::I16);
Volume::from_percent(50).apply_to_bytes(&mut output[..samples * 2], Format::I16);
```

## License

MIT
