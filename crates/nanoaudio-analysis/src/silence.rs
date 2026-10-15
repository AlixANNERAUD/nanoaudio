use nanoaudio_dsp::{Analyzer, RepresentationKind};

use crate::{Measure, math};

/// Tells when the signal goes silent and when the sound comes back.
///
/// A frame is quiet when every channel is at or below the threshold. The signal becomes silent after a run of quiet frames as long as the silence duration, and sound again after a run of louder frames as long as the sound duration, so short pauses and short clicks do not toggle the state. Positions are counted in frames analyzed since creation or the last [`reset`](Self::reset).
///
/// It starts silent, as nothing has been heard yet. In [`Float`](nanoaudio_dsp::Float), NaN and infinite values count as sound.
///
/// ```
/// use nanoaudio_analysis::{SilenceDetector, analyze_using};
/// use nanoaudio_dsp::Float;
///
/// let mut detector = SilenceDetector::<Float>::new(-60.0, 1000, 10, 5);
/// let mut samples = [0i16; 40];
/// samples[10..20].fill(8000);
///
/// analyze_using::<Float, _, _>(&samples, 1, &mut detector).unwrap();
///
/// assert!(detector.is_silent());
/// assert_eq!(detector.transition(), 20);
/// assert_eq!(detector.silent_run(), 20);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilenceDetector<R: Measure> {
    threshold: R::Value,
    silence: u32,
    sound: u32,
    silent: bool,
    pending: u32,
    frames: u64,
    transition: u64,
}

impl<R: Measure> SilenceDetector<R> {
    /// Creates a detector with a threshold in dBFS and durations in milliseconds.
    ///
    /// Durations are rounded to the nearest frame at `sample_rate` Hz, with at least one frame. A threshold at or above 0 dBFS makes every frame within full scale quiet, and negative infinity or NaN makes only exact digital silence quiet.
    pub fn new(threshold: f32, sample_rate: u32, silence: u32, sound: u32) -> Self {
        Self::with_frames(
            linear::<R>(threshold),
            frames(silence, sample_rate),
            frames(sound, sample_rate),
        )
    }

    /// Creates a detector with a linear threshold in the representation and durations in frames.
    ///
    /// It does not use floating point with [`Fixed`](nanoaudio_dsp::Fixed). A duration of 0 acts as 1.
    pub const fn with_frames(threshold: R::Value, silence: u32, sound: u32) -> Self {
        Self {
            threshold,
            silence: if silence == 0 { 1 } else { silence },
            sound: if sound == 0 { 1 } else { sound },
            silent: true,
            pending: 0,
            frames: 0,
            transition: 0,
        }
    }

    /// Returns the linear threshold, in the representation.
    pub const fn threshold(&self) -> R::Value {
        self.threshold
    }

    /// Returns whether the signal is silent.
    pub const fn is_silent(&self) -> bool {
        self.silent
    }

    /// Returns the number of frames analyzed.
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// Returns the position of the first frame of the current state, in frames.
    ///
    /// It is where the silence or the sound started, not where it was confirmed: when the state is silent, the audio from here on can be trimmed. It is 0 before the first change.
    pub const fn transition(&self) -> u64 {
        self.transition
    }

    /// Returns the length of the current silent run, in frames.
    ///
    /// When silent, it counts the frames since [`transition`](Self::transition). When not, it counts the quiet frames at the end that are not yet long enough to be silence, so trailing silence can still be trimmed when the input ends.
    pub const fn silent_run(&self) -> u64 {
        if self.silent {
            self.frames - self.transition
        } else {
            self.pending as u64
        }
    }

    /// Goes back to the silent state at position 0, keeping the threshold and durations.
    pub fn reset(&mut self) {
        *self = Self::with_frames(self.threshold, self.silence, self.sound);
    }
}

impl<R: Measure> Analyzer<R> for SilenceDetector<R> {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        let quiet = frame.iter().fold(true, |quiet, value| {
            quiet & (R::magnitude(*value) <= self.threshold)
        });

        self.frames += 1;

        if quiet == self.silent {
            self.pending = 0;
            return;
        }

        self.pending += 1;

        let duration = if quiet { self.silence } else { self.sound };

        if self.pending >= duration {
            self.silent = quiet;
            self.transition = self.frames - self.pending as u64;
            self.pending = 0;
        }
    }
}

fn frames(milliseconds: u32, sample_rate: u32) -> u32 {
    let frames = (milliseconds as u64 * sample_rate as u64 + 500) / 1000;

    frames.clamp(1, u32::MAX as u64) as u32
}

