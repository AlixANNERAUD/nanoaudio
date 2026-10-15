# nanoaudio

Allocation-free, `no_std` building blocks for PCM audio on microcontrollers and beyond.

`nanoaudio` does not own a pipeline or talk to hardware. It provides small, composable pieces that an audio driver, a mixer task or an operating system can assemble with the memory it already has.

## Features

- Sample formats (`U8`, `I16`, `I32`, `F32`) and stream configuration (`#[repr(C)]`, ready to cross an ABI)
- Sample conversion between formats and channel layouts, on typed slices or raw little-endian bytes
- Volume (Q4.12 control value), applied in place
- Saturating mixing, and a fixed-size mixer with per-source and master volumes
- Single-producer/single-consumer ring buffer with contiguous grants, for DMA and audio callbacks
- WAV header parser that works on a partial buffer and reports when it needs more bytes

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
- No codecs: decoding is left to applications

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
