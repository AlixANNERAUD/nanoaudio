use core::fmt::Debug;
use core::time::Duration;

use nanoaudio_core::MAXIMUM_CHANNELS;
use nanoaudio_dsp::{Analyzer, Fixed, Float};

use crate::{Level, LevelMeter, Power, math, sealed};

const NEPERS_TO_DECIBELS: f64 = 8.685_889_638_065_035;
const BURST_TO_TIME_CONSTANT: f64 = 0.632_321_591_075_894_3;
const VU_TIME_CONSTANT: Duration = Duration::from_nanos(76_588_531);
const DEFAULT_WINDOW: Duration = Duration::from_millis(1);
const ONE: u64 = 1 << 32;

/// The value a [`Ballistics`] follows on each channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Detector {
    /// The largest magnitude of each window, smoothed as an amplitude.
    Peak,
    /// The mean square of each window, smoothed as a power and read as its square root.
    Rms,
}

/// How a meter reading moves: a one-pole attack and release, and a hold before the release.
///
/// The attack and release are time constants: after a step, the smoothed value covers 63% of the way in one time constant. They apply to the amplitude with [`Detector::Peak`] and to the power with [`Detector::Rms`], where a release time constant falls half as many decibels per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ballistics {
    /// The value that is smoothed.
    pub detector: Detector,
    /// The time constant when the level rises.
    pub attack: Duration,
    /// The time constant when the level falls.
    pub release: Duration,
    /// How long the reading stays at its highest value before it falls.
    pub hold: Duration,
}

impl Ballistics {
    /// A VU meter: RMS with a symmetric response that reaches 99% of a steady sine in 300 ms, as in IEC 60268-17, without overshoot.
    pub const VU: Self = Self::new(Detector::Rms, VU_TIME_CONSTANT, VU_TIME_CONSTANT);

    /// A DIN 45406 peak programme meter, IEC 60268-10 type I: 5 ms integration time, falls 20 dB in 1.5 s.
    pub const PPM_DIN: Self =
        Self::ppm(Duration::from_millis(5), 20.0, Duration::from_millis(1500));

    /// A Nordic N9 peak programme meter, IEC 60268-10 type I: 5 ms integration time, falls 20 dB in 1.7 s.
    pub const PPM_NORDIC: Self =
        Self::ppm(Duration::from_millis(5), 20.0, Duration::from_millis(1700));

    /// An EBU or BBC peak programme meter, IEC 60268-10 type II: 10 ms integration time, falls 24 dB in 2.8 s.
    pub const PPM_EBU: Self =
        Self::ppm(Duration::from_millis(10), 24.0, Duration::from_millis(2800));

    /// Creates ballistics with the given time constants and no hold.
    pub const fn new(detector: Detector, attack: Duration, release: Duration) -> Self {
        Self {
            detector,
            attack,
            release,
            hold: Duration::ZERO,
        }
    }

    /// Creates peak ballistics from an integration time and a fall of `decibels` over `fall`.
    ///
    /// The integration time is the length of a tone burst that reads 2 dB below a steady tone, as in IEC 60268-10. A fall of 0 dB never falls.
    pub const fn ppm(integration: Duration, decibels: f32, fall: Duration) -> Self {
        Self::new(
            Detector::Peak,
            scale(integration, BURST_TO_TIME_CONSTANT),
            scale(fall, NEPERS_TO_DECIBELS / decibels as f64),
        )
    }

    /// Returns the same ballistics with a hold of `hold`.
    pub const fn with_hold(self, hold: Duration) -> Self {
        Self { hold, ..self }
    }
}

const fn scale(duration: Duration, factor: f64) -> Duration {
    Duration::from_nanos((duration.as_nanos() as f64 * factor + 0.5) as u64)
}

fn exponential(exponent: f64) -> f64 {
    if exponent.is_nan() {
        return 0.0;
    }

    math::exp(-exponent.max(0.0))
}

/// A representation whose levels a [`BallisticMeter`] can smooth, [`Fixed`] or [`Float`].
///
/// In [`Fixed`], the smoothed values are kept in 64-bit integers and no floating point is used until a reading in decibels. RMS levels below about -93 dBFS read as silence there.
pub trait Smooth: Power + sealed::Sealed {
    #[doc(hidden)]
    type State: Copy + Debug + PartialEq + PartialOrd;
    #[doc(hidden)]
    type Coefficient: Copy + Debug + PartialEq;
    #[doc(hidden)]
    const EMPTY: Self::State;

