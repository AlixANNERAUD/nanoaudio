# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0](https://github.com/AlixANNERAUD/nanoaudio/releases/tag/nanoaudio-dsp-v0.1.0) - 2026-10-15

### Added

- [**breaking**] split codecs into separate crates ([#17](https://github.com/AlixANNERAUD/nanoaudio/pull/17))
- *(dsp)* add optional TPDF dither when reducing bit depth ([#29](https://github.com/AlixANNERAUD/nanoaudio/pull/29))

### Documentation

- document the core, buffer and dsp crates ([#40](https://github.com/AlixANNERAUD/nanoaudio/pull/40))

### Fixed

- convert samples without losing precision and round to nearest ([#22](https://github.com/AlixANNERAUD/nanoaudio/pull/22))

### Performance

- speed up fixed-point processing on microcontrollers ([#24](https://github.com/AlixANNERAUD/nanoaudio/pull/24))
- *(dsp)* process byte slices in fixed-size chunks ([#33](https://github.com/AlixANNERAUD/nanoaudio/pull/33))
- *(dsp)* keep 24-bit fixed-point volume in 32-bit arithmetic ([#34](https://github.com/AlixANNERAUD/nanoaudio/pull/34))
