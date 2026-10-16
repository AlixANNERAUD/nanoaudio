/// Attack, decay, sustain and release settings of an [`Envelope`].
///
/// Times are in samples, so they are exact at any sample rate. Each segment is a straight line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Adsr {
    /// Samples to rise from the current level to full scale, the last of them at full scale.
    pub attack: u32,
    /// Samples to fall from full scale to the sustain level, the last of them at the sustain level.
    pub decay: u32,
    /// Level held until the release, where `u16::MAX` is full scale.
    pub sustain: u16,
    /// Samples to fall from the current level to silence, the last of them silent.
    pub release: u32,
}

impl Adsr {
    /// An envelope that starts and stops at once and holds full scale in between.
    pub const GATE: Self = Self::new(0, 0, u16::MAX, 0);

    /// Creates settings from times in samples.
    pub const fn new(attack: u32, decay: u32, sustain: u16, release: u32) -> Self {
        Self {
            attack,
            decay,
            sustain,
            release,
        }
    }

    /// Creates settings from times in milliseconds, rounded to the nearest sample at `sample_rate`.
    pub const fn from_millis(
        attack: u32,
        decay: u32,
        sustain: u16,
        release: u32,
        sample_rate: u32,
    ) -> Self {
        Self::new(
            samples(attack, sample_rate),
            samples(decay, sample_rate),
            sustain,
            samples(release, sample_rate),
        )
    }
}

const fn samples(milliseconds: u32, sample_rate: u32) -> u32 {
    let samples = (milliseconds as u64 * sample_rate as u64 + 500) / 1000;

    if samples > u32::MAX as u64 {
        u32::MAX
    } else {
        samples as u32
    }
}

/// The segment an [`Envelope`] is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Silent, until the next [`Envelope::trigger`].
    #[default]
    Idle,
    /// Rising to full scale.
    Attack,
    /// Falling to the sustain level.
    Decay,
    /// Holding the sustain level.
    Sustain,
    /// Falling to silence.
    Release,
}

const FULL: u32 = u32::MAX;

/// A linear ADSR envelope, advanced one sample at a time.
///
/// Every stage change happens on an exact sample. Levels are integers, so it gives the same result in [`Fixed`](nanoaudio_dsp::Fixed) and [`Float`](nanoaudio_dsp::Float), and divisions only happen in [`trigger`](Self::trigger) and [`release`](Self::release).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    settings: Adsr,
    stage: Stage,
    level: u32,
    step: u32,
    decay_step: u32,
    remaining: u32,
}

impl Default for Envelope {
    fn default() -> Self {
        Self::new(Adsr::GATE)
    }
}

impl Envelope {
    /// Creates an idle envelope.
    pub const fn new(settings: Adsr) -> Self {
        Self {
            settings,
            stage: Stage::Idle,
            level: 0,
            step: 0,
            decay_step: 0,
            remaining: 0,
        }
    }

    /// Returns the settings.
    pub fn settings(&self) -> Adsr {
        self.settings
    }

    /// Changes the settings, which apply from the next [`trigger`](Self::trigger) or [`release`](Self::release).
    pub fn set_settings(&mut self, settings: Adsr) {
        self.settings = settings;
    }

    /// Returns the current stage.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Returns `true` once the envelope is idle and silent.
    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// Returns the current level, where `u16::MAX` is full scale.
    pub fn level(&self) -> u16 {
        (self.level >> 16) as u16
    }

    fn sustain_level(&self) -> u32 {
        self.settings.sustain as u32 * 0x1_0001
    }

    /// Starts the attack from the current level, which avoids a click when a playing voice is retriggered.
    pub fn trigger(&mut self) {
        let sustain = self.sustain_level();
        let decay = self.settings.decay;
        self.decay_step = if decay == 0 {
            0
        } else {
            (FULL - sustain) / decay
        };

        match self.settings.attack {
            0 => {
                self.level = FULL;
                self.enter_decay();
            }
            attack => {
                self.stage = Stage::Attack;
                self.remaining = attack;
                self.step = (FULL - self.level) / attack;
            }
        }
    }

    /// Starts the release from the current level. Does nothing if the envelope is idle.
    pub fn release(&mut self) {
        if self.stage == Stage::Idle {
            return;
        }

        match self.settings.release {
            0 => self.stop(),
            release => {
                self.stage = Stage::Release;
                self.remaining = release;
                self.step = self.level / release;
            }
        }
    }

    /// Silences the envelope at once.
    pub fn stop(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0;
    }

    fn enter_decay(&mut self) {
        if self.settings.decay == 0 {
            self.enter_sustain();
        } else {
            self.stage = Stage::Decay;
            self.remaining = self.settings.decay;
            self.step = self.decay_step;
        }
    }

    fn enter_sustain(&mut self) {
        self.level = self.sustain_level();
        self.stage = if self.level == 0 {
            Stage::Idle
        } else {
            Stage::Sustain
        };
    }