    #[doc(hidden)]
    fn coefficient(retained: f64) -> Self::Coefficient;
    #[doc(hidden)]
    fn detect(level: &Level<Self>, detector: Detector) -> Option<Self::State>;
    #[doc(hidden)]
    fn approach(state: &mut Self::State, target: Self::State, coefficient: Self::Coefficient);
    #[doc(hidden)]
    fn linear(state: Self::State, detector: Detector) -> Self::Value;
    #[doc(hidden)]
    fn decibels(state: Self::State, detector: Detector) -> f32;
}

#[inline(always)]
fn fraction(difference: u64, coefficient: u64) -> u64 {
    (difference >> 32) * coefficient + (((difference & (ONE - 1)) * coefficient) >> 32)
}

impl Smooth for Fixed {
    type State = u64;
    type Coefficient = u64;
    const EMPTY: u64 = 0;

    fn coefficient(retained: f64) -> u64 {
        (((1.0 - retained) * ONE as f64 + 0.5) as u64).min(ONE)
    }

    #[inline]
    fn detect(level: &Level<Fixed>, detector: Detector) -> Option<u64> {
        let value = match detector {
            Detector::Peak => level.peak(),
            Detector::Rms => level.mean_square(),
        };

        Some((value.max(0) as u64) << 31)
    }

    #[inline]
    fn approach(state: &mut u64, target: u64, coefficient: u64) {
        if target >= *state {
            *state += fraction(target - *state, coefficient);
        } else {
            *state -= fraction(*state - target, coefficient);
        }
    }

    fn linear(state: u64, detector: Detector) -> i32 {
        let value = match detector {
            Detector::Peak => (state + (1 << 30)) >> 31,
            Detector::Rms => state.isqrt(),
        };

        value.min(i32::MAX as u64) as i32
    }

    fn decibels(state: u64, detector: Detector) -> f32 {
        Self::amplitude_decibels(Self::linear(state, detector))
    }
}

impl Smooth for Float {
    type State = f32;
    type Coefficient = f32;
    const EMPTY: f32 = 0.0;

    fn coefficient(retained: f64) -> f32 {
        (1.0 - retained) as f32
    }

    #[inline]
    fn detect(level: &Level<Float>, detector: Detector) -> Option<f32> {
        let value = match detector {
            Detector::Peak => level.peak(),
            Detector::Rms => level.mean_square(),
        };

        if value.is_nan() {
            None
        } else {
            Some(value.min(f32::MAX))
        }
    }

    #[inline]
    fn approach(state: &mut f32, target: f32, coefficient: f32) {
        *state += (target - *state) * coefficient;
    }

    fn linear(state: f32, detector: Detector) -> f32 {
        match detector {
            Detector::Peak => state,
            Detector::Rms => math::square_root_f32(state),
        }
    }

    fn decibels(state: f32, detector: Detector) -> f32 {
        match detector {
            Detector::Peak => Self::amplitude_decibels(state),
            Detector::Rms => 0.5 * Self::amplitude_decibels(state),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    ballistics: Ballistics,
    sample_rate: u32,
    window: Duration,
    hold: Duration,
    fall: f32,
}

/// A meter whose readings follow [`Ballistics`], with a peak hold marker, on each channel.
///
/// A [`LevelMeter`] measures short windows, 1 ms by default, and each completed window moves the readings. The peak hold marker follows the highest reading, stays there for its hold time and then falls at a fixed rate in decibels per second. Without [`with_peak_hold`](Self::with_peak_hold), it is the reading itself.
///
/// Samples in [`Fixed`] are processed without floating point, and so are the readings in [`level`](Self::level) and [`peak_hold`](Self::peak_hold). A NaN window in [`Float`] leaves the readings unchanged, and infinities count as [`f32::MAX`].
///
/// ```
/// use nanoaudio_analysis::{BallisticMeter, Ballistics, analyze_using};
/// use nanoaudio_dsp::Float;
///
/// let mut meter = BallisticMeter::<Float>::new(Ballistics::VU, 48_000);
/// let sine: [f32; 14_400] = core::array::from_fn(|index| {
///     let phase = core::f32::consts::TAU * (index % 48) as f32 / 48.0;
///     phase.sin()
/// });
///
/// analyze_using::<Float, _, _>(&sine, 1, &mut meter).unwrap();
///
/// assert!((meter.level(0) / core::f32::consts::FRAC_1_SQRT_2 - 0.99).abs() < 1e-3);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct BallisticMeter<R: Smooth, const CHANNELS: usize = MAXIMUM_CHANNELS> {
    meter: LevelMeter<R, CHANNELS>,
    settings: Settings,
    attack: R::Coefficient,
    release: R::Coefficient,
    hold: u32,
    marker_release: R::Coefficient,
    marker_hold: u32,
    values: [R::State; CHANNELS],
    holds: [u32; CHANNELS],
    markers: [R::State; CHANNELS],
    marker_holds: [u32; CHANNELS],
}

impl<R: Smooth, const CHANNELS: usize> BallisticMeter<R, CHANNELS> {
    /// Creates a meter with `ballistics` at `sample_rate` Hz.
    pub fn new(ballistics: Ballistics, sample_rate: u32) -> Self {
        Self::build(Settings {
            ballistics,
            sample_rate,
            window: DEFAULT_WINDOW,
            hold: Duration::ZERO,
            fall: f32::INFINITY,
        })
    }

