pub(crate) const HALF_LENGTH: usize = 32;
pub(crate) const PHASES: usize = 1 << PHASE_BITS;
pub(crate) const FRACTION_BITS: u32 = 16;
pub(crate) const LIMIT: u32 = (HALF_LENGTH * PHASES) as u32 * (1 << FRACTION_BITS);
pub(crate) const UNIT_INCREMENT: u32 = (PHASES as u32) << FRACTION_BITS;
pub(crate) const PHASE_BITS: u32 = 7;
pub(crate) const FIXED_BITS: u32 = 15;

const CUTOFF: f64 = 0.9;
const BETA: f64 = 10.0;
const PI: f64 = core::f64::consts::PI;

const fn sin_pi(x: f64) -> f64 {
    let turns = (x + 1.0) * 0.5;
    let mut whole = turns as i64 as f64;
    if whole > turns {
        whole -= 1.0;
    }
    let mut reduced = x - 2.0 * whole;
    if reduced > 0.5 {
        reduced = 1.0 - reduced;
    } else if reduced < -0.5 {
        reduced = -1.0 - reduced;
    }

    let angle = PI * reduced;
    let square = angle * angle;
    let mut term = angle;
    let mut sum = angle;
    let mut index = 1;

    while index < 16 {
        term = -term * square / ((2 * index) as f64 * (2 * index + 1) as f64);
        sum += term;
        index += 1;
    }

    sum
}

const fn square_root(value: f64) -> f64 {
    if value <= 0.0 {
        return 0.0;
    }

    let mut guess = if value > 1.0 { value } else { 1.0 };
    let mut index = 0;

    while index < 64 {
        guess = 0.5 * (guess + value / guess);
        index += 1;
    }

    guess
}

const fn bessel(x: f64) -> f64 {
    let half = x * 0.5;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut index = 1;

    while index < 64 {
        let factor = half / index as f64;
        term *= factor * factor;
        sum += term;
        index += 1;
    }

    sum
}

const fn prototype(index: usize) -> f64 {
    let position = index as f64 / PHASES as f64;
    let ratio = position / HALF_LENGTH as f64;
    let window = bessel(BETA * square_root(1.0 - ratio * ratio)) / bessel(BETA);
    let argument = CUTOFF * position;
    let sinc = if index == 0 {
        1.0
    } else {
        sin_pi(argument) / (PI * argument)
    };

    CUTOFF * sinc * window
}

pub(crate) type Table<T> = [[T; HALF_LENGTH]; PHASES + 1];

const fn float_table() -> Table<f32> {
    let mut table = [[0.0; HALF_LENGTH]; PHASES + 1];
    let mut phase = 0;

    while phase <= PHASES {
        let mut tap = 0;

        while tap < HALF_LENGTH {
            table[phase][tap] = prototype(tap * PHASES + phase) as f32;
            tap += 1;
        }

        phase += 1;
    }

    table
}

const fn fixed_table() -> Table<i16> {
    let mut table = [[0; HALF_LENGTH]; PHASES + 1];
    let mut phase = 0;

    while phase <= PHASES {
        let mut tap = 0;

        while tap < HALF_LENGTH {
            let scaled = prototype(tap * PHASES + phase) * (1 << FIXED_BITS) as f64;
            table[phase][tap] = if scaled < 0.0 {
                (scaled - 0.5) as i16
            } else {
                (scaled + 0.5) as i16
            };
            tap += 1;
        }

        phase += 1;
    }

    table
}

pub(crate) static FLOAT: Table<f32> = float_table();
pub(crate) static FIXED: Table<i16> = fixed_table();

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;

    #[test]
    fn sine_matches_the_standard_library() {
        for step in -4_000..=4_000 {
            let x = step as f64 / 37.0;
            assert!((sin_pi(x) - (PI * x).sin()).abs() < 1e-12, "{x}");
        }
    }

    #[test]
    fn helpers_match_the_standard_library() {
        for step in 0..=1_000 {
            let x = step as f64 / 100.0;
            assert!((square_root(x) - x.sqrt()).abs() < 1e-14, "{x}");
        }

        assert!((bessel(10.0) - 2_815.716_628_466_254).abs() < 1e-9);
    }

    #[test]
    fn prototype_is_a_windowed_sinc() {
        assert_eq!(FIXED[0][0], (CUTOFF * 32_768.0).round() as i16);
        assert!(FLOAT[PHASES][HALF_LENGTH - 1].abs() < 1e-5);
        assert_eq!(FLOAT[PHASES][3], FLOAT[0][4]);

        let crossing = (PHASES as f64 / CUTOFF).round() as usize;
        assert!(FLOAT[crossing - PHASES][1].abs() < 2e-3);
    }

    #[test]
    fn phases_sum_to_unity_and_leave_fixed_headroom() {
        for phase in 0..PHASES {
            let rows = [phase, PHASES - phase];
            let taps = rows
                .iter()
                .flat_map(|row| (0..HALF_LENGTH).map(move |tap| (*row, tap)));
            let sum: f64 = taps.clone().map(|(row, tap)| FLOAT[row][tap] as f64).sum();
            let fixed: i64 = taps.clone().map(|(row, tap)| FIXED[row][tap] as i64).sum();
            let absolute: i64 = taps.map(|(row, tap)| (FIXED[row][tap] as i64).abs()).sum();

            assert!((sum - 1.0).abs() < 1e-4, "{phase} {sum}");
            assert!((fixed - 32_768).abs() < 40, "{phase} {fixed}");
            assert!(absolute < 2 * 65_536, "{phase} {absolute}");

            for row in rows {
                let half: i64 = FIXED[row].iter().map(|tap| (*tap as i64).abs()).sum();
                assert!(half < 65_536, "{phase} {half}");
            }
        }
    }
}
