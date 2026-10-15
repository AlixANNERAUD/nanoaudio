use core::marker::PhantomData;

use nanoaudio_core::codec::Progress;
use nanoaudio_core::{Error, Result, Sample};

use crate::{DefaultRepresentation, Fixed, Float, Representation, Stream};

mod table;

use table::{
    FIXED_BITS, FRACTION_BITS, HALF_LENGTH, LIMIT, PHASE_BITS, PHASES, Table, UNIT_INCREMENT,
};

type Value<S> = <<S as Stream>::Representation as Representation>::Value;

/// Linear interpolation between the two nearest input frames.
///
/// It costs a few operations per output sample and keeps two frames of history, but lets images and aliasing through, which is audible on music: use it for voice, effects and ratios close to 1, and [`Sinc`] otherwise. The output is delayed by one input frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Linear;

/// A 64-tap Kaiser-windowed sinc filter, for music.
///
/// The pass band is flat to within 0.001 dB up to 80 % of the lower of the two Nyquist frequencies, and everything above that Nyquist frequency is attenuated by at least 95 dB with [`Float`]. [`Fixed`] keeps 16 bits per sample and 16-bit coefficients, which limits it to about 80 dB. The output is delayed by [`Resampler::delay`] input frames: 34 when upsampling, about `34 × input rate / output rate` when downsampling.
///
/// Each output frame costs 64 multiply-adds per channel when upsampling, and `64 × input rate / output rate` when downsampling, plus one coefficient interpolation per tap shared by all channels. The coefficient table takes 16.1 KiB of flash with [`Float`] and 8.1 KiB with [`Fixed`], shared by every resampler.
///
/// `FRAMES` is the history kept per channel, which takes `4 × FRAMES` bytes per channel with [`Float`] and `2 × FRAMES` with [`Fixed`]. A ratio is accepted when `2 × ceil(32 × input rate / output rate) + 3` is at most `FRAMES`: 67 is enough for upsampling, and the default of 512 goes down to an output rate of 1/7.9 of the input, such as 48 kHz to 6.1 kHz.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Sinc<const FRAMES: usize = 512>;

mod sealed {
    use nanoaudio_core::Result;

    use super::Table;
    use crate::Representation;

    pub trait Arithmetic: Representation {
        type Stored: Copy;
        type Entry: Copy + 'static;
        type Coefficient: Copy;
        type Scale: Copy;

        const STORED_SILENCE: Self::Stored;
        const ZERO: Self::Coefficient;

        fn store(value: Self::Value) -> Self::Stored;
        fn load(stored: Self::Stored) -> Self::Value;
        fn interpolate(previous: Self::Value, current: Self::Value, fraction: u32) -> Self::Value;
        fn scale(input_rate: u32, output_rate: u32) -> Self::Scale;
        fn table() -> &'static Table<Self::Entry>;
        fn blend(low: Self::Entry, high: Self::Entry, weight: u32) -> Self::Coefficient;
        fn rescale(coefficient: Self::Coefficient, scale: Self::Scale) -> Self::Coefficient;
        fn combine(left: Self::Coefficient, right: Self::Coefficient) -> Self::Coefficient;
        fn accumulate(
            sum: Self::Coefficient,
            stored: Self::Stored,
            coefficient: Self::Coefficient,
        ) -> Self::Coefficient;
        fn finish(sum: Self::Coefficient, scale: Self::Scale) -> Self::Value;
    }

    pub struct LinearState<V, const CHANNELS: usize> {
        pub(crate) previous: [V; CHANNELS],
        pub(crate) current: [V; CHANNELS],
    }

    pub struct SincState<R: Arithmetic, const CHANNELS: usize, const FRAMES: usize> {
        pub(crate) history: [[R::Stored; CHANNELS]; FRAMES],
        pub(crate) end: usize,
        pub(crate) window: usize,
        pub(crate) keep: usize,
        pub(crate) increment: u32,
        pub(crate) scale: R::Scale,
        pub(crate) scaled: bool,
        pub(crate) unity: bool,
    }

