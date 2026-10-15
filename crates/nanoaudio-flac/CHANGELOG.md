# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0](https://github.com/AlixANNERAUD/nanoaudio/releases/tag/nanoaudio-flac-v0.1.0) - 2026-10-15

### Added

- [**breaking**] split codecs into separate crates ([#17](https://github.com/AlixANNERAUD/nanoaudio/pull/17))
- seek in FLAC streams ([#25](https://github.com/AlixANNERAUD/nanoaudio/pull/25))

### Changed

- share bit reading and CRC code between codecs ([#19](https://github.com/AlixANNERAUD/nanoaudio/pull/19))

### Documentation

- document the FLAC and MP3 crates ([#39](https://github.com/AlixANNERAUD/nanoaudio/pull/39))

### Fixed

- convert samples without losing precision and round to nearest ([#22](https://github.com/AlixANNERAUD/nanoaudio/pull/22))

### Performance

- *(flac)* use SIMD for LPC prediction on x86, aarch64 and wasm ([#31](https://github.com/AlixANNERAUD/nanoaudio/pull/31))
