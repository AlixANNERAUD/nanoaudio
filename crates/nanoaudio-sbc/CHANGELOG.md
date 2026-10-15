# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0](https://github.com/AlixANNERAUD/nanoaudio/releases/tag/nanoaudio-sbc-v0.1.0) - 2026-10-15

### Added

- [**breaking**] split codecs into separate crates ([#17](https://github.com/AlixANNERAUD/nanoaudio/pull/17))
- seek in SBC streams ([#23](https://github.com/AlixANNERAUD/nanoaudio/pull/23))

### Changed

- share bit reading and CRC code between codecs ([#19](https://github.com/AlixANNERAUD/nanoaudio/pull/19))

### Documentation

- document the G.711, ADPCM, QOA, G.722 and SBC crates ([#41](https://github.com/AlixANNERAUD/nanoaudio/pull/41))

### Performance

- *(sbc)* use SIMD for the filterbanks on x86, aarch64 and wasm ([#28](https://github.com/AlixANNERAUD/nanoaudio/pull/28))
