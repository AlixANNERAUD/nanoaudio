# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0](https://github.com/AlixANNERAUD/nanoaudio/releases/tag/nanoaudio-analysis-v0.1.0) - 2026-10-15

### Added

- add an analysis tap stage and an analysis crate ([#95](https://github.com/AlixANNERAUD/nanoaudio/pull/95))
- *(analysis)* measure stereo balance and phase correlation ([#98](https://github.com/AlixANNERAUD/nanoaudio/pull/98))
- *(analysis)* detect tones with the Goertzel algorithm ([#101](https://github.com/AlixANNERAUD/nanoaudio/pull/101))
- *(analysis)* detect silence ([#100](https://github.com/AlixANNERAUD/nanoaudio/pull/100))
- *(analysis)* add a peak and RMS level meter ([#104](https://github.com/AlixANNERAUD/nanoaudio/pull/104))
- *(analysis)* add a filter-bank band analyzer for equalizer displays ([#119](https://github.com/AlixANNERAUD/nanoaudio/pull/119))
- *(analysis)* add VU and PPM meter ballistics ([#117](https://github.com/AlixANNERAUD/nanoaudio/pull/117))
- *(analysis)* measure clipping, DC offset and crest factor ([#105](https://github.com/AlixANNERAUD/nanoaudio/pull/105))

### Changed

- *(analysis)* move the level helpers from Representation to a Measure trait ([#97](https://github.com/AlixANNERAUD/nanoaudio/pull/97))
