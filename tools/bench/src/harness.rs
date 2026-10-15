use std::hint::black_box;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub struct Settings {
    pub runs: usize,
    pub target: Duration,
}

pub struct Measurement {
    pub repetitions: usize,
    pub median: f64,
    pub low: f64,
    pub high: f64,
    pub minimum: f64,
}

fn time<F: FnMut() -> usize>(function: &mut F, repetitions: usize) -> f64 {
    let start = Instant::now();

    for _ in 0..repetitions {
        black_box(function());
    }

    start.elapsed().as_nanos() as f64 / repetitions as f64
}

pub fn measure<F: FnMut() -> usize>(settings: Settings, mut function: F) -> Measurement {
    let single = time(&mut function, 1).max(1.0);
    let repetitions = ((settings.target.as_nanos() as f64 / single).ceil() as usize).max(1);

    time(&mut function, repetitions);

    let mut samples: Vec<f64> = (0..settings.runs)
        .map(|_| time(&mut function, repetitions))
        .collect();
    samples.sort_by(f64::total_cmp);

    let quantile =
        |fraction: f64| samples[((samples.len() - 1) as f64 * fraction).round() as usize];

    Measurement {
        repetitions,
        median: quantile(0.5),
        low: quantile(0.25),
        high: quantile(0.75),
        minimum: samples[0],
    }
}