    /// Returns the same meter, cleared, with a peak hold marker that stays `hold` at its highest value, then falls `decibels_per_second`.
    ///
    /// A fall of 0 keeps the marker until [`reset`](Self::reset).
    pub fn with_peak_hold(self, hold: Duration, decibels_per_second: f32) -> Self {
        Self::build(Settings {
            hold,
            fall: decibels_per_second,
            ..self.settings
        })
    }

    /// Returns the same meter, cleared, measuring windows of `window` instead of 1 ms.
    ///
    /// The readings move once per window, so a tone burst can read up to one window longer than it is. Shorter windows follow bursts more closely and cost more.
    pub fn with_window(self, window: Duration) -> Self {
        Self::build(Settings {
            window,
            ..self.settings
        })
    }

    fn build(settings: Settings) -> Self {
        let meter = LevelMeter::with_duration(settings.window, settings.sample_rate);
        let window = meter.window() as f64 / settings.sample_rate as f64;
        let smoothing = |time: Duration| R::coefficient(exponential(window / time.as_secs_f64()));
        let windows = |time: Duration| (time.as_secs_f64() / window + 0.5) as u32;
        let fall = if settings.fall > 0.0 {
            settings.fall as f64
        } else {
            0.0
        };
        let domain = match settings.ballistics.detector {
            Detector::Peak => 1.0,
            Detector::Rms => 2.0,
        };

        Self {
            meter,
            settings,
            attack: smoothing(settings.ballistics.attack),
            release: smoothing(settings.ballistics.release),
            hold: windows(settings.ballistics.hold),
            marker_release: R::coefficient(exponential(
                fall * window * domain / NEPERS_TO_DECIBELS,
            )),
            marker_hold: windows(settings.hold),
            values: [R::EMPTY; CHANNELS],
            holds: [0; CHANNELS],
            markers: [R::EMPTY; CHANNELS],
            marker_holds: [0; CHANNELS],
        }
    }

    /// Returns the ballistics of the readings.
    pub const fn ballistics(&self) -> Ballistics {
        self.settings.ballistics
    }

    /// Returns the level meter that measures the windows.
    pub const fn meter(&self) -> &LevelMeter<R, CHANNELS> {
        &self.meter
    }

    /// Returns the reading of `channel` as an amplitude, 0 dBFS at full scale, or silence for a channel out of range.
    pub fn level(&self, channel: usize) -> R::Value {
        match self.values.get(channel) {
            Some(value) => R::linear(*value, self.settings.ballistics.detector),
            None => R::SILENCE,
        }
    }

    /// Returns the reading of `channel` in dBFS, negative infinity for silence.
    pub fn decibels(&self, channel: usize) -> f32 {
        match self.values.get(channel) {
            Some(value) => R::decibels(*value, self.settings.ballistics.detector),
            None => f32::NEG_INFINITY,
        }
    }

