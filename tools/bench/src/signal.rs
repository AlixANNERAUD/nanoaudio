use std::f64::consts::TAU;

pub const SECONDS: u32 = 4;

const CHORDS: [[f64; 4]; 8] = [
    [220.00, 277.18, 329.63, 440.00],
    [196.00, 246.94, 293.66, 392.00],
    [174.61, 220.00, 261.63, 349.23],
    [164.81, 207.65, 246.94, 329.63],
    [146.83, 185.00, 220.00, 293.66],
    [164.81, 196.00, 246.94, 329.63],
    [174.61, 220.00, 261.63, 349.23],
    [196.00, 246.94, 293.66, 392.00],
];

const HARMONICS: [f64; 4] = [1.0, 0.45, 0.25, 0.12];

struct Random(u32);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f64 / (1u32 << 23) as f64 - 1.0
    }
}

pub fn generate(sample_rate: u32, channels: u8) -> Vec<i16> {
    let frames = (sample_rate * SECONDS) as usize;
    let rate = sample_rate as f64;
    let nyquist = rate / 2.0;
    let mut output = Vec::with_capacity(frames * channels as usize);
    let mut randoms: Vec<Random> = (0..channels as u32)
        .map(|channel| Random(0x1234_5678 ^ channel.wrapping_mul(0x9E37_79B9)))
        .collect();
    let mut noise = vec![0.0f64; channels as usize];
    let mut burst = vec![0.0f64; channels as usize];
    let hit = (rate / 4.0) as usize;

    for frame in 0..frames {
        let time = frame as f64 / rate;
        let chord = &CHORDS[(frame / (sample_rate as usize / 2)) % CHORDS.len()];
        let within = (frame % (sample_rate as usize / 2)) as f64 / rate;
        let envelope = 0.6 + 0.4 * (-within * 4.0).exp();

        for channel in 0..channels as usize {
            let random = &mut randoms[channel];
            let pan = if channel % 2 == 0 { 1.0 } else { 0.8 };
            let mut tone = 0.0;

            for (note, frequency) in chord.iter().enumerate() {
                let weight = if (note + channel) % 2 == 0 {
                    pan
                } else {
                    1.8 - pan
                };
                let vibrato = 1.0 + 0.003 * (TAU * 5.0 * time).sin();

                for (harmonic, amplitude) in HARMONICS.iter().enumerate() {
                    let partial = frequency * (harmonic + 1) as f64 * vibrato;

                    if partial < nyquist * 0.9 {
                        tone += weight * amplitude * (TAU * partial * time + channel as f64).sin();
                    }
                }
            }

            noise[channel] = 0.97 * noise[channel] + 0.03 * random.next();

            if frame % hit == 0 {
                burst[channel] = if (frame / hit).is_multiple_of(4) {
                    0.9
                } else {
                    0.4
                };
            }

            burst[channel] *= (-1.0 / (0.03 * rate)).exp();
            let transient = burst[channel] * random.next();
            let value = 0.11 * envelope * tone + 0.6 * noise[channel] + 0.35 * transient;

            output.push((value * 26_000.0).round().clamp(-32_768.0, 32_767.0) as i16);
        }
    }

    output
}
