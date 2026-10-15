use nanoaudio_dsp::{Analyzer, Fixed, Float};

/// Settings of a [`VoiceActivityDetector`], in units that do not depend on the sample rate.
///
/// The [`DEFAULT`](Self::DEFAULT) settings suit speech at 8, 16 and 48 kHz with a close or desk microphone.
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
    /// How far back the noise floor looks for the quietest block, in milliseconds.
    pub floor: u32,
}

impl VoiceSettings {
    /// Blocks of 10 ms, 9 dB above the floor and above -60 dBFS, 150 to 3000 crossings per second, 30 ms onset, 300 ms hangover and a 3 s floor window.
    pub const DEFAULT: Self = Self {
        block: 10,
        threshold: 9,
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
const MIX_SHIFTS: [u8; 9] = [0, 0, 1, 2, 2, 3, 3, 3, 3];

/// Tells when someone is talking, from the short-term energy and zero-crossing rate of the signal.
///
/// The channels are mixed, the DC offset removed, and the signal cut into blocks of about 10 ms. A block is voiced when its level is above the noise floor by the threshold, above the minimum level, and its zero-crossing rate is in the range of voiced speech, which leaves out hum, rumble, hiss and white noise. Voice starts after a run of voiced blocks as long as the onset, and lasts while blocks stay at least half the threshold above the floor, so the unvoiced sounds and short pauses between words keep it, and ends after a run of other blocks as long as the hangover.
///
/// The noise floor is the level of the quietest block over the last few seconds, so it follows a changing background and assumes the signal is not voice all the time. Until the first block ends, it reads -100 dBFS, and the first blocks are taken as background.
///
/// Each frame costs a handful of 32-bit integer operations and one 32 × 32-bit multiply, also with [`Float`], where samples are converted to 16 bits first: NaN counts as silence and infinities as full scale. The decision runs once per block, also without floating point or division. Positions are counted in frames analyzed since creation or the last [`reset`](Self::reset).
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
    crossings: u32,
    level: i32,
    rate: u32,
    floor: i32,
    minima: [i32; FLOOR_SLOTS],
    current: i32,
    slot: u8,
    count: u32,
    score: i32,
    voice: bool,
    pending: u32,
    start: u64,
    frames: u64,
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

        while shift < DC_BITS && sample_rate >> (shift + 1) >= 160 {
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
            crossings: 0,
            level: MINIMUM_LEVEL,
            rate: 0,
            floor: MINIMUM_LEVEL,
            minima: [i32::MAX; FLOOR_SLOTS],
            current: i32::MAX,
            slot: 0,
            count: 0,
            score: 0,
            voice: false,
            pending: 0,
            start: 0,
            frames: 0,
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
    /// It is a running average over about four blocks, where a voiced-range block scores 128 at the threshold above the floor and 255 at twice the threshold, and other blocks score 0. It moves before [`is_voice`](Self::is_voice) changes.
    pub const fn score(&self) -> u8 {
        self.score as u8
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
        ((self.rate as u64 * self.sample_rate as u64 + self.length as u64 / 2) / self.length as u64)
            as u32
    }

    /// Returns the number of zero crossings in the last block.
    pub const fn crossings(&self) -> u32 {
        self.rate
    }

    /// Returns the number of frames analyzed.
    pub const fn frames(&self) -> u64 {
        self.frames
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
        let sign = filtered >> 31;

        self.energy += (magnitude * magnitude) as u64;
        self.crossings += (sign ^ self.sign) as u32 & 1;
        self.sign = sign;
        self.frames += 1;
        self.position += 1;

        if self.position >= self.length {
            self.finish();
        }
    }

    fn finish(&mut self) {
        let level = decibels(logarithm(self.energy) - self.logarithm).max(MINIMUM_LEVEL);
        let crossings = self.crossings;

        self.position = 0;
        self.energy = 0;
        self.crossings = 0;
        self.blocks += 1;
        self.level = level;
        self.rate = crossings;

        self.current = self.current.min(level);
        self.count += 1;

        if self.count >= self.window {
            self.minima[self.slot as usize] = self.current;
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

        let above = level - floor;
        let audible = level >= self.minimum;
        let voiced = audible
            && above >= self.threshold
            && crossings >= self.lowest
            && crossings <= self.highest;
        let target = if audible && crossings >= self.lowest && crossings <= self.highest {
            ((above.clamp(0, span(self.threshold)) * self.scale) >> 16).min(255)
        } else {
            0
        };

        self.score += (target - self.score) >> 2;

        let keeps = if self.voice {
            audible && 2 * above >= self.threshold
        } else {
            !voiced
        };

        if keeps {
            self.pending = 0;
            return;
        }

        if self.pending == 0 {
            self.start = self.frames - self.length as u64;
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
        let shift = MIX_SHIFTS[frame.len().min(8)];
        let sum = frame
            .iter()
            .fold(0i32, |sum, value| sum.wrapping_add(value >> 16));

        self.push((sum >> shift).clamp(-32768, 32767));
    }
}

impl Analyzer<Float> for VoiceActivityDetector {
    #[inline]
    fn analyze(&mut self, frame: &[f32]) {
        let shift = MIX_SHIFTS[frame.len().min(8)];
        let sum: f32 = frame.iter().sum();
        let mixed = sum * (32768 >> shift) as f32;

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
    let bend = ((fraction * (65536 - fraction)) >> 16) * 22713 >> 16;

    (exponent << 12) + ((fraction + bend) >> 4) as i32
}

const fn decibels(logarithm: i32) -> i32 {
    ((logarithm - (30 << 12)) * 3083) >> 14
}