fn linear<R: Measure>(decibels: f32) -> R::Value {
    if decibels >= 0.0 {
        return R::FULL_SCALE;
    }

    if decibels.is_nan() || decibels < -900.0 {
        return R::SILENCE;
    }

    let value = math::exp2(decibels as f64 * core::f64::consts::LOG2_10 / 20.0);

    match R::KIND {
        RepresentationKind::Fixed => R::from_sample((value * 2147483648.0 + 0.5) as i32),
        RepresentationKind::Float => R::from_sample(value as f32),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use nanoaudio_core::Format;
    use nanoaudio_dsp::{Fixed, Float, Stream, analyze_bytes_using, analyze_using, samples_using};

    use super::*;

    const RATE: u32 = 8000;

    fn state<R: Measure>(detector: &SilenceDetector<R>) -> (bool, u64, u64, u64) {
        (
            detector.is_silent(),
            detector.transition(),
            detector.silent_run(),
            detector.frames(),
        )
    }

    fn sine(index: usize, amplitude: f64) -> i16 {
        (amplitude * 32767.0 * (index as f64 * 0.37 + 1.0).sin()).round() as i16
    }

    fn reference(
        signal: &[i16],
        channels: usize,
        threshold: i16,
        silence: u32,
        sound: u32,
    ) -> (bool, u64, u64) {
        let mut silent = true;
        let mut transition = 0u64;
        let mut run = 0u64;

        for (index, frame) in signal.chunks_exact(channels).enumerate() {
            let quiet = frame
                .iter()
                .all(|sample| sample.unsigned_abs() <= threshold as u16);

            if quiet == silent {
                run = 0;
                continue;
            }

            run += 1;

            let duration = if quiet { silence } else { sound };

            if run >= duration as u64 {
                silent = quiet;
                transition = index as u64 + 1 - run;
                run = 0;
            }
        }

        let frames = (signal.len() / channels) as u64;
        let silent_run = if silent { frames - transition } else { run };

        (silent, transition, silent_run)
    }

    fn check<R: Measure>(signal: &[i16], channels: u8, threshold: i16, silence: u32, sound: u32) {
        let expected = reference(signal, channels as usize, threshold, silence, sound);
        let mut tapped =
            SilenceDetector::<R>::with_frames(R::from_sample(threshold), silence, sound);
        let mut output = [0i16; 4096];

        let frames = samples_using::<R, _>(signal, channels)
            .unwrap()
            .tap(&mut tapped)
            .write_samples(&mut output[..signal.len()]);

        assert_eq!(frames as u64, tapped.frames());
        assert_eq!(&output[..signal.len()], signal);
        assert_eq!(
            (tapped.is_silent(), tapped.transition(), tapped.silent_run()),
            expected
        );

        let mut sliced =
            SilenceDetector::<R>::with_frames(R::from_sample(threshold), silence, sound);

        assert_eq!(
            analyze_using::<R, _, _>(signal, channels, &mut sliced),
            Ok(frames)
        );
        assert_eq!(state(&sliced), state(&tapped));

        let mut bytes = [0u8; 8192];

        for (sample, chunk) in signal.iter().zip(bytes.as_chunks_mut::<2>().0) {
            *chunk = sample.to_le_bytes();
        }

        let mut decoded =
            SilenceDetector::<R>::with_frames(R::from_sample(threshold), silence, sound);

        assert_eq!(
            analyze_bytes_using::<R, _>(
                &bytes[..2 * signal.len()],
                Format::I16,
                channels,
                &mut decoded
            ),
            Ok(frames)
        );
        assert_eq!(state(&decoded), state(&tapped));
    }

    fn follows_the_reference<R: Measure>() {
        const DURATIONS: &[(u32, u32)] = if cfg!(miri) {
            &[(100, 5)]
        } else {
            &[(1, 1), (100, 5), (200, 20), (50, 50), (0, 0), (5000, 1)]
        };
        const THRESHOLDS: &[i16] = if cfg!(miri) {
            &[30]
        } else {
            &[0, 30, 1000, i16::MAX]
        };

        let mut signal = [0i16; 4000];

        for (index, sample) in signal.iter_mut().enumerate() {
            let amplitude = match index {
                0..300 => 0.0,
                300..1200 => 0.5,
                1200..1210 => 0.0,
                1210..1700 => 0.4,
                1700..2600 => 0.0005,
                2600..2604 => 0.9,
                2604..3100 => 0.0,
                3100..3700 => 0.2,
                _ => 0.0,
            };
            *sample = sine(index, amplitude);
        }

        for channels in [1u8, 2, 3] {
            for (silence, sound) in DURATIONS {
                for threshold in THRESHOLDS {
                    check::<R>(&signal, channels, *threshold, *silence, *sound);
                }
            }
        }

        let mut stereo = [0i16; 2000];

        for (index, sample) in stereo.iter_mut().enumerate() {
            let frame = index / 2;
            let loud = if index % 2 == 0 { 400..900 } else { 1100..1500 };
            *sample = if loud.contains(&frame) {
                sine(frame, 0.3)
            } else {
                3
            };
        }

        check::<R>(&stereo, 2, 16, 100, 10);
    }

    fn trims_a_recording<R: Measure>() {
        let mut signal = [0i16; 3000];
        signal[1000..2000]
            .iter_mut()
            .enumerate()
            .for_each(|(index, sample)| *sample = sine(index, 0.5));
        let mut detector = SilenceDetector::<R>::new(-40.0, RATE, 50, 2);

        analyze_using::<R, _, _>(&signal[..1500], 1, &mut detector).unwrap();

        assert!(!detector.is_silent());
        assert_eq!(detector.transition(), 1000);
        assert_eq!(detector.silent_run(), 0);

        analyze_using::<R, _, _>(&signal[1500..2300], 1, &mut detector).unwrap();

        assert!(!detector.is_silent());
        assert_eq!(detector.silent_run(), 300);

        analyze_using::<R, _, _>(&signal[2300..], 1, &mut detector).unwrap();

        assert!(detector.is_silent());
        assert_eq!(detector.frames(), 3000);
        assert_eq!(detector.silent_run(), 1000);
        assert!(detector.transition().abs_diff(2000) <= 2);

        detector.reset();

        assert!(detector.threshold() == SilenceDetector::<R>::new(-40.0, RATE, 50, 2).threshold());
        assert_eq!((detector.silence, detector.sound), (400, 16));
        assert!(detector.is_silent());
        assert_eq!(
            (
                detector.frames(),
                detector.transition(),
                detector.silent_run()
            ),
            (0, 0, 0)
        );
    }

    fn handles_edges<R: Measure>() {
        let mut detector = SilenceDetector::<R>::new(-60.0, 1000, 1, 1);

        assert_eq!(
            analyze_using::<R, _, _>(&[0i16; 0], 2, &mut detector),
            Ok(0)
        );
        assert!(detector.is_silent());
        assert_eq!(detector.silent_run(), 0);

        detector.analyze(&[]);

        assert!(detector.is_silent());
        assert_eq!(detector.silent_run(), 1);

        detector.analyze(&[R::from_sample(i32::MIN)]);

        assert!(!detector.is_silent());
        assert_eq!(detector.transition(), 1);

        let mut everything = SilenceDetector::<R>::new(6.0, 1000, 1, 1);

        everything.analyze(&[R::from_sample(i32::MAX), R::from_sample(-i32::MAX)]);
        assert!(everything.is_silent());

        everything.analyze(&[R::from_sample(i32::MIN)]);
        assert_eq!(
            everything.is_silent(),
            R::magnitude(R::from_sample(i32::MIN)) <= R::FULL_SCALE
        );

        for threshold in [f32::NEG_INFINITY, f32::NAN, -1000.0] {
            let mut digital = SilenceDetector::<R>::new(threshold, 1000, 1, 1);

            assert!(digital.threshold() == R::SILENCE);

            digital.analyze(&[R::from_sample(1i32)]);
            assert!(!digital.is_silent());

            digital.analyze(&[R::SILENCE, R::SILENCE]);
            assert!(digital.is_silent());
        }

        let mut long =
            SilenceDetector::<R>::with_frames(R::from_sample(100i16), u32::MAX, u32::MAX);

        long.frames = u64::MAX - 10;
        long.transition = u64::MAX - 20;
        long.silent = false;
        long.pending = u32::MAX - 2;

        for _ in 0..2 {
            long.analyze(&[R::SILENCE]);
        }

        assert!(long.is_silent());
        assert_eq!(long.transition(), u64::MAX - 8 - u32::MAX as u64);
        assert_eq!(long.silent_run(), u32::MAX as u64);
    }

    #[test]
    fn fixed() {
        follows_the_reference::<Fixed>();
        trims_a_recording::<Fixed>();
        handles_edges::<Fixed>();
    }

    #[test]
    fn float() {
        follows_the_reference::<Float>();
        trims_a_recording::<Float>();
        handles_edges::<Float>();
    }

    #[test]
    fn converts_the_threshold() {
        for decibels in [
            -0.0001f32, -0.1, -1.0, -6.0, -20.0, -40.0, -60.0, -90.0, -120.0, -150.0, -186.0,
            -190.0,
        ] {
            let expected = (10f64.powf(decibels as f64 / 20.0) * 2147483648.0).round() as i64;
            let fixed = SilenceDetector::<Fixed>::new(decibels, RATE, 1, 1).threshold();

            assert_eq!(fixed as i64, expected.min(i32::MAX as i64), "{decibels}");

            let float = SilenceDetector::<Float>::new(decibels, RATE, 1, 1).threshold();

            assert_eq!(
                float,
                10f64.powf(decibels as f64 / 20.0) as f32,
                "{decibels}"
            );
        }

        assert_eq!(
            SilenceDetector::<Fixed>::new(0.0, RATE, 1, 1).threshold(),
            i32::MAX
        );
        assert_eq!(
            SilenceDetector::<Float>::new(f32::INFINITY, RATE, 1, 1).threshold(),
            1.0
        );
    }

    #[test]
    fn converts_the_durations() {
        assert_eq!(frames(0, 48000), 1);
        assert_eq!(frames(1, 48000), 48);
        assert_eq!(frames(250, 44100), 11025);
        assert_eq!(frames(1, 44100), 44);
        assert_eq!(frames(3, 500), 2);
        assert_eq!(frames(u32::MAX, u32::MAX), u32::MAX);

        let detector = SilenceDetector::<Fixed>::new(-50.0, 44100, 250, 1);

        assert_eq!((detector.silence, detector.sound), (11025, 44));
    }
}
