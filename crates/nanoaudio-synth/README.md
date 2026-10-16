# nanoaudio-synth

Oscillators, envelopes, a resonant filter and a polyphonic synthesizer, in fixed or floating point. It is `no_std` and does not allocate.

A `Synth` holds a fixed number of voices, each an oscillator (sine, square with a variable pulse width, saw, triangle or noise), an optional state-variable filter and an ADSR envelope. It is a `Stream`, so it starts a pipeline like any other source. The parts can also be used on their own, to build other instruments or sound effects.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-synth = "0.1"
nanoaudio-dsp = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::Format;
use nanoaudio_dsp::{Stream, Volume};
use nanoaudio_synth::{Adsr, FilterSettings, Patch, Synth, Waveform};

let mut synth: Synth = Synth::new(44_100);
let mut patch = Patch::new(Waveform::Saw, Adsr::from_millis(10, 200, 40_000, 300, 44_100));
patch.filter = Some(FilterSettings::low_pass(2_000.0, 0.3));

for note in [60, 64, 67] {
    synth.note_on(0, note, 100, &patch);
}

let mut output = [0u8; 4096];
let frames = (&mut synth)
    .volume(Volume::from_percent(50))
    .to_channels(2)
    .unwrap()
    .write_bytes(&mut output, Format::I16);

assert_eq!(frames, 1024);

synth.note_off(0, 64);
```

Square and saw waves are band-limited with PolyBLEP, the sine is a polynomial accurate to about 3 LSB at 16 bits, and the filter is a trapezoidal state-variable filter that stays stable up to near half the sample rate. Envelope stages last an exact number of samples. Pitches are integers, in 1/256 semitone, and frequencies, envelopes and oscillators use integers in both representations.

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
