use nanoaudio_dsp::{DefaultRepresentation, Stream};

use crate::{Patch, Stage, Synthesis, Voice, note_increment};

/// A polyphonic synthesizer with `VOICES` voices, producing mono audio.
///
/// It is a [`Stream`] that never ends, so it starts a pipeline like any other source. Notes, and changes to the voices, apply from the next sample, so writing a buffer in two parts with a change in between places that change on an exact sample. Voices are added without scaling and saturate in [`Fixed`](nanoaudio_dsp::Fixed): lower the velocities or the volume of the pipeline when many play at once.
///
/// [`render`](Self::render) gives the same values as the stream, faster, since it runs one voice at a time over a whole buffer.
#[derive(Debug, Clone)]
pub struct Synth<R: Synthesis = DefaultRepresentation, const VOICES: usize = 8> {
    voices: [Voice<R>; VOICES],
    sample_rate: u32,
    counter: u32,
}

impl<R: Synthesis, const VOICES: usize> Synth<R, VOICES> {
    /// Creates a silent synthesizer for audio at `sample_rate` Hz.
    pub fn new(sample_rate: u32) -> Self {
        const { assert!(VOICES > 0, "a synthesizer needs at least one voice") };

        Self {
            voices: core::array::from_fn(|_| Voice::new()),
            sample_rate,
            counter: 0,
        }
    }

    /// Returns the sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Starts `note` (a MIDI note number, 69 is A4 at 440 Hz) on `channel` with `velocity` from 0 to 127, and returns the index of its voice.
    ///
    /// The voice is the one already playing the same note on the same channel, else an idle one, else the quietest releasing one, else the oldest. `channel` only groups notes for [`note_off`](Self::note_off), for example one per MIDI channel or tracker track.
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8, patch: &Patch) -> usize {
        let index = self.allocate(channel, note);
        let increment = note_increment((note as i32) << 8, self.sample_rate);
        let voice = &mut self.voices[index];

        voice.play(patch, increment, velocity, self.sample_rate);
        voice.assign(channel, note, self.counter);
        self.counter = self.counter.wrapping_add(1);

        index
    }

    fn allocate(&self, channel: u8, note: u8) -> usize {
        let playing = self.voices.iter().position(|voice| {
            voice.is_active() && voice.channel() == channel && voice.note() == note
        });

        if let Some(index) =
            playing.or_else(|| self.voices.iter().position(|voice| !voice.is_active()))
        {
            return index;
        }

        let mut best = (0, (0, 0, 0));

        for (index, voice) in self.voices.iter().enumerate() {
            let age = self.counter.wrapping_sub(voice.stamp);
            let score = if voice.envelope().stage() == Stage::Release {
                (1, u16::MAX - voice.envelope().level(), age)
            } else {
                (0, 0, age)
            };

            if score > best.1 {
                best = (index, score);
            }
        }

        best.0
    }

    /// Starts the release of every voice playing `note` on `channel`.
    pub fn note_off(&mut self, channel: u8, note: u8) {
        for voice in &mut self.voices {
            if voice.channel() == channel && voice.note() == note {
                voice.release();
            }
        }
    }

    /// Starts the release of every voice.
    pub fn release_all(&mut self) {
        for voice in &mut self.voices {
            voice.release();
        }
    }

    /// Silences every voice at once.
    pub fn stop_all(&mut self) {
        for voice in &mut self.voices {
            voice.stop();
        }
    }

    /// Returns the number of voices making sound.
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|voice| voice.is_active()).count()
    }

    /// Returns the voices.
    pub fn voices(&self) -> &[Voice<R>; VOICES] {
        &self.voices
    }

    /// Returns the voices, to change their pitch, velocity, filter or envelope while they play.
    pub fn voices_mut(&mut self) -> &mut [Voice<R>; VOICES] {
        &mut self.voices
    }

    /// Writes the next `output.len()` values, replacing the content of `output`.
    pub fn render(&mut self, output: &mut [R::Value]) {
        output.fill(R::SILENCE);

        for voice in &mut self.voices {
            voice.add_to(output);
        }
    }

    #[inline]
    fn next_value(&mut self) -> R::Value {
        let mut value = R::SILENCE;

        for voice in &mut self.voices {
            if voice.is_active() {
                value = R::add(value, voice.next_value());
            }
        }

        value
    }
}

