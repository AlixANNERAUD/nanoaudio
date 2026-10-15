use nanoaudio_dsp::{Analyzer, Fixed, Float};

use crate::math;

/// Settings of a [`VoiceActivityDetector`], in units that do not depend on the sample rate.
///
/// The [`DEFAULT`](Self::DEFAULT) settings are meant for speech at any sample rate from 8 to 48 kHz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceSettings {
    /// Length of an analysis block, in milliseconds.
    pub block: u32,
    /// How far above the noise floor a block must be to count as voice, in dB.
    pub threshold: u8,
    /// Level below which a block never counts as voice, in dBFS.
    pub minimum: i16,
    /// Lowest zero-crossing rate of voiced speech, in crossings per second.
    pub lowest_crossings: u32,
    /// Highest zero-crossing rate of voiced speech, in crossings per second.
    pub highest_crossings: u32,
    /// How long voice must last before it is reported, in milliseconds.
    pub onset: u32,
    /// How long voice is still reported after the last voiced block, in milliseconds.
    pub hangover: u32,
    /// How far back the noise floor looks for the quietest level, in milliseconds.
    pub floor: u32,
}

impl VoiceSettings {
    /// Blocks of 10 ms, 6 dB above the floor and above -60 dBFS, 150 to 3000 crossings per second, 30 ms onset, 300 ms hangover and a 3 s floor window.
    pub const DEFAULT: Self = Self {
        block: 10,
        threshold: 6,
        minimum: -60,
        lowest_crossings: 150,
        highest_crossings: 3000,
        onset: 30,
        hangover: 300,
        floor: 3000,
    };
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

const MINIMUM_LEVEL: i32 = -100 << 8;
const FLOOR_SLOTS: usize = 4;
const DC_BITS: u32 = 12;
const MIX_SCALES: [f32; 9] = [
    32768.0, 32768.0, 16384.0, 8192.0, 8192.0, 4096.0, 4096.0, 4096.0, 4096.0,
];

/// Tells when someone is talking, from the short-term energy and zero-crossing rate of the signal.
///
/// The channels are mixed (summed, then divided by their count rounded up to a power of two), the DC and the low end below about 100 Hz removed, and the signal cut into blocks of about 10 ms. A block is voiced when its level is above the noise floor by the threshold and above the minimum level, and its zero-crossing rate is in the range of voiced speech, which leaves out hum, rumble, hiss and white noise. Crossings are counted only when the signal swings past the noise floor amplitude, so noise riding on a voice does not add any. Voice starts after a run of voiced blocks as long as the onset, lasts while blocks stay above the threshold whatever their crossing rate, so unvoiced sounds and short pauses between words keep it, and ends after a run of quieter blocks as long as the hangover.
///
/// The noise floor is the lowest level over the last few seconds, averaged over about four blocks, so it follows a changing background and assumes the signal is not voice all the time. It reads -100 dBFS until the first block ends, and the first blocks are taken as background.
///
/// Each frame costs a handful of 32-bit integer operations and one 32 × 32-bit multiply, on the top 16 bits of each sample, also with [`Float`], where NaN counts as silence and infinities as full scale. The decision runs once per block, also without floating point or division. Positions are counted in frames analyzed since creation or the last [`reset`](Self::reset).
///
/// ```
/// use nanoaudio_analysis::{VoiceActivityDetector, analyze_using};
/// use nanoaudio_dsp::Fixed;
///
/// let mut detector = VoiceActivityDetector::new(8000);
/// let mut samples = [0i16; 8000];
///
/// for (index, sample) in samples.iter_mut().enumerate().skip(4000) {
///     let phase = (index % 40) as i32;
///     *sample = (if phase < 20 { phase } else { 40 - phase } * 400 - 4000) as i16;
/// }
///
/// analyze_using::<Fixed, _, _>(&samples, 1, &mut detector).unwrap();
///
/// assert!(detector.is_voice());
/// assert_eq!(detector.transition(), 4000);
/// assert_eq!(detector.noise_floor(), -100.0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceActivityDetector {
    settings: VoiceSettings,
    sample_rate: u32,
    length: u32,
    logarithm: i32,
    shift: u32,
    threshold: i32,
    minimum: i32,
    lowest: u32,
    highest: u32,
    onset: u32,
    hangover: u32,
    window: u32,
    scale: i32,
    position: u32,
    dc: i32,
    sign: i32,
    energy: u64,
    counting: u32,
    level: i32,
    crossings: u32,
    floor: i32,
    dead: i32,
    smooth: i32,
    minima: [i32; FLOOR_SLOTS],
    current: i32,
    slot: u8,
    count: u32,
    score: i32,
    voice: bool,
    pending: u32,
    start: u64,
    completed: u64,
    blocks: u64,
    transition: u64,
}

impl VoiceActivityDetector {
    /// Creates a detector with the [default settings](VoiceSettings::DEFAULT) for a signal at `sample_rate` Hz.
    pub const fn new(sample_rate: u32) -> Self {
        Self::with_settings(sample_rate, VoiceSettings::DEFAULT)
    }

