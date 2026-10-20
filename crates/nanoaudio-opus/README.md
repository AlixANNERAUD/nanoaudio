# nanoaudio-opus

An Ogg Opus decoder built on [`opus-rs`](https://crates.io/crates/opus-rs) and [`nanoaudio-ogg`](https://crates.io/crates/nanoaudio-ogg). It is `no_std` and does not allocate. It decodes mono and stereo streams to 48 kHz, drops the pre-skip and trims the end for gapless playback, applies the output gain, conceals lost pages, and seeks by granule position.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-opus = "0.1"
nanoaudio-core = "0.1"
```

```rust
use core::mem::MaybeUninit;
use nanoaudio_core::{Result, codec::Decoder};
use nanoaudio_opus::OpusDecoder;

fn decode(slot: &mut MaybeUninit<OpusDecoder>, input: &[u8], output: &mut [i16]) -> Result<usize> {
    let decoder = OpusDecoder::init(slot);
    decoder.finish();
    let progress = decoder.decode(input, output)?;
    Ok(progress.produced)
}
```

## Memory

The `opus-rs` decoder takes about 175 KB, so `OpusDecoder` is built in place with `init`, for example in a `static` from `static_cell`, and never goes through the stack. `new` is `const` and can initialize a `static` too. The second parameter sets the longest packet, in frames: the default, 120 ms, adds a 45 KiB buffer, while 20 ms packets, what most encoders write, need 7.5 KiB:

```rust
use nanoaudio_opus::OpusDecoder;

static mut DECODER: OpusDecoder<i16, 960> = OpusDecoder::new();
```

Each Ogg page must be whole in the input before it is decoded, because its CRC is checked first. Pages can reach 65,307 bytes, but most files use a few kilobytes.

On targets without compare-and-swap, such as `thumbv6m-none-eabi` (Cortex-M0+) and `riscv32imc-unknown-none-elf` (ESP32-C3), `opus-rs` goes through [`portable-atomic`](https://crates.io/crates/portable-atomic), which needs either its `critical-section` feature or `--cfg portable_atomic_unsafe_assume_single_core` on single-core chips. The `portable-atomic` feature turns it on for other targets too.

## Seeking

`OpusDecoder` implements `Seek` once it has read the headers. Byte offsets are counted from the first byte fed to the decoder, and frame positions from the first frame after the pre-skip. Ogg pages carry their granule position, so the decoder bisects the file: when a request lands too far from the target, `decode` returns `Error::SeekTo(offset)`, and the caller resumes from `offset`. `set_stream_length` lets the first request interpolate over the whole file. Without it, the decoder probes forward until it finds the end, which the caller signals by calling `finish` once it runs out of input.

The decoder resumes 80 ms or more before the target, as RFC 7845 recommends, and drops those frames, so the output lands exactly on the requested frame.

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
