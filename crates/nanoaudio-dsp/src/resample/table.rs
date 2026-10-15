pub(crate) const HALF_LENGTH: usize = 32;
pub(crate) const PHASES: usize = 128;
pub(crate) const LENGTH: usize = HALF_LENGTH * PHASES + 1;
pub(crate) const FRACTION_BITS: u32 = 16;
pub(crate) const LIMIT: u32 = (HALF_LENGTH * PHASES) as u32 * (1 << FRACTION_BITS);
pub(crate) const UNIT_INCREMENT: u32 = (PHASES as u32) << FRACTION_BITS;
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

const fn float_table() -> [f32; LENGTH] {
    let mut table = [0.0; LENGTH];
    let mut index = 0;

    while index < LENGTH {
        table[index] = prototype(index) as f32;
        index += 1;
    }

    table
}

const fn fixed_table() -> [i16; LENGTH] {
    let mut table = [0; LENGTH];
    let mut index = 0;

    while index < LENGTH {
        let scaled = prototype(index) * (1 << FIXED_BITS) as f64;
        table[index] = if scaled < 0.0 {
            (scaled - 0.5) as i16
        } else {
            (scaled + 0.5) as i16
        };
        index += 1;
    }

    table
}

pub(crate) static FLOAT: [f32; LENGTH] = float_table();
pub(crate) static FIXED: [i16; LENGTH] = fixed_table();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_matches_reference() {
        for step in -400..=400 {
            let x = step as f64 / 37.0;
            let expected = libm_free_sin(PI * x);
            assert!((sin_pi(x) - expected).abs() < 1e-12, "{x}");
        }
    }

    fn libm_free_sin(angle: f64) -> f64 {
        let mut sum = 0.0;
        let mut term = angle;
        let mut index = 1;

        while index < 200 {
            sum += term;
            term = -term * angle * angle / ((2 * index) as f64 * (2 * index + 1) as f64);
            index += 1;
        }

        sum
    }

    #[test]
    fn prototype_is_a_windowed_sinc() {
        assert_eq!(FIXED[0], (CUTOFF * 32_768.0).round() as i16);
        assert!(FLOAT[LENGTH - 1].abs() < 1e-5);

        let crossing = (PHASES as f64 / CUTOFF).round() as usize;
        assert!(FLOAT[crossing].abs() < 2e-3);
    }

    #[test]
    fn phases_sum_to_unity_and_leave_fixed_headroom() {
        for phase in 0..PHASES {
            let mut sum = 0.0f64;
            let mut fixed = 0i64;
            let mut absolute = 0i64;
            let mut index = phase;

            while index < LENGTH {
                sum += FLOAT[index] as f64;
                fixed += FIXED[index] as i64;
                absolute += (FIXED[index] as i64).abs();
                index += PHASES;
            }

            let mut index = PHASES - phase;

            while index < LENGTH {
                sum += FLOAT[index] as f64;
                fixed += FIXED[index] as i64;
                absolute += (FIXED[index] as i64).abs();
                index += PHASES;
            }

            assert!((sum - 1.0).abs() < 1e-4, "{phase} {sum}");
            assert!((fixed - 32_768).abs() < 40, "{phase} {fixed}");
            assert!(absolute < 65_536, "{phase} {absolute}");
        }
    }
}