    /// Creates a detector with `settings` for a signal at `sample_rate` Hz.
    ///
    /// Durations are rounded to the nearest block, with at least one block, and a block to the nearest frame, between 1 and 65535 frames. It does not use floating point.
    pub const fn with_settings(sample_rate: u32, settings: VoiceSettings) -> Self {
        let length = rounded(settings.block as u64 * sample_rate as u64, 1000, 65535);
        let threshold = (settings.threshold as i32) << 8;
        let lowest = rounded(
            settings.lowest_crossings as u64 * length as u64,
            sample_rate as u64,
            u32::MAX,
        );
        let highest = rounded(
            settings.highest_crossings as u64 * length as u64,
            sample_rate as u64,
            u32::MAX,
        );
        let block = if settings.block == 0 {
            1
        } else {
            settings.block
        } as u64;
        let mut shift = 1;

        while shift < DC_BITS && sample_rate >> (shift + 1) >= 400 {
            shift += 1;
        }

        Self {
            settings,
            sample_rate,
            length,
            logarithm: logarithm(length as u64),
            shift,
            threshold,
            minimum: (settings.minimum as i32) << 8,
            lowest: if settings.lowest_crossings == 0 {
                0
            } else {
                lowest
            },
            highest: if settings.highest_crossings == 0 {
                0
            } else {
                highest
            },
            onset: rounded(settings.onset as u64, block, u32::MAX),
            hangover: rounded(settings.hangover as u64, block, u32::MAX),
            window: rounded(settings.floor as u64, block * FLOOR_SLOTS as u64, u32::MAX),
            scale: ((255 << 16) + span(threshold) - 1) / span(threshold),
            position: 0,
            dc: 0,
            sign: 0,
            energy: 0,
            counting: 0,
            level: MINIMUM_LEVEL,
            crossings: 0,
            floor: MINIMUM_LEVEL,
            dead: 0,
            smooth: MINIMUM_LEVEL,
            minima: [i32::MAX; FLOOR_SLOTS],
            current: i32::MAX,
            slot: 0,
            count: 0,
            score: 0,
            voice: false,
            pending: 0,
            start: 0,
            completed: 0,
            blocks: 0,
            transition: 0,
        }
    }

    /// Returns the settings.
    pub const fn settings(&self) -> VoiceSettings {
        self.settings
    }

    /// Returns the sample rate, in Hz.
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the length of a block, in frames.
    pub const fn block_frames(&self) -> u32 {
        self.length
    }

    /// Returns whether someone is talking.
    pub const fn is_voice(&self) -> bool {
        self.voice
    }

    /// Returns how voice-like the last blocks were, from 0 to 255.
    ///
    /// It is a running average over about four blocks, where a block in the voiced crossing range scores from 0 at the noise floor to 128 at the threshold above it and 255 at twice the threshold, and other blocks score 0. It moves before [`is_voice`](Self::is_voice) changes.
    pub const fn score(&self) -> u8 {
        ((self.score + 128) >> 8) as u8
    }

    /// Returns the noise floor estimate, in dBFS.
    pub fn noise_floor(&self) -> f32 {
        self.floor as f32 / 256.0
    }

    /// Returns the noise floor estimate, in 1/256 dBFS, without floating point.
    pub const fn noise_floor_fixed(&self) -> i32 {
        self.floor
    }

    /// Returns the RMS level of the last block, in dBFS, where a full-scale sine reads -3 dBFS and levels below -100 dBFS read -100.
    pub fn level(&self) -> f32 {
        self.level as f32 / 256.0
    }

    /// Returns the level of the last block, in 1/256 dBFS, without floating point.
    pub const fn level_fixed(&self) -> i32 {
        self.level
    }

    /// Returns the zero-crossing rate of the last block, in crossings per second.
    pub const fn crossing_rate(&self) -> u32 {
        ((self.crossings as u64 * self.sample_rate as u64 + self.length as u64 / 2)
            / self.length as u64) as u32
    }

    /// Returns the number of zero crossings in the last block.
    pub const fn crossings(&self) -> u32 {
        self.crossings
    }

    /// Returns the number of frames analyzed.
    pub const fn frames(&self) -> u64 {
        self.completed + self.position as u64
    }

    /// Returns the number of blocks completed.
    ///
    /// The results change only when it does, so it tells when to read them again.
    pub const fn blocks(&self) -> u64 {
        self.blocks
    }

    /// Returns the position of the first frame of the current state, in frames.
    ///
    /// It is where the voice or the quiet started, not where it was confirmed: the first frame of the first voiced block, or of the first block of the hangover. It is 0 before the first change.
    pub const fn transition(&self) -> u64 {
        self.transition
    }

    /// Forgets every frame analyzed, as if newly created.
    pub fn reset(&mut self) {
        *self = Self::with_settings(self.sample_rate, self.settings);
    }

    #[inline(always)]
    fn push(&mut self, mixed: i32) {
        self.dc += ((mixed << DC_BITS) - self.dc) >> self.shift;

        let filtered = mixed - (self.dc >> DC_BITS);
        let magnitude = filtered.unsigned_abs();
        let sign = if filtered > self.dead {
            0
        } else if filtered < -self.dead {
            -1
        } else {
            self.sign
        };

        self.energy += (magnitude * magnitude) as u64;
        self.counting += (sign ^ self.sign) as u32 & 1;
        self.sign = sign;
        self.position += 1;

        if self.position >= self.length {
            self.finish();
        }
    }

    fn finish(&mut self) {
        let level = decibels(logarithm(self.energy) - self.logarithm).max(MINIMUM_LEVEL);
        let crossings = self.counting;

        self.completed += self.position as u64;
        self.position = 0;
        self.energy = 0;
        self.counting = 0;
        self.blocks += 1;
        self.level = level;
        self.crossings = crossings;

        self.smooth = if self.blocks == 1 {
            level
        } else {
            self.smooth + ((level - self.smooth) >> 2)
        };
        self.current = self.current.min(self.smooth);
        self.count += 1;

        if self.count >= self.window {
            self.minima[self.slot as usize % FLOOR_SLOTS] = self.current;
            self.slot = (self.slot + 1) % FLOOR_SLOTS as u8;
            self.count = 0;
        }

        let floor = self
            .minima
            .iter()
            .fold(self.current, |floor, minimum| floor.min(*minimum));

        if self.count == 0 {
            self.current = i32::MAX;
        }

        self.floor = floor;
        self.dead = amplitude(floor);

        let above = level - floor;
        let audible = level >= self.minimum;
        let loud = audible && above >= self.threshold;
        let ranged = crossings >= self.lowest && crossings <= self.highest;
        let target = if audible && ranged {
            ((above.max(0).min(span(self.threshold)) * self.scale) >> 16).min(255)
        } else {
            0
        };

        self.score += ((target << 8) - self.score) >> 2;

        let keeps = if self.voice { loud } else { !(loud && ranged) };

        if keeps {
            self.pending = 0;
            return;
        }

        if self.pending == 0 {
            self.start = self.completed - self.length as u64;
        }

        self.pending += 1;

        let duration = if self.voice {
            self.hangover
        } else {
            self.onset
        };

        if self.pending >= duration {
            self.voice = !self.voice;
            self.transition = self.start;
            self.pending = 0;
        }
    }
}