impl<R: Synthesis, const VOICES: usize> Stream for Synth<R, VOICES> {
    type Representation = R;

    fn channels(&self) -> u8 {
        1
    }

    #[inline]
    fn next_frame(&mut self, frame: &mut [R::Value]) -> bool {
        frame.fill(self.next_value());
        true
    }
}

impl<R: Synthesis, const VOICES: usize> Stream for &mut Synth<R, VOICES> {
    type Representation = R;

    fn channels(&self) -> u8 {
        1
    }

    #[inline]
    fn next_frame(&mut self, frame: &mut [R::Value]) -> bool {
        frame.fill(self.next_value());
        true
    }
}

#[cfg(test)]
mod tests {
    use nanoaudio_dsp::{Fixed, Float, Volume};

    use super::*;
    use crate::{Adsr, FilterMode, FilterSettings, Waveform};

    extern crate std;

    use std::vec;
    use std::vec::Vec;

    const SAMPLE_RATE: u32 = 44_100;

    fn patches() -> Vec<Patch> {
        let envelope = Adsr::new(100, 400, 30_000, 300);
        let mut patches: Vec<Patch> = [
            Waveform::Sine,
            Waveform::Square,
            Waveform::Saw,
            Waveform::Triangle,
            Waveform::Noise,
        ]
        .into_iter()
        .map(|waveform| Patch::new(waveform, envelope))
        .collect();

        for (mode, cutoff) in [
            (FilterMode::LowPass, 800.0),
            (FilterMode::BandPass, 2_000.0),
            (FilterMode::HighPass, 300.0),
        ] {
            let mut patch = Patch::new(Waveform::Saw, envelope);
            patch.filter = Some(FilterSettings::new(mode, cutoff, 0.5));
            patch.pulse_width = 20_000;
            patches.push(patch);
        }

        patches
    }

    fn play<R: Synthesis, const VOICES: usize>(synth: &mut Synth<R, VOICES>, patch: &Patch) {
        for (index, note) in [48u8, 55, 60, 64, 67].into_iter().enumerate() {
            synth.note_on(index as u8 % 2, note, 8 + index as u8 * 6, patch);
        }
    }

    fn render_both<R: Synthesis>(patch: &Patch, samples: usize) -> (Vec<R::Value>, Vec<R::Value>) {
        let mut rendered = vec![R::SILENCE; samples];
        let mut streamed = vec![R::SILENCE; samples];
        let mut synth = Synth::<R, 4>::new(SAMPLE_RATE);
        play(&mut synth, patch);
        let mut stream = synth.clone();

        let (first, second) = rendered.split_at_mut(samples / 3);
        synth.render(first);
        synth.note_off(0, 60);
        synth.render(second);

        for (index, value) in streamed.iter_mut().enumerate() {
            if index == samples / 3 {
                stream.note_off(0, 60);
            }

            let mut frame = [R::SILENCE];
            assert!(stream.next_frame(&mut frame));
            *value = frame[0];
        }

        (rendered, streamed)
    }

    #[test]
    fn render_matches_the_stream() {
        let samples = if cfg!(miri) { 96 } else { 4_000 };

        for patch in patches() {
            let (rendered, streamed) = render_both::<Fixed>(&patch, samples);
            assert_eq!(rendered, streamed, "{patch:?}");

            let (rendered, streamed) = render_both::<Float>(&patch, samples);
            assert!(
                rendered
                    .iter()
                    .zip(&streamed)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{patch:?}"
            );
        }
    }