    pub trait Kernel<R: Representation, const CHANNELS: usize> {
        type State;

        fn state() -> Self::State;
        fn configure(state: &mut Self::State, input_rate: u32, output_rate: u32) -> Result<u32>;
        fn clear(state: &mut Self::State);
        fn delay(state: &Self::State) -> usize;
        fn push(state: &mut Self::State, frame: &[R::Value; CHANNELS]);
        fn produce(state: &Self::State, fraction: u32, frame: &mut [R::Value]);
    }
}

use sealed::{Arithmetic, Kernel, LinearState, SincState};

/// An interpolation method for a [`Resampler`]: [`Linear`] or [`Sinc`].
///
/// It is implemented for both with [`Fixed`] and [`Float`], and cannot be implemented outside this crate.
pub trait Interpolation<R: Representation, const CHANNELS: usize>: Kernel<R, CHANNELS> {}

impl<R: Representation, const CHANNELS: usize, K: Kernel<R, CHANNELS>> Interpolation<R, CHANNELS>
    for K
{
}

impl Arithmetic for Fixed {
    type Stored = i16;
    type Entry = i16;
    type Coefficient = i32;
    type Scale = i32;

    const STORED_SILENCE: i16 = 0;
    const ZERO: i32 = 0;

    #[inline(always)]
    fn store(value: i32) -> i16 {
        (value.saturating_add(1 << 15) >> 16) as i16
    }

    #[inline(always)]
    fn load(stored: i16) -> i32 {
        (stored as i32) << 16
    }

    #[inline(always)]
    fn interpolate(previous: i32, current: i32, fraction: u32) -> i32 {
        let difference = ((current >> 1) - (previous >> 1)) >> 15;
        let step = difference * (fraction >> 17) as i32;

        previous.wrapping_add(step.wrapping_mul(2))
    }

    fn scale(input_rate: u32, output_rate: u32) -> i32 {
        (((output_rate as u64) << FIXED_BITS) / input_rate as u64) as i32
    }

    fn table() -> &'static Table<i16> {
        &table::FIXED
    }

    #[inline(always)]
    fn blend(low: i16, high: i16, weight: u32) -> i32 {
        let low = low as i32;
        let high = high as i32;

        low + (((high - low) * weight as i32 + (1 << (FRACTION_BITS - 1))) >> FRACTION_BITS)
    }

    #[inline(always)]
    fn rescale(coefficient: i32, scale: i32) -> i32 {
        (coefficient * scale + (1 << (FIXED_BITS - 1))) >> FIXED_BITS
    }

    #[inline(always)]
    fn combine(left: i32, right: i32) -> i32 {
        left.saturating_add(right)
    }

    #[inline(always)]
    fn accumulate(sum: i32, stored: i16, coefficient: i32) -> i32 {
        sum + stored as i32 * coefficient
    }

    #[inline(always)]
    fn finish(sum: i32, _: i32) -> i32 {
        sum.clamp(i32::MIN >> 1, i32::MAX >> 1) << 1
    }
}

impl Arithmetic for Float {
    type Stored = f32;
    type Entry = f32;
    type Coefficient = f32;
    type Scale = f32;

    const STORED_SILENCE: f32 = 0.0;
    const ZERO: f32 = 0.0;

    #[inline(always)]
    fn store(value: f32) -> f32 {
        value
    }

    #[inline(always)]
    fn load(stored: f32) -> f32 {
        stored
    }

    #[inline(always)]
    fn interpolate(previous: f32, current: f32, fraction: u32) -> f32 {
        previous + (current - previous) * ((fraction >> 8) as f32 * (1.0 / 16_777_216.0))
    }

    fn scale(input_rate: u32, output_rate: u32) -> f32 {
        (output_rate as f64 / input_rate as f64) as f32
    }