    /// Returns the peak hold marker of `channel` as an amplitude, or silence for a channel out of range.
    pub fn peak_hold(&self, channel: usize) -> R::Value {
        match self.markers.get(channel) {
            Some(marker) => R::linear(*marker, self.settings.ballistics.detector),
            None => R::SILENCE,
        }
    }

    /// Returns the peak hold marker of `channel` in dBFS, negative infinity for silence.
    pub fn peak_hold_decibels(&self, channel: usize) -> f32 {
        match self.markers.get(channel) {
            Some(marker) => R::decibels(*marker, self.settings.ballistics.detector),
            None => f32::NEG_INFINITY,
        }
    }

    /// Measures one interleaved frame, and returns `true` when it completes a window and moves the readings.
    #[inline]
    pub fn measure(&mut self, frame: &[R::Value]) -> bool {
        if !self.meter.measure(frame) {
            return false;
        }

        self.update();
        true
    }

    #[inline(never)]
    fn update(&mut self) {
        let detector = self.settings.ballistics.detector;

        for (channel, (((value, hold), marker), marker_hold)) in self
            .values
            .iter_mut()
            .zip(self.holds.iter_mut())
            .zip(self.markers.iter_mut())
            .zip(self.marker_holds.iter_mut())
            .enumerate()
        {
            let Some(target) = R::detect(&self.meter.level(channel), detector) else {
                continue;
            };

            if target >= *value {
                R::approach(value, target, self.attack);
                *hold = self.hold;
            } else if *hold > 0 {
                *hold -= 1;
            } else {
                R::approach(value, target, self.release);
            }

            if *value >= *marker {
                *marker = *value;
                *marker_hold = self.marker_hold;
            } else if *marker_hold > 0 {
                *marker_hold -= 1;
            } else {
                R::approach(marker, *value, self.marker_release);
            }
        }
    }