impl Analyzer<Fixed> for VoiceActivityDetector {
    #[inline]
    fn analyze(&mut self, frame: &[i32]) {
        let shift = math::MIX_SHIFTS[frame.len().min(8)];
        let sum = frame
            .iter()
            .fold(0i32, |sum, value| sum.wrapping_add(value >> 16));

        self.push((sum >> shift).clamp(-32768, 32767));
    }
}

impl Analyzer<Float> for VoiceActivityDetector {
    #[inline]
    fn analyze(&mut self, frame: &[f32]) {
        let sum: f32 = frame.iter().sum();
        let mixed = sum * MIX_SCALES[frame.len().min(8)];

        self.push((mixed as i32).clamp(-32768, 32767));
    }
}

const fn rounded(numerator: u64, denominator: u64, maximum: u32) -> u32 {
    if denominator == 0 {
        return maximum;
    }

    let value = (numerator + denominator / 2) / denominator;

    if value == 0 {
        1
    } else if value > maximum as u64 {
        maximum
    } else {
        value as u32
    }
}

const fn span(threshold: i32) -> i32 {
    if threshold == 0 { 1 } else { 2 * threshold }
}

const fn logarithm(value: u64) -> i32 {
    let mut value = if value == 0 { 1 } else { value };
    let mut exponent = 63;

    if value >> 32 == 0 {
        value <<= 32;
        exponent -= 32;
    }

    if value >> 48 == 0 {
        value <<= 16;
        exponent -= 16;
    }

    if value >> 56 == 0 {
        value <<= 8;
        exponent -= 8;
    }

    if value >> 60 == 0 {
        value <<= 4;
        exponent -= 4;
    }

    if value >> 62 == 0 {
        value <<= 2;
        exponent -= 2;
    }

    if value >> 63 == 0 {
        value <<= 1;
        exponent -= 1;
    }

    let fraction = ((value >> 47) & 0xFFFF) as u32;
    let parabola = (fraction * (65536 - fraction)) >> 16;
    let bend = (parabola * (27689 - ((10184 * fraction) >> 16))) >> 16;

    (exponent << 12) + ((fraction + bend) >> 4) as i32
}

const fn amplitude(decibels: i32) -> i32 {
    let exponent = ((decibels * 10885) >> 16) + (15 << 8);

    if exponent < 0 {
        0
    } else {
        let fraction = exponent & 255;
        let mantissa = 256 + fraction - ((fraction * (256 - fraction) * 88) >> 16);

        (mantissa << (exponent >> 8)) >> 8
    }
}