    fn table() -> &'static Table<f32> {
        &table::FLOAT
    }

    #[inline(always)]
    fn blend(low: f32, high: f32, weight: u32) -> f32 {
        low + (high - low) * (weight as f32 * (1.0 / 65_536.0))
    }

    #[inline(always)]
    fn rescale(coefficient: f32, _: f32) -> f32 {
        coefficient
    }

    #[inline(always)]
    fn combine(left: f32, right: f32) -> f32 {
        left + right
    }

    #[inline(always)]
    fn accumulate(sum: f32, stored: f32, coefficient: f32) -> f32 {
        sum + stored * coefficient
    }

    #[inline(always)]
    fn finish(sum: f32, scale: f32) -> f32 {
        sum * scale
    }
}

impl<R: Arithmetic, const CHANNELS: usize> Kernel<R, CHANNELS> for Linear {
    type State = LinearState<R::Value, CHANNELS>;

    fn state() -> Self::State {
        LinearState {
            previous: [R::SILENCE; CHANNELS],
            current: [R::SILENCE; CHANNELS],
        }
    }

    fn configure(_: &mut Self::State, _: u32, _: u32) -> Result<u32> {
        Ok(0)
    }

    fn clear(state: &mut Self::State) {
        *state = <Self as Kernel<R, CHANNELS>>::state();
    }

    fn delay(_: &Self::State) -> usize {
        1
    }

    #[inline(always)]
    fn push(state: &mut Self::State, frame: &[R::Value; CHANNELS]) {
        state.previous = state.current;
        state.current = *frame;
    }

    #[inline(always)]
    fn produce(state: &Self::State, fraction: u32, frame: &mut [R::Value]) {
        for ((output, previous), current) in
            frame.iter_mut().zip(&state.previous).zip(&state.current)
        {
            *output = R::interpolate(*previous, *current, fraction);
        }
    }
}

impl<R: Arithmetic, const CHANNELS: usize, const FRAMES: usize> SincState<R, CHANNELS, FRAMES> {
    fn reserve(&mut self, keep: usize) {
        self.keep = keep;

        if self.end < keep {
            let missing = keep - self.end;
            self.history.copy_within(..self.end, missing);
            self.history[..missing].fill([R::STORED_SILENCE; CHANNELS]);
            self.end = keep;
        }
    }

    #[inline(always)]
    fn coefficient(&self, position: u32) -> R::Coefficient {
        let table = R::table();
        let phase = (position >> FRACTION_BITS) as usize & (PHASES - 1);
        let tap = (position >> (FRACTION_BITS + PHASE_BITS)) as usize;
        let weight = position & ((1 << FRACTION_BITS) - 1);

        R::rescale(
            R::blend(table[phase][tap], table[phase + 1][tap], weight),
            self.scale,
        )
    }

