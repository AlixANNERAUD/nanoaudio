# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1](https://github.com/AlixANNERAUD/nanoaudio/compare/nanoaudio-mp3-v0.1.0...nanoaudio-mp3-v0.1.1) - 2026-10-15

### Added

- *(mp3)* trim encoder delay and padding for gapless playback ([#115](https://github.com/AlixANNERAUD/nanoaudio/pull/115))

### Performance

- *(mp3)* use a smaller default input window ([#107](https://github.com/AlixANNERAUD/nanoaudio/pull/107))

## [0.1.0](https://github.com/AlixANNERAUD/nanoaudio/releases/tag/nanoaudio-mp3-v0.1.0) - 2026-10-15

### Added

- [**breaking**] split codecs into separate crates ([#17](https://github.com/AlixANNERAUD/nanoaudio/pull/17))
- seek in MP3 streams ([#26](https://github.com/AlixANNERAUD/nanoaudio/pull/26))

### Documentation

- document the FLAC and MP3 crates ([#39](https://github.com/AlixANNERAUD/nanoaudio/pull/39))

### Performance

- *(mp3)* enable SIMD by default ([#27](https://github.com/AlixANNERAUD/nanoaudio/pull/27))
