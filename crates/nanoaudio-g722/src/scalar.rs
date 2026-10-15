macro_rules! qmf_analysis {
    ($history:expr) => {{
        let (mut even, mut odd) = (0, 0);

        for (age, coefficient) in $crate::FILTER.iter().enumerate() {
            let product = coefficient * $history[23 - age];

            if age % 2 == 0 {
                even += product;
            } else {
                odd += product;
            }
        }

        (even, odd)
    }};
}

macro_rules! qmf_synthesis {
    ($differences:expr, $sums:expr) => {{
        let (mut first, mut second) = (0, 0);

        for age in 0..12 {
            first += $differences[11 - age] * $crate::FILTER[2 * age];
            second += $sums[11 - age] * $crate::FILTER[2 * age + 1];
        }

        (first, second)
    }};
}

macro_rules! low_interval {
    ($magnitude:expr, $scale:expr) => {
        (1..$crate::LOW_DECISION_LEVELS.len())
            .find(|interval| $magnitude < ($crate::LOW_DECISION_LEVELS[*interval] * $scale) >> 12)
            .unwrap_or($crate::LOW_DECISION_LEVELS.len()) as u8
    };
}

pub(crate) use {low_interval, qmf_analysis, qmf_synthesis};
