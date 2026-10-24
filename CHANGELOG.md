# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.3](https://github.com/AlixANNERAUD/nanoaudio/compare/v0.2.2...v0.2.3) - 2026-10-24

### Added

- *(opus)* decode Opus in Ogg ([#151](https://github.com/AlixANNERAUD/nanoaudio/pull/151))

## [0.2.2](https://github.com/AlixANNERAUD/nanoaudio/compare/v0.2.1...v0.2.2) - 2026-10-16

### Added

- *(analysis)* add an FFT spectrum analyzer ([#131](https://github.com/AlixANNERAUD/nanoaudio/pull/131))
- *(synth)* add a synthesizer crate ([#150](https://github.com/AlixANNERAUD/nanoaudio/pull/150))

### Changed

- *(g711)* drop the dead target-feature check in the build script ([#134](https://github.com/AlixANNERAUD/nanoaudio/pull/134))
- *(analysis)* share the math helpers between analyzers ([#149](https://github.com/AlixANNERAUD/nanoaudio/pull/149))

### Performance

- *(adpcm)* avoid software division in MS ADPCM decode and encode ([#140](https://github.com/AlixANNERAUD/nanoaudio/pull/140))
- *(flac)* store blocks as i16 for streams of up to 16 bits ([#136](https://github.com/AlixANNERAUD/nanoaudio/pull/136))

## [0.2.1](https://github.com/AlixANNERAUD/nanoaudio/compare/v0.2.0...v0.2.1) - 2026-10-15

### Added

- add a browser audio driver based on AudioWorklet ([#103](https://github.com/AlixANNERAUD/nanoaudio/pull/103))
- *(web)* resample when the context rate differs ([#111](https://github.com/AlixANNERAUD/nanoaudio/pull/111))
- *(cpal)* resample when the device rate differs ([#110](https://github.com/AlixANNERAUD/nanoaudio/pull/110))
- *(mp3)* trim encoder delay and padding for gapless playback ([#115](https://github.com/AlixANNERAUD/nanoaudio/pull/115))
- *(analysis)* measure stereo balance and phase correlation ([#98](https://github.com/AlixANNERAUD/nanoaudio/pull/98))
- *(analysis)* detect tones with the Goertzel algorithm ([#101](https://github.com/AlixANNERAUD/nanoaudio/pull/101))
- *(analysis)* detect silence ([#100](https://github.com/AlixANNERAUD/nanoaudio/pull/100))
- *(analysis)* add a peak and RMS level meter ([#104](https://github.com/AlixANNERAUD/nanoaudio/pull/104))
- *(analysis)* add a filter-bank band analyzer for equalizer displays ([#119](https://github.com/AlixANNERAUD/nanoaudio/pull/119))
- *(analysis)* add VU and PPM meter ballistics ([#117](https://github.com/AlixANNERAUD/nanoaudio/pull/117))
- *(analysis)* measure clipping, DC offset and crest factor ([#105](https://github.com/AlixANNERAUD/nanoaudio/pull/105))
- *(dsp)* add sample rate conversion ([#106](https://github.com/AlixANNERAUD/nanoaudio/pull/106))
- add an analysis tap stage and an analysis crate ([#95](https://github.com/AlixANNERAUD/nanoaudio/pull/95))
- add a desktop audio driver based on cpal ([#99](https://github.com/AlixANNERAUD/nanoaudio/pull/99))

### Changed

- *(analysis)* move the level helpers from Representation to a Measure trait ([#97](https://github.com/AlixANNERAUD/nanoaudio/pull/97))
- share the RIFF and AIFF chunk walker ([#108](https://github.com/AlixANNERAUD/nanoaudio/pull/108))

### Documentation

- *(flac)* make the seek example independent of test fixtures ([#69](https://github.com/AlixANNERAUD/nanoaudio/pull/69))

### Fixed

- *(dsp)* compare float outputs without depending on NaN payloads in tests ([#114](https://github.com/AlixANNERAUD/nanoaudio/pull/114))

### Performance

- *(sbc)* avoid 64-bit multiplies on Cortex-M0 ([#116](https://github.com/AlixANNERAUD/nanoaudio/pull/116))
- *(flac)* skip frames by header after a seek ([#135](https://github.com/AlixANNERAUD/nanoaudio/pull/135))
- *(g711)* use the lookup table on Armv8-M Baseline ([#71](https://github.com/AlixANNERAUD/nanoaudio/pull/71))
- make the channel limit a const generic in ADPCM, QOA and pipelines ([#112](https://github.com/AlixANNERAUD/nanoaudio/pull/112))
- *(dsp)* inline narrow float conversions in Float hot loops on FPU targets ([#125](https://github.com/AlixANNERAUD/nanoaudio/pull/125))
- *(dsp)* keep the fixed-point volume stage in 32-bit arithmetic on Cortex-M0+ and M23 ([#118](https://github.com/AlixANNERAUD/nanoaudio/pull/118))
- *(dsp)* vectorize dither with a jumpable noise generator ([#132](https://github.com/AlixANNERAUD/nanoaudio/pull/132))
- *(dsp)* keep 32-bit fixed-point volume in 32-bit arithmetic where it pays off ([#133](https://github.com/AlixANNERAUD/nanoaudio/pull/133))
- *(core)* inline the 24-bit sample conversions ([#89](https://github.com/AlixANNERAUD/nanoaudio/pull/89))
- *(mp3)* use a smaller default input window ([#107](https://github.com/AlixANNERAUD/nanoaudio/pull/107))

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
