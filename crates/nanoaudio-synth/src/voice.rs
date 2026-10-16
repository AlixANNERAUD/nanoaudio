use crate::{Adsr, Envelope, Filter, FilterSettings, Oscillator, Synthesis, Waveform};

/// The sound of a note: a waveform, an envelope and an optional filter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    /// The oscillator waveform.
    pub waveform: Waveform,
    /// The pulse width of [`Waveform::Square`], out of 65 536.
    pub pulse_width: u16,
    /// The amplitude envelope.
    pub envelope: Adsr,
    /// The filter, or `None` to leave the oscillator unfiltered, which is cheaper.
    pub filter: Option<FilterSettings>,
}

impl Patch {
    /// Creates an unfiltered patch with a square pulse width.
    pub const fn new(waveform: Waveform, envelope: Adsr) -> Self {
        Self {
            waveform,
            pulse_width: 1 << 15,
            envelope,
            filter: None,
        }
    }
}

impl Default for Patch {
    fn default() -> Self {
        Self::new(Waveform::Sine, Adsr::GATE)
    }
}

/// One voice: an [`Oscillator`], an optional [`Filter`] and an [`Envelope`], scaled by a velocity.
///
/// A [`Synth`](crate::Synth) holds a fixed number of them. Its parts can be changed while it plays, between two samples, for vibrato, pitch bends, pulse-width or filter sweeps.
#[derive(Debug, Clone)]
pub struct Voice<R: Synthesis> {
    oscillator: Oscillator,
    envelope: Envelope,
    filter: Filter<R>,
    filtered: bool,
    gain: i32,
    velocity: u8,
    channel: u8,
    note: u8,
    pub(crate) stamp: u32,
}

impl<R: Synthesis> Default for Voice<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Synthesis> Voice<R> {
    /// Creates an idle voice.
    pub fn new() -> Self {
        Self {
            oscillator: Oscillator::new(Waveform::Sine),
            envelope: Envelope::new(Adsr::GATE),
            filter: Filter::default(),
            filtered: false,
            gain: 0,
            velocity: 0,
            channel: 0,
            note: 0,
            stamp: 0,
        }
    }

    /// Starts `patch` at the phase increment `increment` and `velocity`, from 0 to 127, for audio at `sample_rate`.
    ///
    /// An idle voice starts from phase 0 with a cleared filter. A playing voice keeps its phase, filter state and envelope level, so taking it over does not click.
    pub fn play(&mut self, patch: &Patch, increment: u32, velocity: u8, sample_rate: u32) {
        if self.envelope.is_idle() {
            self.oscillator.set_phase(0);
            self.filter.reset();
        }

        self.oscillator.set_waveform(patch.waveform);
        self.oscillator.set_pulse_width(patch.pulse_width);
        self.oscillator.set_increment(increment);
        self.envelope.set_settings(patch.envelope);
        self.set_filter(patch.filter, sample_rate);
        self.set_velocity(velocity);
        self.envelope.trigger();
    }

    /// Starts the release of the envelope.
    pub fn release(&mut self) {
        self.envelope.release();
    }

    /// Silences the voice at once.
    pub fn stop(&mut self) {
        self.envelope.stop();
    }

    /// Returns `true` while the voice makes sound, until the end of its release.
    pub fn is_active(&self) -> bool {
        !self.envelope.is_idle()
    }

    /// Returns the channel given to [`Synth::note_on`](crate::Synth::note_on).
    pub fn channel(&self) -> u8 {
        self.channel
    }

    /// Returns the note given to [`Synth::note_on`](crate::Synth::note_on).
    pub fn note(&self) -> u8 {
        self.note
    }

    pub(crate) fn assign(&mut self, channel: u8, note: u8, stamp: u32) {
        self.channel = channel;
        self.note = note;
        self.stamp = stamp;
    }

    /// Returns the velocity.
    pub fn velocity(&self) -> u8 {
        self.velocity
    }

    /// Sets the velocity, a linear gain from 0 to 127, where 127 is full scale.
    pub fn set_velocity(&mut self, velocity: u8) {
        let velocity = velocity.min(127);
        self.velocity = velocity;
        self.gain = velocity as i32 * 258 + velocity as i32 / 64;
    }

    /// Returns the oscillator.
    pub fn oscillator(&self) -> &Oscillator {
        &self.oscillator
    }

    /// Returns the oscillator, to change its frequency or pulse width.
    pub fn oscillator_mut(&mut self) -> &mut Oscillator {
        &mut self.oscillator
    }

    /// Returns the envelope.
    pub fn envelope(&self) -> &Envelope {
        &self.envelope
    }

    /// Returns the envelope.
    pub fn envelope_mut(&mut self) -> &mut Envelope {
        &mut self.envelope
    }

    /// Returns the filter, or `None` if the voice is unfiltered.
    pub fn filter(&self) -> Option<&Filter<R>> {
        self.filtered.then_some(&self.filter)
    }

    /// Returns the filter, to change its settings, or `None` if the voice is unfiltered.
    pub fn filter_mut(&mut self) -> Option<&mut Filter<R>> {
        self.filtered.then_some(&mut self.filter)
    }

    /// Turns the filter on with `settings`, or off with `None`, for audio at `sample_rate`.
    ///
    /// A filter that was off starts from a cleared state.
    pub fn set_filter(&mut self, settings: Option<FilterSettings>, sample_rate: u32) {
        match settings {
            Some(settings) => {
                if !self.filtered {
                    self.filter.reset();
                }

                self.filter.set(settings, sample_rate);
                self.filtered = true;
            }
            None => self.filtered = false,
        }
    }

    #[inline(always)]
    fn compute(&mut self) -> R::Value {
        let level = self.envelope.next_level();
        let sample = self.oscillator.next_sample();
        let amplitude = (level * self.gain) >> 15;

        if self.filtered {
            self.filter
                .process_scaled(R::from_oscillator(sample), amplitude)
        } else {
            R::amplify_oscillator(sample, amplitude)
        }
    }

    /// Returns the next value, or silence once the voice is idle.
    #[inline]
    pub fn next_value(&mut self) -> R::Value {
        if self.envelope.is_idle() {
            R::SILENCE
        } else {
            self.compute()
        }
    }

    #[inline]
    pub(crate) fn add_to(&mut self, output: &mut [R::Value]) {
        if self.filtered {
            self.add_with::<true>(output);
        } else {
            self.add_with::<false>(output);
        }
    }

    #[inline(always)]
    fn add_with<const FILTERED: bool>(&mut self, output: &mut [R::Value]) {
        let mut oscillator = self.oscillator.clone();
        let mut envelope = self.envelope.clone();
        let mut filter = self.filter.clone();
        let gain = self.gain;

        for value in output {
            if envelope.is_idle() {
                break;
            }

            let amplitude = (envelope.next_level() * gain) >> 15;
            let sample = oscillator.next_sample();
            let voice = if FILTERED {
                filter.process_scaled(R::from_oscillator(sample), amplitude)
            } else {
                R::amplify_oscillator(sample, amplitude)
            };

            *value = R::add(*value, voice);
        }

        self.oscillator = oscillator;
        self.envelope = envelope;
        self.filter = filter;
    }
}