const fn decibels(logarithm: i32) -> i32 {
    ((logarithm - (30 << 12)) * 3083) >> 14
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use nanoaudio_core::Format;
    use nanoaudio_dsp::{
        Fixed, Float, Representation, Stream, analyze_bytes_using, analyze_using, samples_using,
    };

    use super::*;

    const TAU: f64 = core::f64::consts::TAU;

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn uniform(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }

        fn between(&mut self, low: f64, high: f64) -> f64 {
            low + (high - low) * self.uniform()
        }

        fn gaussian(&mut self) -> f64 {
            let radius = (-2.0 * (1.0 - self.uniform()).ln()).sqrt();
            radius * (TAU * self.uniform()).cos()
        }
    }

    struct Resonator {
        first: f64,
        second: f64,
        gain: f64,
        previous: f64,
        before: f64,
    }

    impl Resonator {
        fn new(frequency: f64, bandwidth: f64, rate: f64) -> Self {
            let radius = (-core::f64::consts::PI * bandwidth / rate).exp();
            let first = 2.0 * radius * (TAU * frequency / rate).cos();
            let second = -radius * radius;
            Self {
                first,
                second,
                gain: 1.0 - first - second,
                previous: 0.0,
                before: 0.0,
            }
        }

        fn process(&mut self, input: f64) -> f64 {
            let output = self.gain * input + self.first * self.previous + self.second * self.before;
            self.before = self.previous;
            self.previous = output;
            output
        }
    }

    const VOWELS: [[f64; 3]; 6] = [
        [730.0, 1090.0, 2440.0],
        [270.0, 2290.0, 3010.0],
        [300.0, 870.0, 2240.0],
        [530.0, 1840.0, 2480.0],
        [570.0, 840.0, 2410.0],
        [660.0, 1720.0, 2410.0],
    ];

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Voice {
        Formants,
        Tones,
    }

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Noise {
        White,
        Pink,
        Hum,
    }

    struct Scene {
        speech: Vec<f64>,
        truth: Vec<(usize, usize)>,
    }

    fn envelope(position: usize, length: usize, ramp: usize) -> f64 {
        let ramp = ramp.min(length / 2).max(1);
        let distance = position.min(length - 1 - position);

        if distance >= ramp {
            1.0
        } else {
            0.5 - 0.5 * (core::f64::consts::PI * distance as f64 / ramp as f64).cos()
        }
    }

    fn syllable(
        output: &mut [f64],
        voice: Voice,
        rate: f64,
        pitch: f64,
        vowel: [f64; 3],
        random: &mut Random,
    ) {
        let length = output.len();
        let glide = random.between(-0.15, 0.15);
        let ramp = (0.03 * rate) as usize;
        let mut formants = vowel.map(|frequency| Resonator::new(frequency, 90.0, rate));
        let mut glottis = 0.0;
        let mut phase = 0.0f64;

        for (index, sample) in output.iter_mut().enumerate() {
            let progress = index as f64 / length as f64;
            let frequency = pitch * (1.0 + glide * progress) * (1.0 + 0.01 * random.gaussian());
            let value = match voice {
                Voice::Formants => {
                    phase += frequency / rate;
                    let pulse = if phase >= 1.0 {
                        phase -= 1.0;
                        1.0
                    } else {
                        0.0
                    };
                    glottis = 0.9 * glottis + pulse;
                    formants
                        .iter_mut()
                        .fold(glottis, |signal, formant| formant.process(signal))
                }
                Voice::Tones => {
                    phase += frequency / rate;
                    (1..=5)
                        .map(|harmonic| (TAU * phase * harmonic as f64).sin() / harmonic as f64)
                        .sum()
                }
            };
            *sample = value;
        }

        let power = output.iter().map(|sample| sample * sample).sum::<f64>() / length as f64;
        let gain = 1.0 / power.sqrt().max(1e-12);

        for (index, sample) in output.iter_mut().enumerate() {
            *sample *= gain * envelope(index, length, ramp);
        }
    }

    fn fricative(output: &mut [f64], rate: f64, random: &mut Random) {
        let length = output.len();
        let ramp = (0.015 * rate) as usize;
        let mut previous = 0.0;

        for (index, sample) in output.iter_mut().enumerate() {
            let white = random.gaussian();
            *sample = 0.15 * (white - previous) * envelope(index, length, ramp);
            previous = white;
        }
    }

    fn scene(voice: Voice, rate: u32, seconds: f64, seed: u64) -> Scene {
        let rate_f = rate as f64;
        let total = (seconds * rate_f) as usize;
        let mut speech = vec![0.0; total];
        let mut truth = Vec::new();
        let mut random = Random(seed);
        let mut position = (1.5 * rate_f) as usize;

        while position < total {
            let pitch = random.between(95.0, 230.0);
            let words = 2 + (random.next() % 5) as usize;
            let begin = position;

            for word in 0..words {
                if word > 0 {
                    position =
                        (position + (random.between(0.04, 0.22) * rate_f) as usize).min(total);
                }

                if voice == Voice::Formants && random.uniform() < 0.3 {
                    let length = (random.between(0.05, 0.11) * rate_f) as usize;
                    let end = (position + length).min(total);
                    fricative(&mut speech[position..end], rate_f, &mut random);
                    position = end;
                }

                for _ in 0..1 + random.next() % 3 {
                    let length = (random.between(0.12, 0.28) * rate_f) as usize;
                    let end = (position + length).min(total);
                    let vowel = VOWELS[(random.next() % VOWELS.len() as u64) as usize];
                    let loudness = random.between(0.5, 1.0);
                    syllable(
                        &mut speech[position..end],
                        voice,
                        rate_f,
                        pitch,
                        vowel,
                        &mut random,
                    );
                    speech[position..end]
                        .iter_mut()
                        .for_each(|sample| *sample *= loudness);
                    position = end;
                }

                if voice == Voice::Formants && random.uniform() < 0.3 {
                    let length = (random.between(0.05, 0.11) * rate_f) as usize;
                    let end = (position + length).min(total);
                    fricative(&mut speech[position..end], rate_f, &mut random);
                    position = end;
                }

                if position >= total {
                    break;
                }
            }

            truth.push((begin, position.min(total)));
            position += (random.between(0.8, 1.6) * rate_f) as usize;
        }

        let active: f64 = truth.iter().map(|(begin, end)| end - begin).sum::<usize>() as f64;
        let power = speech.iter().map(|sample| sample * sample).sum::<f64>() / active;
        let gain = 10f64.powf(-26.0 / 20.0) / power.sqrt();

        speech.iter_mut().for_each(|sample| *sample *= gain);

        Scene { speech, truth }
    }

    fn noise(kind: Noise, rate: u32, length: usize, seed: u64) -> Vec<f64> {
        let mut random = Random(seed);
        let mut poles = [0.0f64; 3];
        let mut signal: Vec<f64> = (0..length)
            .map(|index| {
                let white = random.gaussian();
                match kind {
                    Noise::White => white,
                    Noise::Pink => {
                        poles[0] = 0.99765 * poles[0] + white * 0.0990460;
                        poles[1] = 0.96300 * poles[1] + white * 0.2965164;
                        poles[2] = 0.57000 * poles[2] + white * 1.0526913;
                        poles.iter().sum::<f64>() + white * 0.1848
                    }
                    Noise::Hum => {
                        let time = index as f64 / rate as f64;
                        (TAU * 50.0 * time).sin() + 0.3 * (TAU * 150.0 * time).sin() + 0.1 * white
                    }
                }
            })
            .collect();
        let power = signal.iter().map(|sample| sample * sample).sum::<f64>() / length as f64;

        signal.iter_mut().for_each(|sample| *sample /= power.sqrt());
        signal
    }

    fn quantize(signal: &[f64]) -> Vec<i16> {
        signal
            .iter()
            .map(|sample| (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
            .collect()
    }

    fn decisions<R: Representation>(
        samples: &[i16],
        channels: u8,
        detector: &mut VoiceActivityDetector,
    ) -> Vec<bool>
    where
        VoiceActivityDetector: Analyzer<R>,
    {
        let chunk = detector.block_frames() as usize * channels as usize;

        samples
            .chunks(chunk)
            .map(|block| {
                analyze_using::<R, _, _>(block, channels, &mut *detector).unwrap();
                detector.is_voice()
            })
            .collect()
    }

    struct Score {
        accuracy: f64,
        recall: f64,
        false_alarms: f64,
    }

    fn evaluate(
        truth: &[(usize, usize)],
        decided: &[bool],
        length: usize,
        settings: VoiceSettings,
        rate: u32,
    ) -> Score {
        let grace_before = (settings.onset + 2 * settings.block) as usize * rate as usize / 1000;
        let grace_after = (settings.hangover + settings.block) as usize * rate as usize / 1000;
        let (mut correct, mut scored, mut hits, mut speech, mut alarms, mut quiet) =
            (0, 0, 0, 0, 0, 0);

        for (index, decision) in decided.iter().enumerate() {
            let begin = index * length;
            let end = begin + length;
            let inside = truth
                .iter()
                .any(|(start, stop)| begin >= *start && end <= *stop);
            let near = truth.iter().any(|(start, stop)| {
                (begin < start + grace_before && end > *start)
                    || (begin < stop + grace_after && end > *stop)
            });

            if near || index * length < settings.floor as usize * rate as usize / 1000 / 4 {
                continue;
            }

            if inside {
                speech += 1;
                hits += *decision as usize;
            } else {
                quiet += 1;
                alarms += *decision as usize;
            }

            scored += 1;
            correct += (*decision == inside) as usize;
        }

        Score {
            accuracy: correct as f64 / scored as f64,
            recall: hits as f64 / speech.max(1) as f64,
            false_alarms: alarms as f64 / quiet.max(1) as f64,
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn detects_synthetic_speech() {
        const RATIOS: [f64; 5] = [30.0, 20.0, 10.0, 5.0, 0.0];

        let settings = VoiceSettings::DEFAULT;
        let mut totals = [(0.0, 0.0, 0.0, 0usize); RATIOS.len()];

        for voice in [Voice::Formants, Voice::Tones] {
            for (index, rate) in [8000, 16000, 48000].into_iter().enumerate() {
                let scene = scene(voice, rate, 20.0, 7 + index as u64);

                for kind in [Noise::White, Noise::Pink, Noise::Hum] {
                    let background = noise(kind, rate, scene.speech.len(), 99 + index as u64);

                    for (slot, ratio) in RATIOS.iter().enumerate() {
                        let gain = 10f64.powf((-26.0 - ratio) / 20.0);
                        let mixed: Vec<f64> = scene
                            .speech
                            .iter()
                            .zip(&background)
                            .map(|(speech, noise)| speech + gain * noise)
                            .collect();
                        let samples = quantize(&mixed);
                        let mut detector = VoiceActivityDetector::new(rate);
                        let decided = decisions::<Fixed>(&samples, 1, &mut detector);
                        let score = evaluate(
                            &scene.truth,
                            &decided,
                            detector.block_frames() as usize,
                            settings,
                            rate,
                        );
                        let label = std::format!("{voice:?} {rate} Hz {kind:?} {ratio} dB");

                        std::println!(
                            "{label}: accuracy {:.3} recall {:.3} false alarms {:.3}",
                            score.accuracy,
                            score.recall,
                            score.false_alarms,
                        );

                        assert!(score.false_alarms <= 0.01, "{label}");

                        if *ratio >= 10.0 {
                            assert!(score.accuracy >= 0.97, "{label}");
                        } else if *ratio >= 5.0 {
                            assert!(score.accuracy >= 0.8, "{label}");
                        }

                        let total = &mut totals[slot];
                        total.0 += score.accuracy;
                        total.1 += score.recall;
                        total.2 += score.false_alarms;
                        total.3 += 1;
                    }
                }
            }
        }

        for (ratio, (accuracy, recall, alarms, count)) in RATIOS.iter().zip(totals) {
            let count = count as f64;
            std::println!(
                "{ratio} dB SNR: accuracy {:.3} recall {:.3} false alarms {:.3}",
                accuracy / count,
                recall / count,
                alarms / count
            );
        }
    }

    fn sine(rate: u32, frequency: f64, amplitude: f64, length: usize) -> Vec<i16> {
        (0..length)
            .map(|index| {
                (amplitude * 32767.0 * (TAU * frequency * index as f64 / rate as f64).sin()).round()
                    as i16
            })
            .collect()
    }

    fn high_pass_gain(rate: u32, frequency: f64) -> f64 {
        let mut shift = 1;

        while shift < 12 && rate >> (shift + 1) >= 400 {
            shift += 1;
        }

        let alpha = 1.0 / (1u32 << shift) as f64;
        let angle = TAU * frequency / rate as f64;
        let numerator = (1.0 - alpha) * (2.0 - 2.0 * angle.cos()).sqrt();
        let pole = 1.0 - alpha;
        let denominator = (1.0 - 2.0 * pole * angle.cos() + pole * pole).sqrt();

        numerator / denominator
    }

    #[test]
    fn follows_closed_forms() {
        let cases: &[(u32, f64, f64)] = if cfg!(miri) {
            &[(8000, 1000.0, 0.5)]
        } else {
            &[
                (8000, 1000.0, 0.5),
                (8000, 300.0, 0.01),
                (16000, 400.0, 0.9),
                (16000, 3000.0, 0.1),
                (48000, 1200.0, 0.3),
                (44100, 2205.0, 0.05),
            ]
        };

        for (rate, frequency, amplitude) in cases {
            let mut detector = VoiceActivityDetector::new(*rate);
            let length = detector.block_frames() as usize;
            let signal = sine(*rate, *frequency, *amplitude, 30 * length);

            analyze_using::<Fixed, _, _>(&signal, 1, &mut detector).unwrap();

            let gain = high_pass_gain(*rate, *frequency);
            let expected = 20.0 * (amplitude * 32767.0 / 32768.0 * gain / 2f64.sqrt()).log10();
            let crossings = 2.0 * frequency * length as f64 / *rate as f64;

            assert!(
                (detector.level() as f64 - expected).abs() < 0.05,
                "{rate} {frequency} {} {expected}",
                detector.level()
            );
            assert!((detector.crossings() as f64 - crossings).abs() <= 1.0);
            assert_eq!(detector.blocks(), 30);
            assert_eq!(detector.frames(), 30 * length as u64);
            assert!(!detector.is_voice());
            assert_eq!(detector.transition(), 0);
            assert!(detector.score() < 8);
            assert!(
                (detector.noise_floor() as f64 - expected).abs() < 0.3,
                "{rate} {frequency} {} {expected}",
                detector.noise_floor()
            );
            assert!(
                detector
                    .crossing_rate()
                    .abs_diff((2.0 * frequency).round() as u32)
                    <= (*rate as f64 / length as f64) as u32
            );
        }
    }

    #[test]
    fn approximates_logarithms() {
        let mut random = Random(5);

        for index in 0..if cfg!(miri) { 200 } else { 20000 } {
            let value = match index {
                0 => 1,
                1 => u64::MAX,
                _ => random.next() >> (random.next() % 64),
            }
            .max(1);
            let error = logarithm(value) as f64 / 4096.0 - (value as f64).log2();

            assert!(error.abs() < 0.003, "{value} {error}");
        }

        assert_eq!(logarithm(0), 0);

        for decibels in -100..=0 {
            let expected = 32768.0 * 10f64.powf(decibels as f64 / 20.0);
            let actual = amplitude(decibels << 8) as f64;

            assert!(
                (actual - expected).abs() <= expected * 0.01 + 1.0,
                "{decibels} {actual} {expected}"
            );
        }
    }

    struct Reference {
        length: usize,
        alpha: f64,
        dc: f64,
        negative: bool,
        energy: f64,
        crossings: u32,
        position: usize,
        smooth: Option<f64>,
        minima: Vec<f64>,
        current: f64,
        count: u32,
        window: u32,
        floor: f64,
        voice: bool,
        pending: u32,
        settings: VoiceSettings,
        rate: u32,
    }

    impl Reference {
        fn new(rate: u32, settings: VoiceSettings) -> Self {
            let shift = (rate as f64 / 400.0).log2().floor().clamp(1.0, 12.0);
            Self {
                length: (rate as f64 * settings.block as f64 / 1000.0).round() as usize,
                alpha: 0.5f64.powf(shift),
                dc: 0.0,
                negative: false,
                energy: 0.0,
                crossings: 0,
                position: 0,
                smooth: None,
                minima: Vec::new(),
                current: f64::INFINITY,
                count: 0,
                window: (settings.floor as f64 / settings.block as f64 / 4.0).round() as u32,
                floor: -100.0,
                voice: false,
                pending: 0,
                settings,
                rate,
            }
        }

        fn push(&mut self, sample: i16) -> Option<(f64, u32, f64, bool)> {
            let sample = sample as f64;
            self.dc += self.alpha * (sample - self.dc);
            let filtered = sample - self.dc;
            let dead = 32768.0 * 10f64.powf(self.floor / 20.0);

            self.energy += filtered * filtered;

            if (self.negative && filtered > dead) || (!self.negative && filtered < -dead) {
                self.negative = !self.negative;
                self.crossings += 1;
            }

            self.position += 1;

            if self.position < self.length {
                return None;
            }

            let level =
                (10.0 * (self.energy / self.length as f64 / 1073741824.0).log10()).max(-100.0);
            let smooth = match self.smooth {
                Some(smooth) => smooth + (level - smooth) / 4.0,
                None => level,
            };
            let crossings = core::mem::take(&mut self.crossings);

            self.smooth = Some(smooth);
            self.position = 0;
            self.energy = 0.0;
            self.current = self.current.min(smooth);
            self.count += 1;

            if self.count >= self.window {
                self.minima.push(self.current);
                self.current = f64::INFINITY;
                self.count = 0;
            }

            self.floor = self
                .minima
                .iter()
                .rev()
                .take(4)
                .fold(self.current, |floor, minimum| floor.min(*minimum));

            let block = self.length as f64 / self.rate as f64;
            let rate = crossings as f64 / block;
            let loud = level >= self.settings.minimum as f64
                && level - self.floor >= self.settings.threshold as f64;
            let ranged = rate >= self.settings.lowest_crossings as f64
                && rate <= self.settings.highest_crossings as f64;
            let keeps = if self.voice { loud } else { !(loud && ranged) };

            if keeps {
                self.pending = 0;
            } else {
                self.pending += 1;

                let duration = if self.voice {
                    self.settings.hangover
                } else {
                    self.settings.onset
                } / self.settings.block;

                if self.pending >= duration {
                    self.voice = !self.voice;
                    self.pending = 0;
                }
            }

            Some((level, crossings, self.floor, self.voice))
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn follows_the_reference() {
        let mut checked = 0;
        let mut disagreements = 0;
        let mut worst = (0.0f64, 0.0f64, 0u32, 0u32);

        for (rate, kind, ratio) in [
            (16000, Noise::Pink, 10.0),
            (8000, Noise::White, 20.0),
            (48000, Noise::Hum, 5.0),
        ] {
            let scene = scene(Voice::Formants, rate, 12.0, rate as u64);
            let background = noise(kind, rate, scene.speech.len(), 3);
            let gain = 10f64.powf((-26.0 - ratio) / 20.0);
            let mixed: Vec<f64> = scene
                .speech
                .iter()
                .zip(&background)
                .map(|(speech, noise)| speech + gain * noise)
                .collect();
            let samples = quantize(&mixed);
            let mut detector = VoiceActivityDetector::new(rate);
            let mut reference = Reference::new(rate, VoiceSettings::DEFAULT);

            for sample in samples {
                Analyzer::<Fixed>::analyze(&mut detector, &[Fixed::from_sample(sample)]);

                if let Some((level, crossings, floor, voice)) = reference.push(sample) {
                    worst.0 = worst.0.max((detector.level() as f64 - level).abs());
                    worst.1 = worst.1.max((detector.noise_floor() as f64 - floor).abs());
                    let difference = detector.crossings().abs_diff(crossings);
                    worst.2 = worst.2.max(difference);
                    worst.3 += (difference > 2) as u32;
                    checked += 1;
                    disagreements += (detector.is_voice() != voice) as u32;
                }
            }
        }

        std::println!("{worst:?} {disagreements} of {checked}");
        assert!(worst.0 < 0.03 && worst.1 < 0.05 && worst.2 <= 4);
        assert!(worst.3 * 100 <= checked);
        assert!(checked > 3000);
        assert!(
            disagreements * 200 <= checked,
            "{disagreements} of {checked}"
        );
    }

    fn state(detector: &VoiceActivityDetector) -> (bool, u64, u64, u64, i32, i32, u32, u8) {
        (
            detector.is_voice(),
            detector.transition(),
            detector.frames(),
            detector.blocks(),
            detector.level_fixed(),
            detector.noise_floor_fixed(),
            detector.crossings(),
            detector.score(),
        )
    }

    fn paths<R: Representation>(signal: &[i16], channels: u8, rate: u32) -> VoiceActivityDetector
    where
        VoiceActivityDetector: Analyzer<R>,
    {
        let mut tapped = VoiceActivityDetector::new(rate);
        let mut output = vec![0i16; signal.len()];

        let frames = samples_using::<R, _>(signal, channels)
            .unwrap()
            .tap(&mut tapped)
            .write_samples(&mut output);

        assert_eq!(frames as u64, tapped.frames());
        assert_eq!(output, signal);

        let mut sliced = VoiceActivityDetector::new(rate);

        assert_eq!(
            analyze_using::<R, _, _>(signal, channels, &mut sliced),
            Ok(frames)
        );
        assert_eq!(state(&sliced), state(&tapped));

        let bytes: Vec<u8> = signal
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let mut decoded = VoiceActivityDetector::new(rate);

        assert_eq!(
            analyze_bytes_using::<R, _>(&bytes, Format::I16, channels, &mut decoded),
            Ok(frames)
        );
        assert_eq!(state(&decoded), state(&tapped));

        tapped
    }

    #[test]
    fn takes_every_path() {
        let rate = 8000;
        let seconds = if cfg!(miri) { 0.15 } else { 6.0 };
        let mut speech = vec![0i16; (seconds * rate as f64) as usize];
        let begin = speech.len() / 3;

        for (index, sample) in speech.iter_mut().enumerate().skip(begin) {
            if (index - begin) % 4000 < 3200 {
                *sample = triangle(index, 40, 3000);
            }
        }

        let mono_fixed = paths::<Fixed>(&speech, 1, rate);
        let mono_float = paths::<Float>(&speech, 1, rate);

        assert_eq!(state(&mono_fixed), state(&mono_float));
        assert!(mono_fixed.is_voice());
        assert_eq!(mono_fixed.transition(), begin as u64);

        for channels in [2u8, 3] {
            let interleaved: Vec<i16> = speech
                .iter()
                .enumerate()
                .flat_map(|(index, sample)| {
                    (0..channels).map(move |channel| match channel {
                        0 => *sample,
                        1 => sample / 2 + (index % 7) as i16 - 3,
                        _ => -*sample,
                    })
                })
                .collect();
            let fixed = paths::<Fixed>(&interleaved, channels, rate);
            let float = paths::<Float>(&interleaved, channels, rate);

            assert!(fixed.is_voice() && float.is_voice());
            assert_eq!(fixed.transition(), begin as u64);
            assert_eq!(float.transition(), begin as u64);
            assert_eq!(fixed.blocks(), float.blocks());
            assert_eq!(fixed.frames(), speech.len() as u64);
        }
    }

    fn triangle(index: usize, period: usize, amplitude: i32) -> i16 {
        let phase = (index % period) as i32;
        let half = period as i32 / 2;
        let ramp = if phase < half {
            phase
        } else {
            period as i32 - phase
        };

        ((2 * ramp - half) * amplitude / half) as i16
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn finds_onsets_and_offsets() {
        let rate = 16000;
        let mut signal = vec![0i16; 80000];

        for (index, sample) in signal.iter_mut().enumerate() {
            if (16000..32000).contains(&index) || (34400..42400).contains(&index) {
                *sample = triangle(index, 50, 3000);
            }
        }

        let mut detector = VoiceActivityDetector::new(rate);

        assert_eq!(detector.block_frames(), 160);

        analyze_using::<Fixed, _, _>(&signal[..16479], 1, &mut detector).unwrap();
        assert!(!detector.is_voice());
        assert!(detector.score() > 0);

        analyze_using::<Fixed, _, _>(&signal[16479..16480], 1, &mut detector).unwrap();
        assert!(detector.is_voice());
        assert_eq!(detector.transition(), 16000);
        assert_eq!(detector.noise_floor(), -100.0);
        assert!([600, 700].contains(&detector.crossing_rate()));

        analyze_using::<Fixed, _, _>(&signal[16480..42400 + 4640], 1, &mut detector).unwrap();
        assert!(detector.is_voice());
        assert_eq!(detector.transition(), 16000);
        assert!(detector.score() < 8);

        analyze_using::<Fixed, _, _>(&signal[42400 + 4640..42400 + 4800], 1, &mut detector)
            .unwrap();
        assert!(!detector.is_voice());
        assert_eq!(detector.transition(), 42400);

        analyze_using::<Fixed, _, _>(&signal[42400 + 4800..], 1, &mut detector).unwrap();
        assert!(!detector.is_voice());
        assert_eq!(detector.frames(), 80000);
        assert_eq!(detector.blocks(), 500);

        let mut steady = VoiceActivityDetector::new(rate);
        analyze_using::<Fixed, _, _>(&signal[16000..32000], 1, &mut steady).unwrap();
        assert!(!steady.is_voice());
        assert!(steady.score() < 8);

        let mut late = VoiceActivityDetector::new(rate);
        analyze_using::<Fixed, _, _>(&signal[..20000], 1, &mut late).unwrap();
        assert_eq!(late.score(), 255);

        late.reset();
        assert_eq!(late, VoiceActivityDetector::new(rate));
    }

    #[test]
    fn ignores_noise_and_hum() {
        let seconds = if cfg!(miri) { 0.12 } else { 8.0 };
        let rates: &[u32] = if cfg!(miri) {
            &[8000]
        } else {
            &[8000, 16000, 48000]
        };

        for rate in rates.iter().copied() {
            let length = (seconds * rate as f64) as usize;
            let start = length / 4;
            let white = noise(Noise::White, rate, length, 11);
            let pink = noise(Noise::Pink, rate, length, 12);
            let hum: Vec<f64> = (0..length)
                .map(|index| (TAU * 50.0 * index as f64 / rate as f64).sin())
                .collect();

            for (name, signal, level) in [
                ("silence", vec![0.0; length], 0.0),
                ("white", white.clone(), 0.03),
                ("pink", pink.clone(), 0.03),
                ("hum", hum.clone(), 0.1),
                ("loud hum", hum.clone(), 0.9),
            ] {
                for sudden in [false, true] {
                    let scaled: Vec<f64> = signal
                        .iter()
                        .enumerate()
                        .map(|(index, sample)| {
                            if sudden && index < start {
                                0.0
                            } else {
                                sample * level
                            }
                        })
                        .collect();
                    let samples = quantize(&scaled);
                    let mut detector = VoiceActivityDetector::new(rate);
                    let decided = decisions::<Fixed>(&samples, 1, &mut detector);
                    let alarms = decided.iter().filter(|voice| **voice).count();

                    if name == "pink" && sudden {
                        continue;
                    }

                    assert_eq!(alarms, 0, "{name} {rate} {sudden}");
                }
            }
        }
    }

    #[test]
    fn handles_edges() {
        let mut detector = VoiceActivityDetector::new(8000);

        assert_eq!(
            analyze_using::<Fixed, _, _>(&[0i16; 0], 2, &mut detector),
            Ok(0)
        );
        assert_eq!(detector.frames(), 0);
        assert_eq!(detector.noise_floor(), -100.0);
        assert_eq!(detector.level(), -100.0);
        assert_eq!(detector.crossing_rate(), 0);

        for _ in 0..80 {
            Analyzer::<Fixed>::analyze(&mut detector, &[]);
        }

        assert_eq!((detector.blocks(), detector.level()), (1, -100.0));

        let mut minimum = VoiceActivityDetector::new(8000);

        for index in 0..800 {
            let value = if index % 2 == 0 { i32::MIN } else { i32::MAX };
            Analyzer::<Fixed>::analyze(&mut minimum, &[value]);

            if index == 79 {
                assert_eq!(minimum.crossings(), 80);
            }
        }

        let nyquist = 20.0 * high_pass_gain(8000, 4000.0).log10();

        assert!((minimum.level() as f64 - nyquist).abs() < 0.02);
        assert!(!minimum.is_voice());

        let mut float = VoiceActivityDetector::new(8000);

        for index in 0..800 {
            let value = match index % 4 {
                0 => f32::INFINITY,
                1 => f32::NAN,
                2 => f32::NEG_INFINITY,
                _ => f32::NAN,
            };
            Analyzer::<Float>::analyze(&mut float, &[value]);
        }

        let mut expected = VoiceActivityDetector::new(8000);

        for index in 0..800 {
            let value = match index % 4 {
                0 => 32767i16,
                2 => -32768,
                _ => 0,
            };
            Analyzer::<Fixed>::analyze(&mut expected, &[Fixed::from_sample(value)]);
        }

        assert_eq!(state(&float), state(&expected));

        if cfg!(miri) {
            return;
        }

        let mut long = VoiceActivityDetector::with_settings(
            48000,
            VoiceSettings {
                block: 5000,
                ..VoiceSettings::DEFAULT
            },
        );

        assert_eq!(long.block_frames(), 65535);

        long.completed = u64::MAX - 70000;

        for index in 0..65535 {
            let value = if index % 2 == 0 { i16::MIN } else { i16::MAX };
            Analyzer::<Fixed>::analyze(&mut long, &[Fixed::from_sample(value)]);
        }

        let nyquist = 20.0 * high_pass_gain(48000, 24000.0).log10();

        assert!((long.level() as f64 - nyquist).abs() < 0.02);
        assert_eq!(long.frames(), u64::MAX - 70000 + 65535);
        assert_eq!(long.blocks(), 1);
    }

    #[test]
    fn converts_the_settings() {
        for (rate, frames, shift) in [
            (8000, 80, 4),
            (16000, 160, 5),
            (44100, 441, 6),
            (48000, 480, 6),
            (96000, 960, 7),
        ] {
            let detector = VoiceActivityDetector::new(rate);

            assert_eq!(detector.block_frames(), frames);
            assert_eq!(detector.shift, shift);
            assert_eq!(detector.sample_rate(), rate);
            assert_eq!(detector.settings(), VoiceSettings::default());
            assert_eq!(
                (detector.onset, detector.hangover, detector.window),
                (3, 30, 75)
            );
            assert_eq!(
                (detector.lowest, detector.highest),
                (
                    (150 * frames + rate / 2) / rate,
                    (3000 * frames + rate / 2) / rate
                )
            );
            assert_eq!(detector.threshold, 6 << 8);
        }

        let settings = VoiceSettings {
            block: 0,
            threshold: 0,
            minimum: -120,
            lowest_crossings: 0,
            highest_crossings: 0,
            onset: 0,
            hangover: 0,
            floor: 0,
        };
        let detector = VoiceActivityDetector::with_settings(16000, settings);

        assert_eq!(detector.block_frames(), 1);
        assert_eq!(
            (detector.onset, detector.hangover, detector.window),
            (1, 1, 1)
        );
        assert_eq!((detector.lowest, detector.highest), (0, 0));
        assert_eq!(detector.settings(), settings);

        let mut stopped = VoiceActivityDetector::new(0);

        assert_eq!(stopped.block_frames(), 1);

        for _ in 0..10 {
            Analyzer::<Fixed>::analyze(&mut stopped, &[i32::MAX]);
        }

        assert!(!stopped.is_voice());
        assert_eq!(stopped.crossing_rate(), 0);
    }
}
