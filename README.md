# nanoaudio

[![crates.io](https://img.shields.io/crates/v/nanoaudio.svg)](https://crates.io/crates/nanoaudio)
[![docs.rs](https://img.shields.io/docsrs/nanoaudio)](https://docs.rs/nanoaudio)
[![CI](https://github.com/AlixANNERAUD/nanoaudio/actions/workflows/check.yml/badge.svg)](https://github.com/AlixANNERAUD/nanoaudio/actions/workflows/check.yml)
[![codecov](https://codecov.io/github/AlixANNERAUD/nanoaudio/graph/badge.svg)](https://codecov.io/github/AlixANNERAUD/nanoaudio)
[![License: MIT](https://img.shields.io/crates/l/nanoaudio.svg)](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE)
[![MSRV](https://img.shields.io/crates/msrv/nanoaudio.svg)](https://github.com/AlixANNERAUD/nanoaudio/blob/main/Cargo.toml)

**Allocation-free, `no_std` audio for embedded Rust.**
Convert, mix and adjust the volume of PCM audio, chain it all into single-pass pipelines, buffer it for DMA or audio callbacks, and decode WAV, G.711, IMA ADPCM and MP3, on anything from a Cortex-M0 to a desktop.
nanoaudio does not own a runtime or talk to hardware: it gives your driver, mixer task or operating system the pieces, and lets you bring the memory.

## Why nanoaudio?

- **`no_std` and no allocator.** Everything works with `core` only, on caller-provided buffers. Nothing allocates, on the hot path or anywhere else.
- **Single-pass pipelines.** Chain format conversion, channel mapping, volume and mixing: the compiler fuses the chain into one loop, with no intermediate buffers, up to 2.9 times faster than running each step on its own.
- **Integer or floating point, your call.** Processing runs in fixed point on cores without an FPU and in `f32` on cores with one, chosen from the target at build time, overridable with a feature, or selectable at run time.
- **Made for DMA and audio callbacks.** A lock-free single-producer/single-consumer ring buffer hands out contiguous slices, and every operation is synchronous and non-blocking: it processes what fits and reports how much it did.
- **Codecs included.** WAV (PCM, A-law, µ-law, IMA ADPCM), G.711 and IMA ADPCM encoders and decoders, bit-exact with reference implementations, and MP3 decoding behind a feature.
- **Small and dependency-free.** No third-party dependency by default; MP3 support adds [`nanomp3-core`](https://crates.io/crates/nanomp3-core), itself `no_std` and free of `unsafe`.
- **Checked.** Tested on every combination of features, run under Miri, and built for Cortex-M0, Cortex-M4F and RISC-V in CI.

## Installation

```sh
cargo add nanoaudio
```

or in `Cargo.toml`:

```toml
[dependencies]
nanoaudio = "0.1"
```

Every feature except MP3, `analysis-fft` and `synth` is enabled by default. With `default-features = false`, only the types from `nanoaudio-core` remain, and you pick what you need:

| Feature        | Enables                                                                                                   |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| `dsp`          | Conversion, volume, mixing and pipelines (`nanoaudio-dsp`).                                                |
| `analysis`     | The `analysis` module of analyzers for `tap` (`nanoaudio-analysis`). Implies `dsp`.                     |
| `analysis-fft` | `analysis::Spectrum`, an FFT spectrum analyzer with [`microfft`](https://crates.io/crates/microfft). Implies `analysis`. Not default. |
| `synth`        | The `synth` module: oscillators, envelopes, a filter and a polyphonic `Synth` source for pipelines (`nanoaudio-synth`). Implies `dsp`. Not default. |
| `buffer`       | `RingBuffer`, `Producer` and `Consumer` (`nanoaudio-buffer`).                                              |
| `g711`         | `codec::G711` and the A-law and µ-law functions (`nanoaudio-g711`).                                        |
| `adpcm`        | `codec::ImaAdpcm` and `codec::MsAdpcm` (`nanoaudio-adpcm`).                                                |
| `qoa`          | `codec::QoaDecoder` and `codec::QoaEncoder` (`nanoaudio-qoa`).                                             |
| `flac`         | `codec::FlacDecoder` (`nanoaudio-flac`).                                                                   |
| `g722`         | `codec::G722Decoder` and `codec::G722Encoder` (`nanoaudio-g722`).                                          |
| `sbc`          | `codec::SbcDecoder` and `codec::SbcEncoder` (`nanoaudio-sbc`).                                             |
| `wav`          | `WavHeader` and `codec::WavDecoder` (`nanoaudio-wav`). Implies `g711` and `adpcm`.                         |
| `aiff`         | `AiffHeader` and `codec::AiffDecoder` (`nanoaudio-aiff`). Implies `g711`.                                  |
| `metadata`     | The `metadata` module (`nanoaudio-metadata`).                                                              |
| `mp3`          | `codec::Mp3Decoder`, MP3 decoding with [`nanomp3-core`](https://crates.io/crates/nanomp3-core). Not default. |
| `mp3-simd`     | SIMD in the MP3 decoder, already on with `mp3`. Implies `mp3`.                                             |
| `mpeg-layer12` | MPEG Layer I and II (MP1, MP2) decoding. Implies `mp3`.                                                    |
| `fixed`        | Forces fixed-point processing by default. See [Integer or Floating Point](#integer-or-floating-point).    |
| `float`        | Forces floating-point processing by default. If `fixed` is also enabled, `fixed` takes precedence.        |

`nanoaudio` re-exports a set of smaller crates, one per codec or building block, which can also be used on their own: [`nanoaudio-core`](https://crates.io/crates/nanoaudio-core), [`nanoaudio-dsp`](https://crates.io/crates/nanoaudio-dsp), [`nanoaudio-analysis`](https://crates.io/crates/nanoaudio-analysis), [`nanoaudio-synth`](https://crates.io/crates/nanoaudio-synth), [`nanoaudio-buffer`](https://crates.io/crates/nanoaudio-buffer), [`nanoaudio-g711`](https://crates.io/crates/nanoaudio-g711), [`nanoaudio-adpcm`](https://crates.io/crates/nanoaudio-adpcm), [`nanoaudio-qoa`](https://crates.io/crates/nanoaudio-qoa), [`nanoaudio-flac`](https://crates.io/crates/nanoaudio-flac), [`nanoaudio-g722`](https://crates.io/crates/nanoaudio-g722), [`nanoaudio-sbc`](https://crates.io/crates/nanoaudio-sbc), [`nanoaudio-mp3`](https://crates.io/crates/nanoaudio-mp3), [`nanoaudio-wav`](https://crates.io/crates/nanoaudio-wav), [`nanoaudio-aiff`](https://crates.io/crates/nanoaudio-aiff) and [`nanoaudio-metadata`](https://crates.io/crates/nanoaudio-metadata).

## Quick Start

Mix 16-bit stereo music at 80 % volume with an 8-bit mono notification, and write the result as 16-bit stereo PCM, in a single pass:

```rust
use nanoaudio::{Format, Stream, Volume, source};

let music = [0u8; 4096];
let notification = [0x80u8; 512];
let mut output = [0u8; 4096];

let frames = source(&music, Format::I16, 2)
    .unwrap()
    .volume(Volume::from_percent(80))
    .mix(source(&notification, Format::U8, 1).unwrap().to_channels(2).unwrap())
    .write_bytes(&mut output, Format::I16);

assert_eq!(frames, 1024);
```

A pipeline starts from a source, goes through any number of stages and ends in a sink:

- **Sources:** `source` reads raw little-endian bytes in any `Format`, `samples` reads typed slices (`&[i16]`, `&[f32]`, …). `remaining()` returns what has not been consumed yet.
- **Stages:** `volume`, `to_channels` (duplicates mono, averages down to mono, otherwise keeps or pads channels), `mix` (adds another stream), `map` (any function on the processed values) and `tap` (gives each frame to an analyzer, such as a meter, and passes it on unchanged).
- **Sinks:** `write_bytes` and `write_samples` overwrite the output, `mix_into_bytes` and `mix_into_samples` add to it. They stop when the output is full or the stream has ended, and return the number of frames processed.

Each stage pulls one frame at a time from the previous one, so samples are converted to the processing representation once on the way in and once on the way out.

## Decoding Audio Files

Every decoder implements `codec::Decoder`: give it whatever input you have and an output of at least `minimum_output()` samples, and it tells you how many bytes it consumed and how many samples it produced. Call `finish()` at the end of the input, so decoders that wait for more data drain what is left. The same loop decodes WAV and MP3:

```rust
use nanoaudio::codec::{Decoder, WavDecoder};
use nanoaudio::{Error, WavHeader};

fn decode_all<D: Decoder>(decoder: &mut D, mut input: &[u8]) -> Result<Vec<D::Sample>, Error> {
    let mut buffer = vec![D::Sample::default(); decoder.minimum_output().max(1024)];
    let mut decoded = Vec::new();
    let mut finished = false;

    loop {
        let progress = decoder.decode(input, &mut buffer)?;
        input = &input[progress.consumed..];
        decoded.extend_from_slice(&buffer[..progress.produced]);

        if progress.is_stalled() {
            if finished {
                return Ok(decoded);
            }

            decoder.finish();
            finished = true;
        }
    }
}

fn decode_wav(file: &[u8]) -> Result<Vec<i16>, Error> {
    let header = WavHeader::parse(file)?;
    let end = (header.data_offset + header.data_size as usize).min(file.len());

    decode_all(&mut WavDecoder::<i16>::new(&header)?, &file[header.data_offset..end])
}
```

`WavHeader::parse` works on the first bytes of a file and returns `Error::Incomplete` when it needs more. The decoded samples can be fed to `samples(...)` to continue in a pipeline, and `WavDecoder::<f32>` or `Mp3Decoder::<f32>` decode straight to floating point.

| Codec               | Type                  | Feature | Memory                                                    | Notes                                                              |
| ------------------- | --------------------- | ------- | --------------------------------------------------------- | ------------------------------------------------------------------ |
| PCM WAV             | `WavDecoder`          |         | a few bytes                                               | 8, 16, 24 and 32-bit integer and 32-bit float                      |
| G.711 A-law / µ-law | `G711`, `WavDecoder`  |         | a few bytes                                               | Encoder and decoder, bit-exact with the reference implementation  |
| IMA ADPCM           | `ImaAdpcm`, `WavDecoder` |      | 40 bytes                                                  | Encoder and decoder, WAV block layout, bit-exact sample codec     |
| MP3                 | `Mp3Decoder`          | `mp3`   | ~23 KiB, plus 4 KiB of input and one frame of output      | MPEG-1/2/2.5 Layer III, decodes in floating point                  |

## Choosing a Codec

| Codec     | Kind     | Typical bitrate                                     | RAM to decode                             | Flash, Cortex-M4 | Decoding speed | Best for                                |
| --------- | -------- | --------------------------------------------------- | ----------------------------------------- | ---------------- | -------------- | --------------------------------------- |
| G.711     | lossy    | 64 kbit/s (8 kHz mono)                              | a few bytes                               | 1.3 KiB          | fastest        | Telephony, 8 kHz voice                  |
| IMA ADPCM | lossy    | 4 bits per sample (354 kbit/s at 44.1 kHz stereo)   | 1.3 to 10 KiB, by block size              | 3.0 KiB          | very fast      | Sound effects and prompts on small MCUs |
| MS ADPCM  | lossy    | 4 bits per sample                                   | 1.3 to 10 KiB, by block size              | 2.9 KiB          | very fast      | Existing WAV files                      |
| QOA       | lossy    | 3.2 bits per sample (285 kbit/s at 44.1 kHz stereo) | 0.6 KiB                                   | 4.1 KiB          | very fast      | Music and effects on MCUs               |
| G.722     | lossy    | 64 kbit/s (16 kHz mono)                             | 0.3 KiB                                   | 3.5 KiB          | fast           | Wideband voice                          |
| SBC       | lossy    | 60 kbit/s (mSBC) to 330 kbit/s (A2DP)               | 1.9 KiB                                   | 8.4 KiB          | fast           | Bluetooth                               |
| FLAC      | lossless | 50 to 70 % of PCM, by material                      | ~32 KiB for 16-bit stereo, `i16` blocks   | 13.8 KiB         | moderate       | Lossless playback                       |
| MP3       | lossy    | 128 kbit/s                                          | ~32 KiB, less with a smaller input window | 46.7 KiB         | moderate       | Music files                             |

- For the lowest CPU and flash use G.711 at 8 kHz, or IMA ADPCM at higher sample rates.
- On a microcontroller, QOA gives the best quality per byte: about the same signal-to-noise ratio as IMA ADPCM at 80 % of the size, in integer arithmetic and under 1 KiB of RAM.
- For lossless audio use FLAC, if you can spare around 32 KiB of RAM for 16-bit files (`i16` blocks) or 50 KiB for deeper ones; set its maximum block size to the one your files use.
- For Bluetooth use SBC (A2DP, or mSBC for hands-free), and for wideband voice over IP use G.722.
- For music files use MP3, preferably on a core with an FPU, since it decodes in floating point.
- If the device encodes, prefer G.711 or IMA ADPCM: the MS ADPCM and QOA encoders search for the best parameters and take 10 to 30 times longer than their decoders.

Flash is `.text` and `.rodata` for decoding only at `opt-level = "s"`, and RAM includes the decoder and the smallest input and output buffers on a 32-bit target. The full numbers, for more targets and configurations, are in [docs/benchmarks.md](https://github.com/AlixANNERAUD/nanoaudio/blob/main/docs/benchmarks.md).

## Feeding Hardware

`RingBuffer` connects the code producing audio to the code consuming it, such as a DMA interrupt, an I2S driver or a host audio callback. The producer and the consumer can live in different tasks or interrupts, without locks, and both work on contiguous slices:

```rust
use nanoaudio::{Format, RingBuffer, Stream, Volume, samples};

let mut buffer = RingBuffer::<4096>::new();
let (mut producer, mut consumer) = buffer.split();

let tone = [1000i16, -1000, 2000, -2000];
let frames = samples(&tone, 2)
    .unwrap()
    .volume(Volume::from_percent(50))
    .write_bytes(producer.grant(), Format::I16);
producer.commit(frames * 4);

let ready = consumer.readable().len();
assert_eq!(ready, 8);
consumer.release(ready);
```

`Producer::write` and `Consumer::read` copy as much as fits and return how much they did, so a writer never blocks.

On a desktop or in the browser, [`nanoaudio-cpal`](https://crates.io/crates/nanoaudio-cpal) plays and records through [cpal](https://crates.io/crates/cpal), with a ring buffer between the application and the audio callback. It needs `std` and is not part of `nanoaudio`.

## Integer or Floating Point

Volume and mixing are computed with a `Representation`:

| Representation | Values                   | Gain          | Best for                                           |
| -------------- | ------------------------ | ------------- | -------------------------------------------------- |
| `Fixed`        | `i32` (Q31), saturating  | `i32` (Q4.12) | Cores without a floating-point unit                |
| `Float`        | `f32`, clipped on output | `f32`         | Cores with a single-precision floating-point unit  |

Every API is generic and monomorphized, so the choice costs nothing per sample:

- **At build time,** `DefaultRepresentation` (used by `source`, `mix`, `Volume::apply_to`, `Mixer<N>`, …) is chosen from the target: `Float` on x86, AArch64, WebAssembly, `eabihf` ARM, RISC-V with the `F` extension, ESP32 and ESP32-S3, and `Fixed` otherwise (Cortex-M0/M3, ESP32-C3/C6, ESP32-S2, …). The `fixed` and `float` features override it; when both are enabled, for example by two dependencies, `fixed` takes precedence since it runs everywhere.
- **Explicitly,** with `source_using::<Fixed>`, the `_using::<R>` functions, or `Mixer<N, Float>`.
- **At run time,** with `RepresentationKind` and the `_with` functions, which dispatch once per buffer, not per sample.

## Building Blocks

The pieces used by pipelines are also available on their own:

| Item                                              | Purpose                                                                     |
| ------------------------------------------------- | --------------------------------------------------------------------------- |
| `Format`, `Configuration`, `Sample`               | Sample formats (`U8`, `I16`, `I32`, `F32`) and stream configuration, `#[repr(C)]` to cross an ABI |
| `convert`, `convert_bytes`, `convert_channels`    | Format and channel layout conversion, on typed slices or raw bytes          |
| `Volume`                                          | Volume as a Q4.12 control value, applied in place                           |
| `mix`, `mix_bytes`, `Mixer`                       | Saturating mixing, and a fixed-size mixer with per-source and master volumes |
| `RingBuffer`, `Producer`, `Consumer`              | Lock-free single-producer/single-consumer buffer with contiguous grants     |
| `WavHeader`                                       | WAV header parsing on a partial buffer                                      |
| `codec::{G711, ImaAdpcm, WavDecoder, Mp3Decoder}` | Decoders and encoders                                                       |

## Platform Support

nanoaudio only needs `core`, and does not depend on an operating system, an allocator or atomics beyond loads and stores. CI runs the tests and Miri on Linux, and builds for:

| Target                          | Example hardware                  | Default representation |
| ------------------------------- | --------------------------------- | ---------------------- |
| `thumbv6m-none-eabi`            | Cortex-M0, RP2040                 | `Fixed`                |
| `thumbv7em-none-eabihf`         | Cortex-M4F, STM32F4, nRF52840     | `Float`                |
| `riscv32imc-unknown-none-elf`   | ESP32-C3                          | `Fixed`                |
| `riscv32imafc-unknown-none-elf` | RISC-V with an FPU                | `Float`                |

## Compared to Other Crates

|                         | nanoaudio                          | [arduino-audio-tools](https://github.com/pschatzmann/arduino-audio-tools) | [rodio](https://crates.io/crates/rodio) | [symphonia](https://crates.io/crates/symphonia) | [dasp](https://crates.io/crates/dasp) |
| ----------------------- | ---------------------------------- | ------------------------- | ----------------------- | --------------------- | -------------------------- |
| Language                | Rust                               | C++                       | Rust                    | Rust                  | Rust                       |
| `no_std`, no allocator  | yes                                | Arduino framework         | no                      | no                    | yes                        |
| Conversion, volume, mixing | yes                             | yes                       | yes                     | no                    | building blocks            |
| Single-pass pipelines   | yes                                | stream chaining           | iterator chaining       | no                    | iterator chaining          |
| Decoders                | WAV, G.711, IMA ADPCM, MP3         | many                      | through symphonia       | many                  | no                         |
| Hardware output         | bring your own, ring buffer        | I2S and more              | through cpal            | no                    | no                         |
| License                 | MIT                                | GPL-3.0                   | MIT or Apache-2.0       | MPL-2.0               | MIT or Apache-2.0          |

Choose rodio or symphonia for desktop playback with `std`. Choose nanoaudio when you need audio processing and decoding on a microcontroller, in a kernel, or anywhere you manage the memory yourself.

## Minimum Supported Rust Version

nanoaudio requires Rust 1.89 or newer, and is tested against it in CI.

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).

---

Developed by [Alix ANNERAUD](https://alix.anneraud.fr).
