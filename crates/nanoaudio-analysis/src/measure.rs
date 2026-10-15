use nanoaudio_dsp::{Fixed, Float, Representation};

/// Level helpers on a [`Representation`], so an analyzer can be written once for [`Fixed`] and [`Float`].
///
/// Values compare by level, and [`Representation::to_sample`] converts results to a common domain such as `i32` or `f32`.
pub trait Measure: Representation<Value: PartialOrd> {
    /// The largest value a sample converts to, full scale in the positive direction.
    const FULL_SCALE: Self::Value;

    /// Returns the absolute value of `value`, saturating at [`FULL_SCALE`](Self::FULL_SCALE) in fixed point.
    fn magnitude(value: Self::Value) -> Self::Value;
}

impl Measure for Fixed {
    const FULL_SCALE: i32 = i32::MAX;

    #[inline]
    fn magnitude(value: i32) -> i32 {
        value.saturating_abs()
    }
}

impl Measure for Float {
    const FULL_SCALE: f32 = 1.0;

    #[inline]
    fn magnitude(value: f32) -> f32 {
        value.abs()
    }
}

#[cfg(test)]
mod tests {
    use nanoaudio_core::I24;

    use super::*;

    fn magnitudes<R: Measure>() {
        assert_eq!(
            R::to_sample::<i16>(R::magnitude(R::from_sample(-1234i16))),
            1234
        );
        assert_eq!(
            R::to_sample::<i16>(R::magnitude(R::from_sample(i16::MIN))),
            i16::MAX
        );
        assert_eq!(
            R::to_sample::<I24>(R::magnitude(R::from_sample(I24::MIN))),
            I24::MAX
        );
        assert_eq!(R::to_sample::<i32>(R::FULL_SCALE), i32::MAX);
        assert!(R::magnitude(R::from_sample(i32::MIN)) >= R::FULL_SCALE);
        assert!(R::magnitude(R::from_sample(i16::MAX)) < R::FULL_SCALE);
        assert!(R::magnitude(R::SILENCE) == R::SILENCE);
    }

    #[test]
    fn fixed() {
        magnitudes::<Fixed>();
    }

    #[test]
    fn float() {
        magnitudes::<Float>();
    }
}
