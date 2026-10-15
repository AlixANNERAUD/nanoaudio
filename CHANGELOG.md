# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
