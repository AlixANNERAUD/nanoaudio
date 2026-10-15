# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0](https://github.com/AlixANNERAUD/nanoaudio/compare/v0.1.0...v0.2.0) - 2026-10-15

### Added

- seek in MP3 streams ([#26](https://github.com/AlixANNERAUD/nanoaudio/pull/26))
- seek in SBC streams ([#23](https://github.com/AlixANNERAUD/nanoaudio/pull/23))
- *(dsp)* add optional TPDF dither when reducing bit depth ([#29](https://github.com/AlixANNERAUD/nanoaudio/pull/29))
- [**breaking**] mark open-ended public enums as non-exhaustive ([#18](https://github.com/AlixANNERAUD/nanoaudio/pull/18))
- seek in PCM, G.711, ADPCM, QOA and G.722 streams ([#21](https://github.com/AlixANNERAUD/nanoaudio/pull/21))
- seek in FLAC streams ([#25](https://github.com/AlixANNERAUD/nanoaudio/pull/25))
- [**breaking**] split codecs into separate crates ([#17](https://github.com/AlixANNERAUD/nanoaudio/pull/17))

### Changed

- share bit reading and CRC code between codecs ([#19](https://github.com/AlixANNERAUD/nanoaudio/pull/19))

### Documentation

- document the FLAC and MP3 crates ([#39](https://github.com/AlixANNERAUD/nanoaudio/pull/39))
- document the G.711, ADPCM, QOA, G.722 and SBC crates ([#41](https://github.com/AlixANNERAUD/nanoaudio/pull/41))
- document the core, buffer and dsp crates ([#40](https://github.com/AlixANNERAUD/nanoaudio/pull/40))
- add codec benchmarks and a guide to choosing a codec ([#32](https://github.com/AlixANNERAUD/nanoaudio/pull/32))
- correct the MP3 decoder memory figure ([#36](https://github.com/AlixANNERAUD/nanoaudio/pull/36))
- document the WAV, AIFF, metadata and facade crates ([#43](https://github.com/AlixANNERAUD/nanoaudio/pull/43))

### Fixed

- avoid integer overflow when parsing WAV and AIFF headers on 32-bit targets ([#20](https://github.com/AlixANNERAUD/nanoaudio/pull/20))
- convert samples without losing precision and round to nearest ([#22](https://github.com/AlixANNERAUD/nanoaudio/pull/22))
- allow enabling the fixed and float features together ([#3](https://github.com/AlixANNERAUD/nanoaudio/pull/3))

### Performance

- *(sbc)* use SIMD for the filterbanks on x86, aarch64 and wasm ([#28](https://github.com/AlixANNERAUD/nanoaudio/pull/28))
- *(g722)* use SIMD for the QMF filters on x86, aarch64 and wasm ([#30](https://github.com/AlixANNERAUD/nanoaudio/pull/30))
- *(flac)* use SIMD for LPC prediction on x86, aarch64 and wasm ([#31](https://github.com/AlixANNERAUD/nanoaudio/pull/31))
- *(g711)* find the segment without a search so encoding vectorizes ([#38](https://github.com/AlixANNERAUD/nanoaudio/pull/38))
- *(dsp)* process byte slices in fixed-size chunks ([#33](https://github.com/AlixANNERAUD/nanoaudio/pull/33))
- *(dsp)* keep 24-bit fixed-point volume in 32-bit arithmetic ([#34](https://github.com/AlixANNERAUD/nanoaudio/pull/34))
- speed up fixed-point processing on microcontrollers ([#24](https://github.com/AlixANNERAUD/nanoaudio/pull/24))
- *(mp3)* enable SIMD by default ([#27](https://github.com/AlixANNERAUD/nanoaudio/pull/27))
- *(qoa)* keep the LMS predictor scalar so it stays off the vector unit ([#35](https://github.com/AlixANNERAUD/nanoaudio/pull/35))

## [0.1.0](https://github.com/AlixANNERAUD/nanoaudio/releases/tag/v0.1.0) - 2026-10-15

### Documentation

- rewrite README and crate metadata for discoverability

### Miscellaneous

- upload test coverage to Codecov

### Other

- Add sample formats, conversion and volume
- Add single-producer single-consumer ring buffer
- Add mixer with per-source and master volumes
- Add WAV header parser
- Add checked conversion from u8 to Format
- Support fixed-point and floating-point processing
- Add chainable single-pass processing pipeline
- Add G.711 and IMA ADPCM codecs
- Decode compressed WAV files
- Add MP3 decoding behind the mp3 feature
