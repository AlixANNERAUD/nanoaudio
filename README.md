# nanoaudio

Allocation-free, `no_std` building blocks for PCM audio on microcontrollers and beyond.

`nanoaudio` does not own a pipeline or talk to hardware. It provides small, composable pieces that an audio driver, a mixer task or an operating system can assemble with the memory it already has.

## Features

- Sample formats (`U8`, `I16`, `I32`, `F32`) and stream configuration (`#[repr(C)]`, ready to cross an ABI)
- Sample conversion between formats and channel layouts, on typed slices or raw little-endian bytes
- Fixed-point volume (Q4.12), applied in place
- Saturating mixing, and a fixed-size mixer with per-source and master volumes

- Single-producer/single-consumer ring buffer with contiguous grants, for DMA and audio callbacks

Planned: a WAV header parser.

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