    #[inline(always)]
    fn walk<'a>(
        &self,
        frames: impl Iterator<Item = &'a [R::Stored; CHANNELS]>,
        mut position: u32,
        sums: &mut [R::Coefficient; CHANNELS],
    ) where
        R::Stored: 'a,
    {
        for frame in frames {
            if position >= LIMIT {
                break;
            }

            let coefficient = self.coefficient(position);

            for (sum, value) in sums.iter_mut().zip(frame) {
                *sum = R::accumulate(*sum, *value, coefficient);
            }

            position += self.increment;
        }
    }

    #[inline(always)]
    fn downsample(&self, center: usize, fraction: u32, sums: &mut [R::Coefficient; CHANNELS]) {
        let left = ((fraction as u64 * self.increment as u64) >> 32) as u32;
        let (past, future) = self.history[..self.end].split_at(center + 1);

        let mut right = [R::ZERO; CHANNELS];

        self.walk(past.iter().rev(), left, sums);
        self.walk(future.iter(), self.increment - left, &mut right);

        for (sum, right) in sums.iter_mut().zip(right) {
            *sum = R::combine(*sum, right);
        }
    }

    #[inline(always)]
    fn blend_row(
        low: &[R::Entry; HALF_LENGTH],
        high: &[R::Entry; HALF_LENGTH],
        weight: u32,
    ) -> [R::Coefficient; HALF_LENGTH] {
        core::array::from_fn(|tap| R::blend(low[tap], high[tap], weight))
    }

    #[inline(always)]
    fn upsample(&self, center: usize, fraction: u32, sums: &mut [R::Coefficient; CHANNELS]) {
        let table = R::table();
        let position = fraction >> (32 - PHASE_BITS - FRACTION_BITS);
        let phase = (position >> FRACTION_BITS) as usize;
        let weight = position & ((1 << FRACTION_BITS) - 1);
        let (row, opposite) = if weight == 0 {
            (PHASES - phase, 0)
        } else {
            (PHASES - 1 - phase, (1 << FRACTION_BITS) - weight)
        };
        let left = Self::blend_row(&table[phase], &table[phase + 1], weight);
        let right = Self::blend_row(&table[row], &table[(row + 1).min(PHASES)], opposite);

        let past = &self.history[center + 1 - HALF_LENGTH..=center];
        let future = &self.history[center + 1..center + 1 + HALF_LENGTH];
        let mut lanes = [[R::ZERO; CHANNELS]; 4];

        for (coefficients, frames) in left
            .as_chunks::<4>()
            .0
            .iter()
            .zip(past.as_rchunks::<4>().1.iter().rev())
        {
            for (lane, (coefficient, frame)) in lanes
                .iter_mut()
                .zip(coefficients.iter().zip(frames.iter().rev()))
            {
                for (sum, value) in lane.iter_mut().zip(frame) {
                    *sum = R::accumulate(*sum, *value, *coefficient);
                }
            }
        }

        for (coefficients, frames) in right
            .as_chunks::<4>()
            .0
            .iter()
            .zip(future.as_chunks::<4>().0)
        {
            for (lane, (coefficient, frame)) in
                lanes.iter_mut().zip(coefficients.iter().zip(frames))
            {
                for (sum, value) in lane.iter_mut().zip(frame) {
                    *sum = R::accumulate(*sum, *value, *coefficient);
                }
            }
        }

        for (channel, sum) in sums.iter_mut().enumerate() {
            *sum = R::combine(
                R::combine(lanes[0][channel], lanes[1][channel]),
                R::combine(lanes[2][channel], lanes[3][channel]),
            );
        }
    }
}

