# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2](https://github.com/AlixANNERAUD/nanoaudio/compare/nanoaudio-adpcm-v0.1.1...nanoaudio-adpcm-v0.1.2) - 2026-10-16

### Performance

- *(adpcm)* avoid software division in MS ADPCM decode and encode ([#140](https://github.com/AlixANNERAUD/nanoaudio/pull/140))

## [0.1.1](https://github.com/AlixANNERAUD/nanoaudio/compare/nanoaudio-adpcm-v0.1.0...nanoaudio-adpcm-v0.1.1) - 2026-10-15

### Performance

- make the channel limit a const generic in ADPCM, QOA and pipelines ([#112](https://github.com/AlixANNERAUD/nanoaudio/pull/112))

## [0.1.0](https://github.com/AlixANNERAUD/nanoaudio/releases/tag/nanoaudio-adpcm-v0.1.0) - 2026-10-15

### Added

- [**breaking**] split codecs into separate crates ([#17](https://github.com/AlixANNERAUD/nanoaudio/pull/17))
- seek in PCM, G.711, ADPCM, QOA and G.722 streams ([#21](https://github.com/AlixANNERAUD/nanoaudio/pull/21))

### Documentation

- document the G.711, ADPCM, QOA, G.722 and SBC crates ([#41](https://github.com/AlixANNERAUD/nanoaudio/pull/41))