    /// Clears the readings, the markers and the window in progress, and keeps the settings.
    pub fn reset(&mut self) {
        *self = Self::build(self.settings);
    }
}

impl<R: Smooth, const CHANNELS: usize> Analyzer<R> for BallisticMeter<R, CHANNELS> {
    #[inline]
    fn analyze(&mut self, frame: &[R::Value]) {
        self.measure(frame);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use nanoaudio_core::Format;
    use nanoaudio_dsp::{Stream, analyze_bytes_using, analyze_using, samples_using};

    use super::*;

    const RATE: u32 = 48_000;
    const WINDOW: usize = 48;

    fn linear<R: Smooth>(value: R::Value) -> f64 {
        R::to_sample::<i32>(value) as f64 / 2_147_483_648.0
    }

    fn tone(amplitude: f64, period: f64, frames: usize) -> Vec<i32> {
        (0..frames)
            .map(|frame| {
                let phase = core::f64::consts::TAU * frame as f64 / period;
                (amplitude * 2_147_483_647.0 * phase.cos()).round() as i32
            })
            .collect()
    }

    fn feed<R: Smooth>(meter: &mut BallisticMeter<R, 1>, samples: &[i32]) -> Vec<f64> {
        samples
            .chunks(WINDOW)
            .map(|window| {
                analyze_using::<R, _, _>(window, 1, &mut *meter).unwrap();
                linear::<R>(meter.level(0))
            })
            .collect()
    }

    fn decibels(value: f64) -> f64 {
        20.0 * value.log10()
    }

    struct Model {
        detector: Detector,
        attack: f64,
        release: f64,
        hold: u32,
        marker_release: f64,
        marker_hold: u32,
        value: f64,
        held: u32,
        marker: f64,
        marker_held: u32,
    }

    impl Model {
        fn new(ballistics: Ballistics, hold: Duration, fall: f64) -> Self {
            let window = WINDOW as f64 / RATE as f64;
            let smoothing = |time: Duration| 1.0 - (-window / time.as_secs_f64()).exp();
            let domain = if ballistics.detector == Detector::Peak {
                20.0
            } else {
                10.0
            };
            Self {
                detector: ballistics.detector,
                attack: smoothing(ballistics.attack),
                release: smoothing(ballistics.release),
                hold: (ballistics.hold.as_secs_f64() / window).round() as u32,
                marker_release: 1.0 - 10f64.powf(-fall * window / domain),
                marker_hold: (hold.as_secs_f64() / window).round() as u32,
                value: 0.0,
                held: 0,
                marker: 0.0,
                marker_held: 0,
            }
        }

        fn window(&mut self, samples: &[f64]) {
            let target = match self.detector {
                Detector::Peak => samples
                    .iter()
                    .fold(0.0, |peak, value| value.abs().max(peak)),
                Detector::Rms => {
                    samples.iter().map(|value| value * value).sum::<f64>() / samples.len() as f64
                }
            };

            if target >= self.value {
                self.value += (target - self.value) * self.attack;
                self.held = self.hold;
            } else if self.held > 0 {
                self.held -= 1;
            } else {
                self.value += (target - self.value) * self.release;
            }

            if self.value >= self.marker {
                self.marker = self.value;
                self.marker_held = self.marker_hold;
            } else if self.marker_held > 0 {
                self.marker_held -= 1;
            } else {
                self.marker += (self.value - self.marker) * self.marker_release;
            }
        }

        fn read(&self, value: f64) -> f64 {
            match self.detector {
                Detector::Peak => value,
                Detector::Rms => value.sqrt(),
            }
        }
    }

    fn signal(channels: usize, frames: usize) -> Vec<i16> {
        (0..frames * channels)
            .map(|index| {
                let (frame, channel) = (index / channels, index % channels);
                let envelope = [0.9, 0.0, 0.3, 0.05, 0.7, 0.0][(frame / 1500 + channel) % 6];
                let phase = frame as f64 * (channel as f64 + 1.0) * 0.071;
                (envelope * 32_767.0 * phase.sin()).round() as i16
            })
            .collect()
    }

    fn matches_model<R: Smooth>() {
        let settings: [(Ballistics, Duration, f64); 4] = [
            (Ballistics::VU, Duration::ZERO, f64::INFINITY),
            (Ballistics::PPM_NORDIC, Duration::from_millis(30), 20.0),
            (
                Ballistics::new(
                    Detector::Rms,
                    Duration::from_millis(3),
                    Duration::from_millis(40),
                )
                .with_hold(Duration::from_millis(12)),
                Duration::from_millis(5),
                0.0,
            ),
            (
                Ballistics::new(Detector::Peak, Duration::ZERO, Duration::from_millis(20))
                    .with_hold(Duration::from_millis(7)),
                Duration::from_millis(50),
                60.0,
            ),
        ];
        let frames = if cfg!(miri) { 2_016 } else { 12_000 };

        for channels in [1, 2] {
            let input = signal(channels, frames);

            for (ballistics, hold, fall) in settings {
                let mut meter =
                    BallisticMeter::<R, 2>::new(ballistics, RATE).with_peak_hold(hold, fall as f32);
                let mut models: Vec<Model> = (0..channels)
                    .map(|_| Model::new(ballistics, hold, fall))
                    .collect();

                for (index, block) in input.chunks(WINDOW * channels).enumerate() {
                    for (channel, model) in models.iter_mut().enumerate() {
                        let samples: Vec<f64> = block
                            .iter()
                            .skip(channel)
                            .step_by(channels)
                            .map(|sample| *sample as f64 / 32_768.0)
                            .collect();
                        model.window(&samples);
                    }

                    analyze_using::<R, _, _>(block, channels as u8, &mut meter).unwrap();
                    assert_eq!(meter.meter().windows(), index as u64 + 1);

                    for (channel, model) in models.iter().enumerate() {
                        let value = model.read(model.value);
                        let marker = model.read(model.marker);
                        assert!(
                            (linear::<R>(meter.level(channel)) - value).abs() < 1e-6 + value * 1e-5
                        );
                        assert!(
                            (linear::<R>(meter.peak_hold(channel)) - marker).abs()
                                < 1e-6 + marker * 1e-5
                        );
                        if value > 1e-3 {
                            assert!(
                                (meter.decibels(channel) as f64 - decibels(value)).abs() < 5e-3
                            );
                            assert!(
                                (meter.peak_hold_decibels(channel) as f64 - decibels(marker)).abs()
                                    < 5e-3
                            );
                        }
                    }
                }

                let mut tapped =
                    BallisticMeter::<R, 2>::new(ballistics, RATE).with_peak_hold(hold, fall as f32);
                let mut output = std::vec![0i16; input.len()];
                samples_using::<R, _>(&input, channels as u8)
                    .unwrap()
                    .tap(&mut tapped)
                    .write_samples(&mut output);
                assert_eq!(output, input);
                assert_eq!(tapped, meter);

                let mut sliced =
                    BallisticMeter::<R, 2>::new(ballistics, RATE).with_peak_hold(hold, fall as f32);
                let bytes: Vec<u8> = input
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect();
                analyze_bytes_using::<R, _>(&bytes, Format::I16, channels as u8, &mut sliced)
                    .unwrap();
                assert_eq!(sliced, meter);
            }
        }
    }

    #[test]
    fn fixed_matches_model() {
        matches_model::<Fixed>();
    }

    #[test]
    fn float_matches_model() {
        matches_model::<Float>();
    }

    fn vu_step<R: Smooth>() {
        let amplitude = 0.5;
        let steady = amplitude * core::f64::consts::FRAC_1_SQRT_2;
        let time_constant = 0.3 / (1.0 / (1.0 - 0.99f64 * 0.99)).ln();
        let mut meter = BallisticMeter::<R, 1>::new(Ballistics::VU, RATE);
        let rise = feed(&mut meter, &tone(amplitude, 48.0, 600 * WINDOW));

        assert!((rise[299] / steady - 0.99).abs() < 1e-4);
        assert!(rise[298] / steady < 0.99);
        for (index, value) in rise.iter().enumerate() {
            let time = (index + 1) as f64 / 1000.0;
            let expected = steady * (1.0 - (-time / time_constant).exp()).sqrt();
            assert!((value - expected).abs() < 1e-5);
            assert!(*value <= steady * (1.0 + 1e-5));
        }

        let fall = feed(&mut meter, &[0; 300 * WINDOW]);
        let start = rise[599] / steady;
        for (index, value) in fall.iter().enumerate() {
            let time = (index + 1) as f64 / 1000.0;
            let expected = steady * start * (-time / time_constant / 2.0).exp();
            assert!((value - expected).abs() < 1e-5);
        }
        assert!((fall[299] / steady - (1.0 - 0.99f64 * 0.99).sqrt()).abs() < 1e-3);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn fixed_vu_step() {
        vu_step::<Fixed>();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn float_vu_step() {
        vu_step::<Float>();
    }

    fn burst<R: Smooth>(ballistics: Ballistics, burst: usize, offset: usize) -> f64 {
        let mut samples = std::vec![0; offset];
        samples.extend(tone(0.5, 9.6, burst));
        samples.resize(offset + burst + 50 * WINDOW, 0);
        let mut meter = BallisticMeter::<R, 1>::new(ballistics, RATE);
        let highest = feed(&mut meter, &samples).into_iter().fold(0.0, f64::max);
        decibels(highest / 0.5)
    }

    fn ppm_bursts<R: Smooth>() {
        for (ballistics, integration) in [
            (Ballistics::PPM_DIN, 5),
            (Ballistics::PPM_NORDIC, 5),
            (Ballistics::PPM_EBU, 10),
        ] {
            let frames = integration * WINDOW;
            assert!((burst::<R>(ballistics, frames, 0) + 2.0).abs() < 0.01);
            assert!(burst::<R>(ballistics, 20 * frames, 0).abs() < 0.01);

            let misaligned = burst::<R>(ballistics, frames, WINDOW / 2);
            let longest =
                decibels(1.0 - (1.0 - 10f64.powf(-0.1)).powf(1.0 + 1.0 / integration as f64));
            assert!(
                misaligned > -1.9 && misaligned < longest + 0.01,
                "{misaligned} {longest}"
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn fixed_ppm_bursts() {
        ppm_bursts::<Fixed>();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn float_ppm_bursts() {
        ppm_bursts::<Float>();
    }

    fn ppm_falls<R: Smooth>() {
        for (ballistics, fall, decibel) in [
            (Ballistics::PPM_DIN, 1500, 20.0),
            (Ballistics::PPM_NORDIC, 1700, 20.0),
            (Ballistics::PPM_EBU, 2800, 24.0),
        ] {
            let mut meter = BallisticMeter::<R, 1>::new(ballistics, RATE);
            let steady = feed(&mut meter, &tone(0.5, 9.6, 100 * WINDOW))[99];
            assert!((steady / 0.5 - 1.0).abs() < 1e-6);

            let falling = feed(&mut meter, &std::vec![0; fall * WINDOW]);
            assert!((decibels(falling[fall - 1] / steady) + decibel).abs() < 0.01);
            for (index, value) in falling.iter().enumerate() {
                let expected = -decibel * (index + 1) as f64 / fall as f64;
                assert!((decibels(value / steady) - expected).abs() < 0.01);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn fixed_ppm_falls() {
        ppm_falls::<Fixed>();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn float_ppm_falls() {
        ppm_falls::<Float>();
    }

    fn holds<R: Smooth>() {
        let ballistics = Ballistics::new(Detector::Peak, Duration::ZERO, Duration::from_millis(10))
            .with_hold(Duration::from_millis(20));
        let mut meter = BallisticMeter::<R, 1>::new(ballistics, RATE)
            .with_peak_hold(Duration::from_millis(500), 20.0);
        let mut samples = tone(0.5, 9.6, 10 * WINDOW);
        samples.resize(2000 * WINDOW, 0);
        let readings: Vec<(f64, f64)> = samples
            .chunks(WINDOW)
            .map(|window| {
                analyze_using::<R, _, _>(window, 1, &mut meter).unwrap();
                (linear::<R>(meter.level(0)), linear::<R>(meter.peak_hold(0)))
            })
            .collect();

        for (index, (value, marker)) in readings.iter().enumerate() {
            if index < 30 {
                assert!((value - 0.5).abs() < 1e-6);
            } else {
                let expected = 0.5 * (-((index - 29) as f64) / 10.0).exp();
                assert!((value - expected).abs() < 1e-6);
            }

            if index < 530 {
                assert!((marker - 0.5).abs() < 1e-6);
            } else {
                let expected = -20.0 * (index - 529) as f64 / 1000.0;
                assert!((decibels(marker / 0.5) - expected).abs() < 0.01);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn fixed_holds() {
        holds::<Fixed>();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn float_holds() {
        holds::<Float>();
    }

    fn lifecycle<R: Smooth>() {
        let mut meter = BallisticMeter::<R, 2>::new(Ballistics::PPM_EBU, RATE)
            .with_window(Duration::from_micros(500))
            .with_peak_hold(Duration::from_secs(1), 0.0);

        assert_eq!(meter.meter().window(), 24);
        assert_eq!(meter.ballistics(), Ballistics::PPM_EBU);
        assert_eq!(analyze_using::<R, i16, _>(&[], 2, &mut meter), Ok(0));
        assert_eq!(meter.level(0), R::SILENCE);
        assert_eq!(meter.decibels(0), f32::NEG_INFINITY);
        assert_eq!(meter.level(7), R::SILENCE);
        assert_eq!(meter.decibels(7), f32::NEG_INFINITY);
        assert_eq!(meter.peak_hold(7), R::SILENCE);
        assert_eq!(meter.peak_hold_decibels(7), f32::NEG_INFINITY);

        let loud = R::from_sample(i16::MAX / 2);
        for index in 0..23 {
            assert!(!meter.measure(&[loud, loud, loud]));
            assert_eq!(meter.meter().windows(), 0, "{index}");
        }
        assert!(meter.measure(&[loud, R::SILENCE]));
        let first = meter.level(0);
        assert!(first > R::SILENCE);
        assert_eq!(meter.level(1), meter.peak_hold(1));

        for _ in 0..if cfg!(miri) { 2_400 } else { 240_000 } {
            meter.measure(&[R::SILENCE, R::SILENCE]);
        }
        assert!(meter.decibels(0) < if cfg!(miri) { -2.0 } else { -40.0 });
        assert_eq!(meter.peak_hold(0), first);

        meter.reset();
        assert_eq!(
            meter,
            BallisticMeter::new(Ballistics::PPM_EBU, RATE)
                .with_window(Duration::from_micros(500))
                .with_peak_hold(Duration::from_secs(1), 0.0)
        );
    }

    #[test]
    fn fixed_lifecycle() {
        lifecycle::<Fixed>();
    }

    #[test]
    fn float_lifecycle() {
        lifecycle::<Float>();
    }

    #[test]
    fn presets() {
        let close = |duration: Duration, seconds: f64| {
            assert!((duration.as_secs_f64() - seconds).abs() < 1e-9);
        };
        let burst = 1.0 / (1.0 / (1.0 - 10f64.powf(-0.1))).ln();
        let neper = 20.0 / 10f64.ln();

        close(
            Ballistics::VU.attack,
            0.3 / (1.0 / (1.0 - 0.99f64 * 0.99)).ln(),
        );
        assert_eq!(Ballistics::VU.attack, Ballistics::VU.release);
        assert_eq!(Ballistics::VU.detector, Detector::Rms);
        close(Ballistics::PPM_DIN.attack, 0.005 * burst);
        close(Ballistics::PPM_DIN.release, 1.5 * neper / 20.0);
        close(Ballistics::PPM_NORDIC.release, 1.7 * neper / 20.0);
        close(Ballistics::PPM_EBU.attack, 0.01 * burst);
        close(Ballistics::PPM_EBU.release, 2.8 * neper / 24.0);
        assert_eq!(Ballistics::PPM_EBU.detector, Detector::Peak);
        assert_eq!(Ballistics::PPM_EBU.hold, Duration::ZERO);
        assert_eq!(
            Ballistics::ppm(Duration::from_millis(5), 0.0, Duration::from_secs(1)).release,
            Duration::from_nanos(u64::MAX)
        );
        assert_eq!(
            Ballistics::VU.with_hold(Duration::from_millis(3)).hold,
            Duration::from_millis(3)
        );

        for index in 0..4000 {
            let exponent = index as f64 * 0.173;
            let exact = (-exponent).exp();
            let error = (exponential(exponent) - exact).abs() / exact;
            assert!(error <= 1e-13, "{exponent} {error}");
        }
        assert_eq!(exponential(f64::INFINITY), 0.0);
        assert_eq!(exponential(f64::NAN), 0.0);
        assert_eq!(exponential(0.0), 1.0);
        assert_eq!(Fixed::coefficient(0.0), 1 << 32);
        assert_eq!(Fixed::coefficient(1.0), 0);
    }

    #[test]
    fn fixed_extremes() {
        let mut meter = BallisticMeter::<Fixed, 1>::new(
            Ballistics::new(Detector::Peak, Duration::ZERO, Duration::from_millis(1)),
            RATE,
        );
        analyze_using::<Fixed, _, _>(&[i32::MIN; WINDOW], 1, &mut meter).unwrap();
        assert_eq!(meter.level(0), i32::MAX);
        assert!(meter.decibels(0).abs() < 1e-6);

        let mut meter = BallisticMeter::<Fixed, 1>::new(
            Ballistics::new(Detector::Rms, Duration::from_millis(2), Duration::ZERO),
            RATE,
        );
        for _ in 0..if cfg!(miri) { 4_800 } else { 200_000 } {
            meter.measure(&[i32::MIN]);
        }
        assert!(meter.level(0) > i32::MAX - 4);
        assert!(meter.decibels(0).abs() < 1e-6);
        assert_eq!(fraction(u64::MAX, 1 << 32), u64::MAX);
        assert_eq!(fraction(1 << 62, 1), 1 << 30);

        let mut meter = BallisticMeter::<Fixed, 1>::new(Ballistics::PPM_NORDIC, 0);
        meter.measure(&[1 << 30]);
        assert_eq!(meter.level(0), 1 << 30);
    }

    #[test]
    fn float_special_values() {
        let mut meter = BallisticMeter::<Float, 2>::new(Ballistics::PPM_NORDIC, RATE);
        for _ in 0..WINDOW {
            meter.measure(&[0.5, 0.5]);
        }
        let before = meter.level(0);

        meter.measure(&[f32::NAN, f32::INFINITY]);
        for _ in 1..WINDOW {
            meter.measure(&[0.5, 0.5]);
        }
        assert!(meter.level(0) > before);
        assert!(meter.level(1).is_finite() && meter.level(1) > 1e30);

        let mut meter = BallisticMeter::<Float, 1>::new(Ballistics::VU, RATE);
        meter.measure(&[f32::NAN]);
        for _ in 1..WINDOW {
            meter.measure(&[0.5]);
        }
        assert_eq!(meter.level(0), 0.0);

        for value in [f32::INFINITY, 0.0] {
            for _ in 0..WINDOW {
                meter.measure(&[value]);
            }
        }
        assert!(meter.level(0).is_finite() && meter.level(0) > 1e18);
        assert!(meter.decibels(0).is_finite());
    }
}