    #[test]
    fn fixed_matches_float() {
        let samples = if cfg!(miri) { 96 } else { 8_000 };

        for patch in patches() {
            let mut fixed = vec![0; samples];
            let mut float = vec![0.0; samples];
            let mut fixed_synth = Synth::<Fixed, 4>::new(SAMPLE_RATE);
            let mut float_synth = Synth::<Float, 4>::new(SAMPLE_RATE);
            play(&mut fixed_synth, &patch);
            play(&mut float_synth, &patch);
            fixed_synth.render(&mut fixed);
            float_synth.render(&mut float);

            let tolerance = if patch.filter.is_some() {
                2.0e-4
            } else {
                1.0e-6
            };
            let worst = fixed
                .iter()
                .zip(&float)
                .map(|(fixed, float)| (*fixed as f64 / 2_147_483_648.0 - *float as f64).abs())
                .fold(0.0, f64::max);

            assert!(worst < tolerance, "{patch:?} {worst}");
        }
    }

    #[test]
    fn notes_start_and_stop_on_exact_samples() {
        let mut synth = Synth::<Fixed, 2>::new(SAMPLE_RATE);
        let patch = Patch::new(Waveform::Square, Adsr::new(0, 0, u16::MAX, 0));
        let mut output = [0; 64];

        synth.render(&mut output[..10]);
        synth.note_on(0, 69, 127, &patch);
        synth.render(&mut output[10..30]);
        synth.note_off(0, 69);
        synth.render(&mut output[30..]);

        assert!(output[..10].iter().all(|value| *value == 0));
        assert!(output[10..30].iter().all(|value| *value != 0));
        assert!(output[30..].iter().all(|value| *value == 0));
        assert_eq!(synth.active_voices(), 0);
    }

    #[test]
    fn voices_are_stolen_quietest_released_then_oldest() {
        let mut synth = Synth::<Fixed, 3>::new(SAMPLE_RATE);
        let patch = Patch::new(Waveform::Sine, Adsr::new(0, 0, u16::MAX, 1_000));
        let mut output = [0; 100];

        assert_eq!(synth.note_on(0, 60, 100, &patch), 0);
        assert_eq!(synth.note_on(0, 62, 100, &patch), 1);
        assert_eq!(synth.note_on(1, 60, 100, &patch), 2);
        assert_eq!(synth.active_voices(), 3);

        assert_eq!(synth.note_on(0, 62, 90, &patch), 1);
        assert_eq!(synth.voices()[1].velocity(), 90);

        assert_eq!(synth.note_on(0, 64, 100, &patch), 0);
        assert_eq!(synth.voices()[0].note(), 64);

        synth.note_off(0, 62);
        synth.render(&mut output);
        synth.note_off(1, 60);
        synth.render(&mut output[..10]);
        assert_eq!(synth.note_on(0, 65, 100, &patch), 1);

        assert_eq!(synth.note_on(0, 67, 100, &patch), 2);
        assert_eq!(synth.note_on(0, 69, 100, &patch), 0);

        synth.release_all();
        assert_eq!(synth.active_voices(), 3);
        synth.stop_all();
        assert_eq!(synth.active_voices(), 0);
    }

    #[test]
    fn plugs_into_a_pipeline() {
        let mut synth: Synth<Fixed> = Synth::new(SAMPLE_RATE);
        synth.note_on(0, 69, 127, &Patch::new(Waveform::Saw, Adsr::GATE));
        let mut output = [0i16; 64];

        let frames = (&mut synth)
            .volume(Volume::from_percent(50))
            .to_channels(2)
            .unwrap()
            .write_samples(&mut output);

        assert_eq!(frames, 32);
        assert!(output.chunks(2).all(|frame| frame[0] == frame[1]));
        assert!(output.iter().all(|sample| sample.unsigned_abs() <= 16_384));
        assert_eq!(synth.active_voices(), 1);
    }

    #[test]
    fn fixed_saturates_without_overflow() {
        let samples = if cfg!(miri) { 64 } else { 8_192 };
        let mut output = vec![0; samples];

        for mut patch in patches() {
            patch.envelope = Adsr::GATE;

            if let Some(filter) = &mut patch.filter {
                filter.resonance = 1.0;
            }

            for pitch in [0u8, 60, 127] {
                let mut synth = Synth::<Fixed, 8>::new(SAMPLE_RATE);

                for _ in 0..8 {
                    let index = synth.note_on(0, pitch, 127, &patch);
                    synth.voices_mut()[index].assign(1, index as u8, 0);
                }

                synth.render(&mut output);
                assert!(output.iter().any(|value| *value != 0));
            }
        }
    }
}