    /// Advances one sample and returns the level, where 32 767 is full scale.
    #[inline]
    pub fn next_level(&mut self) -> i32 {
        match self.stage {
            Stage::Idle | Stage::Sustain => {}
            Stage::Attack => {
                self.level += self.step;
                self.remaining -= 1;

                if self.remaining == 0 {
                    self.level = FULL;
                    self.enter_decay();

                    return (FULL >> 17) as i32;
                }
            }
            Stage::Decay => {
                self.level -= self.step;
                self.remaining -= 1;

                if self.remaining == 0 {
                    self.enter_sustain();
                }
            }
            Stage::Release => {
                self.level -= self.step;
                self.remaining -= 1;

                if self.remaining == 0 {
                    self.stop();
                }
            }
        }

        (self.level >> 17) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;

    use std::vec::Vec;

    fn run(envelope: &mut Envelope, samples: usize) -> Vec<i32> {
        (0..samples).map(|_| envelope.next_level()).collect()
    }

    #[test]
    fn stages_last_exactly_their_samples() {
        let mut envelope = Envelope::new(Adsr::new(10, 20, 0x8000, 30));
        envelope.trigger();

        let attack = run(&mut envelope, 10);
        assert!(attack.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(attack[0], 3_276);
        assert_eq!(attack[9], 32_767);
        assert_eq!(envelope.stage(), Stage::Decay);

        let decay = run(&mut envelope, 20);
        assert!(decay.windows(2).all(|pair| pair[0] > pair[1]));
        assert!(decay[0] < 32_767);
        assert_eq!(decay[19], 16_384);
        assert_eq!(envelope.stage(), Stage::Sustain);

        assert!(run(&mut envelope, 1000).iter().all(|level| *level == 16_384));

        envelope.release();
        let release = run(&mut envelope, 30);
        assert!(release.windows(2).all(|pair| pair[0] > pair[1]));
        assert_eq!(release[28], 546);
        assert_eq!(release[29], 0);
        assert!(envelope.is_idle());
        assert_eq!(run(&mut envelope, 5), [0; 5]);
    }

    #[test]
    fn long_segments_are_exact() {
        let samples = if cfg!(miri) { 1_000 } else { 480_000 };
        let mut envelope = Envelope::new(Adsr::new(samples, samples, 1, samples));
        envelope.trigger();

        let attack = run(&mut envelope, samples as usize);
        assert!(attack[samples as usize - 2] < 32_767);
        assert_eq!(attack[samples as usize - 1], 32_767);
        assert_eq!(envelope.stage(), Stage::Decay);

        run(&mut envelope, samples as usize - 1);
        assert_eq!(envelope.stage(), Stage::Decay);
        envelope.next_level();
        assert_eq!(envelope.stage(), Stage::Sustain);
        assert_eq!(envelope.level(), 1);

        envelope.release();
        run(&mut envelope, samples as usize - 1);
        assert_eq!(envelope.stage(), Stage::Release);
        assert_eq!(envelope.next_level(), 0);
        assert!(envelope.is_idle());
    }

    #[test]
    fn zero_length_segments_are_skipped() {
        let mut envelope = Envelope::new(Adsr::new(0, 0, 0x4000, 0));
        envelope.trigger();
        assert_eq!(envelope.stage(), Stage::Sustain);
        assert_eq!(envelope.next_level(), 8_192);

        envelope.release();
        assert!(envelope.is_idle());
        assert_eq!(envelope.next_level(), 0);

        let mut envelope = Envelope::new(Adsr::new(4, 0, 0x4000, 0));
        envelope.trigger();
        assert_eq!(run(&mut envelope, 5), [8_191, 16_383, 24_575, 32_767, 8_192]);
    }

    #[test]
    fn percussive_envelopes_end_after_the_decay() {
        let mut envelope = Envelope::new(Adsr::new(2, 3, 0, 100));
        envelope.trigger();

        assert_eq!(run(&mut envelope, 5)[4], 0);
        assert!(envelope.is_idle());
    }

    #[test]
    fn release_starts_from_the_current_level() {
        let mut envelope = Envelope::new(Adsr::new(100, 0, u16::MAX, 10));
        envelope.trigger();
        let level = run(&mut envelope, 50)[49];

        envelope.release();
        let release = run(&mut envelope, 10);
        assert!(release[0] < level);
        assert_eq!(release[9], 0);
        assert!(envelope.is_idle());

        envelope.release();
        assert!(envelope.is_idle());
    }

    #[test]
    fn retrigger_starts_from_the_current_level() {
        let mut envelope = Envelope::new(Adsr::new(10, 0, u16::MAX, 10));
        envelope.trigger();
        let level = run(&mut envelope, 5)[4];

        envelope.trigger();
        let attack = run(&mut envelope, 10);
        assert!(attack[0] > level);
        assert_eq!(attack[9], 32_767);
    }

    #[test]
    fn milliseconds_round_to_samples() {
        assert_eq!(
            Adsr::from_millis(10, 1, 7, 1000, 44_100),
            Adsr::new(441, 44, 7, 44_100)
        );
        assert_eq!(Adsr::from_millis(u32::MAX, 0, 0, 0, u32::MAX).attack, u32::MAX);
    }
}
