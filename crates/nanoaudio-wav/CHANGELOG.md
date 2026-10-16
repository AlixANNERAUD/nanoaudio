# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2](https://github.com/AlixANNERAUD/nanoaudio/compare/nanoaudio-wav-v0.1.1...nanoaudio-wav-v0.1.2) - 2026-10-16

### Miscellaneous

- updated the following local packages: nanoaudio-adpcm, nanoaudio-g711

## [0.1.1](https://github.com/AlixANNERAUD/nanoaudio/compare/nanoaudio-wav-v0.1.0...nanoaudio-wav-v0.1.1) - 2026-10-15

### Changed

- share the RIFF and AIFF chunk walker ([#108](https://github.com/AlixANNERAUD/nanoaudio/pull/108))

## [0.1.0](https://github.com/AlixANNERAUD/nanoaudio/releases/tag/nanoaudio-wav-v0.1.0) - 2026-10-15

### Added

- [**breaking**] split codecs into separate crates ([#17](https://github.com/AlixANNERAUD/nanoaudio/pull/17))
- [**breaking**] mark open-ended public enums as non-exhaustive ([#18](https://github.com/AlixANNERAUD/nanoaudio/pull/18))
- seek in PCM, G.711, ADPCM, QOA and G.722 streams ([#21](https://github.com/AlixANNERAUD/nanoaudio/pull/21))

### Documentation

- document the WAV, AIFF, metadata and facade crates ([#43](https://github.com/AlixANNERAUD/nanoaudio/pull/43))

### Fixed

- avoid integer overflow when parsing WAV and AIFF headers on 32-bit targets ([#20](https://github.com/AlixANNERAUD/nanoaudio/pull/20))