impl<R: Arithmetic, const CHANNELS: usize, const FRAMES: usize> Kernel<R, CHANNELS>
    for Sinc<FRAMES>
{
    type State = SincState<R, CHANNELS, FRAMES>;

    fn state() -> Self::State {
        SincState {
            history: [[R::STORED_SILENCE; CHANNELS]; FRAMES],
            end: 0,
            window: 0,
            keep: 0,
            increment: UNIT_INCREMENT,
            scale: R::scale(1, 1),
            scaled: false,
            unity: false,
        }
    }

    fn configure(state: &mut Self::State, input_rate: u32, output_rate: u32) -> Result<u32> {
        let downsampling = output_rate < input_rate;
        let reach = if downsampling {
            (HALF_LENGTH as u64 * input_rate as u64).div_ceil(output_rate as u64) as usize
        } else {
            HALF_LENGTH
        };
        let required = reach + 1;
        let (window, growth) = if state.window == 0 {
            (required + 1, 0)
        } else if required > state.window {
            (required + 1, required + 1 - state.window)
        } else {
            (state.window, 0)
        };
        let keep = window + reach;

        if keep >= FRAMES {
            return Err(Error::InvalidParameter);
        }

        state.window = window;
        state.increment = if downsampling {
            ((UNIT_INCREMENT as u64 * output_rate as u64) / input_rate as u64) as u32
        } else {
            UNIT_INCREMENT
        };
        state.scale = R::scale(input_rate, output_rate.min(input_rate));
        state.scaled = downsampling;
        state.unity = input_rate == output_rate;
        state.reserve(keep);

        Ok(growth as u32)
    }

    fn clear(state: &mut Self::State) {
        state.history = [[R::STORED_SILENCE; CHANNELS]; FRAMES];
        state.end = state.keep;
    }

    fn delay(state: &Self::State) -> usize {
        state.window
    }

    #[inline]
    fn push(state: &mut Self::State, frame: &[R::Value; CHANNELS]) {
        if state.end == FRAMES {
            state.history.copy_within(FRAMES - state.keep..FRAMES, 0);
            state.end = state.keep;
        }

        state.history[state.end] = frame.map(R::store);
        state.end += 1;
    }

    #[inline]
    fn produce(state: &Self::State, fraction: u32, frame: &mut [R::Value]) {
        let center = state.end - 1 - state.window;

        if state.unity {
            for (output, stored) in frame.iter_mut().zip(&state.history[center]) {
                *output = R::load(*stored);
            }

            return;
        }

        let mut sums = [R::ZERO; CHANNELS];

        if state.scaled {
            state.downsample(center, fraction, &mut sums);
        } else {
            state.upsample(center, fraction, &mut sums);
        }

        for (output, sum) in frame.iter_mut().zip(sums) {
            *output = R::finish(sum, state.scale);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Clock {
    input_rate: u32,
    output_rate: u32,
    step: u32,
    step_fraction: u32,
    step_error: u32,
    fraction: u32,
    error: u32,
    pending: u32,
}

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }

    left
}

impl Clock {
    fn new(input_rate: u32, output_rate: u32) -> Result<Self> {
        if input_rate == 0 || output_rate == 0 {
            return Err(Error::InvalidParameter);
        }

        let divisor = greatest_common_divisor(input_rate, output_rate);
        let input_rate = input_rate / divisor;
        let output_rate = output_rate / divisor;
        let remainder = ((input_rate % output_rate) as u64) << 32;

        Ok(Self {
            input_rate,
            output_rate,
            step: input_rate / output_rate,
            step_fraction: (remainder / output_rate as u64) as u32,
            step_error: (remainder % output_rate as u64) as u32,
            fraction: 0,
            error: 0,
            pending: 1,
        })
    }

    fn retime(&mut self, previous: &Self) {
        self.fraction = previous.fraction;
        self.error = ((previous.error as u64 * self.output_rate as u64)
            / previous.output_rate as u64) as u32;
        self.pending = previous.pending;
    }

    #[inline(always)]
    fn advance(&mut self) {
        let (fraction, carry) = self.fraction.overflowing_add(self.step_fraction);
        let (error, overflow) = self.error.overflowing_add(self.step_error);
        let (error, borrow) = if overflow || error >= self.output_rate {
            (error.wrapping_sub(self.output_rate), true)
        } else {
            (error, false)
        };
        let (fraction, borrow_carry) = fraction.overflowing_add(borrow as u32);

        self.fraction = fraction;
        self.error = error;
        self.pending += self.step + carry as u32 + borrow_carry as u32;
    }

    fn phase(&self) -> u128 {
        ((self.fraction as u128) * self.output_rate as u128) + self.error as u128
    }

    fn input_frames(&self, output_frames: usize) -> usize {
        if output_frames == 0 {
            return 0;
        }

        let numerator =
            self.phase() + (output_frames as u128 - 1) * ((self.input_rate as u128) << 32);
        let whole = numerator / ((self.output_rate as u128) << 32);

        (self.pending as u128 + whole).min(usize::MAX as u128) as usize
    }

    fn output_frames(&self, input_frames: usize) -> usize {
        let pending = self.pending as usize;

        if input_frames < pending {
            return 0;
        }

        let available = ((input_frames - pending) as u128 + 1) * ((self.output_rate as u128) << 32)
            - self.phase();

        available
            .div_ceil((self.input_rate as u128) << 32)
            .min(usize::MAX as u128) as usize
    }
}

/// A streaming sample rate converter that does not allocate.
///
/// It converts interleaved audio with up to `CHANNELS` channels, with the interpolation `K` ([`Linear`] or [`Sinc`]) in the representation `R`. The ratio between the two rates is kept exactly, so `n` input frames always give `ceil(n × output rate / input rate)` output frames, whatever the size of the buffers they come in. Feed it one buffer at a time with [`process`](Self::process) or the [`Stream::resample`] stage. In an audio callback, [`input_frames`](Self::input_frames) says how much input the next output buffer needs.
///
/// [`set_rates`](Self::set_rates) changes the ratio between two buffers without a click, to follow a device clock that drifts from its nominal rate.
///
/// ```
/// use nanoaudio_dsp::{Resampler, Sinc};
///
/// let mut resampler = Resampler::<Sinc>::new(2, 44_100, 48_000).unwrap();
/// let input = [0i16; 2 * 441];
/// let mut output = [0i16; 2 * 480];
///
/// let progress = resampler.process(&input, &mut output);
///
/// assert_eq!(progress.consumed, input.len());
/// assert_eq!(progress.produced, output.len());
/// ```
pub struct Resampler<K, R = DefaultRepresentation, const CHANNELS: usize = 2>
where
    R: Representation,
    K: Interpolation<R, CHANNELS>,
{
    state: K::State,
    clock: Clock,
    channels: u8,
    representation: PhantomData<R>,
}

impl<K, R, const CHANNELS: usize> Resampler<K, R, CHANNELS>
where
    R: Representation,
    K: Interpolation<R, CHANNELS>,
{
    /// Creates a resampler for `channels` channels, from `input_rate` to `output_rate` in Hz.
    ///
    /// Returns [`Error::TooManyChannels`] if `channels` is 0 or above `CHANNELS`, and [`Error::InvalidParameter`] if a rate is 0 or the ratio needs more history than a [`Sinc`] keeps.
    pub fn new(channels: u8, input_rate: u32, output_rate: u32) -> Result<Self> {
        if channels == 0 || channels as usize > CHANNELS {
            return Err(Error::TooManyChannels);
        }

        let clock = Clock::new(input_rate, output_rate)?;
        let mut state = K::state();
        K::configure(&mut state, clock.input_rate, clock.output_rate)?;

        Ok(Self {
            state,
            clock,
            channels,
            representation: PhantomData,
        })
    }

    /// Changes the ratio, keeping the position in the stream so the output stays continuous.
    ///
    /// Only the ratio matters, so rates can be scaled up for a finer adjustment: `set_rates(4_410_000, 4_800_013)` plays 44.1 kHz audio on a 48 kHz device that runs 2.7 ppm fast. Returns [`Error::InvalidParameter`] like [`new`](Self::new), and keeps the previous ratio then.
    ///
    /// With [`Sinc`], a ratio that lowers the output rate by more than about 3 % at once lengthens the delay, by holding back the next output for a few input frames.
    pub fn set_rates(&mut self, input_rate: u32, output_rate: u32) -> Result<()> {
        let mut clock = Clock::new(input_rate, output_rate)?;
        let growth = K::configure(&mut self.state, clock.input_rate, clock.output_rate)?;

        clock.retime(&self.clock);
        clock.pending += growth;
        self.clock = clock;

        Ok(())
    }

    /// Returns the ratio as an input and an output rate, reduced to lowest terms.
    pub fn rates(&self) -> (u32, u32) {
        (self.clock.input_rate, self.clock.output_rate)
    }

    /// Returns the number of channels.
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Returns how far the output lags behind the input, in input frames.
    ///
    /// To get the end of a stream out, feed this many frames of silence after it.
    pub fn delay(&self) -> usize {
        K::delay(&self.state)
    }

    /// Returns how many input frames the next `output_frames` output frames need.
    ///
    /// Giving [`process`](Self::process) exactly this many input frames and room for `output_frames` produces all of them and consumes all the input.
    pub fn input_frames(&self, output_frames: usize) -> usize {
        self.clock.input_frames(output_frames)
    }

    /// Returns how many output frames the next `input_frames` input frames give, with enough room for the output.
    pub fn output_frames(&self, input_frames: usize) -> usize {
        self.clock.output_frames(input_frames)
    }

    /// Forgets the history and goes back to the start of a stream, keeping the ratio.
    pub fn reset(&mut self) {
        K::clear(&mut self.state);
        self.clock.fraction = 0;
        self.clock.error = 0;
        self.clock.pending = 1;
    }

    /// Converts interleaved samples from `input` into `output`, and returns how many samples were consumed and produced.
    ///
    /// It takes every input frame the next output frame needs, then writes that frame, until the input runs out or `output` is full. With room for [`output_frames`](Self::output_frames) of the input, all of it is consumed. Input that was not consumed is still needed: pass it again with the next call. Incomplete frames at the end of either slice are ignored.
    pub fn process<I: Sample, O: Sample>(&mut self, input: &[I], output: &mut [O]) -> Progress {
        let channels = self.channels as usize;
        let mut frames = input.chunks_exact(channels);
        let mut outputs = output.chunks_exact_mut(channels);
        let mut consumed = 0;
        let mut produced = 0;

        loop {
            while self.clock.pending > 0 {
                let Some(frame) = frames.next() else {
                    return Progress::new(consumed, produced);
                };
                let values = core::array::from_fn(|channel| {
                    frame
                        .get(channel)
                        .map_or(R::SILENCE, |sample| R::from_sample(*sample))
                });

                K::push(&mut self.state, &values);
                self.clock.pending -= 1;
                consumed += channels;
            }

            let Some(output) = outputs.next() else {
                return Progress::new(consumed, produced);
            };
            let mut values = [R::SILENCE; CHANNELS];
            K::produce(&self.state, self.clock.fraction, &mut values[..channels]);

            for (sample, value) in output.iter_mut().zip(values) {
                *sample = R::to_sample(value);
            }

            self.clock.advance();
            produced += channels;
        }
    }

    #[inline]
    fn next_frame<St>(&mut self, stream: &mut St, frame: &mut [R::Value]) -> bool
    where
        St: Stream<Representation = R>,
    {
        let channels = self.channels as usize;

        while self.clock.pending > 0 {
            let mut values = [R::SILENCE; CHANNELS];

            if !stream.next_frame(&mut values[..channels]) {
                return false;
            }

            K::push(&mut self.state, &values);
            self.clock.pending -= 1;
        }

        K::produce(&self.state, self.clock.fraction, frame);
        self.clock.advance();
        true
    }
}

/// The stage returned by [`Stream::resample`].
///
/// It ends when the stream before it ends, and the [`Resampler`] keeps its place, so the next buffer can go through a new pipeline with the same resampler.
pub struct Resample<'a, St, K, const CHANNELS: usize>
where
    St: Stream,
    K: Interpolation<St::Representation, CHANNELS>,
{
    stream: St,
    resampler: &'a mut Resampler<K, St::Representation, CHANNELS>,
}

impl<'a, St, K, const CHANNELS: usize> Resample<'a, St, K, CHANNELS>
where
    St: Stream,
    K: Interpolation<St::Representation, CHANNELS>,
{
    pub(crate) fn new(
        stream: St,
        resampler: &'a mut Resampler<K, St::Representation, CHANNELS>,
    ) -> Result<Self> {
        if stream.channels() != resampler.channels() {
            return Err(Error::InvalidParameter);
        }

        Ok(Self { stream, resampler })
    }
}

impl<St, K, const CHANNELS: usize> Stream for Resample<'_, St, K, CHANNELS>
where
    St: Stream,
    K: Interpolation<St::Representation, CHANNELS>,
{
    type Representation = St::Representation;

    fn channels(&self) -> u8 {
        self.resampler.channels
    }

    #[inline]
    fn next_frame(&mut self, frame: &mut [Value<Self>]) -> bool {
        self.resampler.next_frame(&mut self.stream, frame)
    }
}
