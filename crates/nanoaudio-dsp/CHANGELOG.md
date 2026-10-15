# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1](https://github.com/AlixANNERAUD/nanoaudio/compare/nanoaudio-dsp-v0.1.0...nanoaudio-dsp-v0.1.1) - 2026-10-15

### Added

- add an analysis tap stage and an analysis crate ([#95](https://github.com/AlixANNERAUD/nanoaudio/pull/95))
- *(dsp)* add sample rate conversion ([#106](https://github.com/AlixANNERAUD/nanoaudio/pull/106))

### Changed

- *(analysis)* move the level helpers from Representation to a Measure trait ([#97](https://github.com/AlixANNERAUD/nanoaudio/pull/97))

### Fixed

- *(dsp)* compare float outputs without depending on NaN payloads in tests ([#114](https://github.com/AlixANNERAUD/nanoaudio/pull/114))

### Performance

- *(core)* inline the 24-bit sample conversions ([#89](https://github.com/AlixANNERAUD/nanoaudio/pull/89))
- make the channel limit a const generic in ADPCM, QOA and pipelines ([#112](https://github.com/AlixANNERAUD/nanoaudio/pull/112))
- *(dsp)* inline narrow float conversions in Float hot loops on FPU targets ([#125](https://github.com/AlixANNERAUD/nanoaudio/pull/125))
- *(dsp)* keep the fixed-point volume stage in 32-bit arithmetic on Cortex-M0+ and M23 ([#118](https://github.com/AlixANNERAUD/nanoaudio/pull/118))
- *(dsp)* vectorize dither with a jumpable noise generator ([#132](https://github.com/AlixANNERAUD/nanoaudio/pull/132))
- *(dsp)* keep 32-bit fixed-point volume in 32-bit arithmetic where it pays off ([#133](https://github.com/AlixANNERAUD/nanoaudio/pull/133))

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
