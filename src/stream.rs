use core::marker::PhantomData;

use crate::{DefaultRepresentation, Error, Format, Representation, Result, Sample, Volume};

pub const MAXIMUM_CHANNELS: usize = 8;

type Value<S> = <<S as Stream>::Representation as Representation>::Value;

type Frame<R> = [<R as Representation>::Value; MAXIMUM_CHANNELS];

fn silent_frame<R: Representation>() -> Frame<R> {
    [R::SILENCE; MAXIMUM_CHANNELS]
}

fn check_channels(channels: u8) -> Result<u8> {
    if channels == 0 || channels as usize > MAXIMUM_CHANNELS {
        Err(Error::TooManyChannels)
    } else {
        Ok(channels)
    }
}

pub trait Stream {
    type Representation: Representation;

    fn channels(&self) -> u8;

    fn next_frame(&mut self, frame: &mut [Value<Self>]) -> bool;

    fn volume(self, volume: Volume) -> Amplify<Self>
    where
        Self: Sized,
    {
        Amplify {
            gain: Self::Representation::gain(volume),
            mute: volume.is_mute(),
            stream: self,
        }
    }

    fn to_channels(self, channels: u8) -> Result<Remap<Self>>
    where
        Self: Sized,
    {
        Ok(Remap {
            channels: check_channels(channels)?,
            stream: self,
        })
    }

    fn mix<O>(self, other: O) -> Mix<Self, O>
    where
        Self: Sized,
        O: Stream<Representation = Self::Representation>,
    {
        Mix {
            left: self,
            right: other,
        }
    }

    fn map<F>(self, function: F) -> Map<Self, F>
    where
        Self: Sized,
        F: FnMut(Value<Self>) -> Value<Self>,
    {
        Map {
            stream: self,
            function,
        }
    }

    fn write_samples<S: Sample>(&mut self, output: &mut [S]) -> usize
    where
        Self: Sized,
    {
        process_samples(self, output, |_, value| value)
    }

    fn mix_into_samples<S: Sample>(&mut self, output: &mut [S]) -> usize
    where
        Self: Sized,
    {
        process_samples(self, output, |existing, value| {
            Self::Representation::add(Self::Representation::from_sample(existing), value)
        })
    }

    fn write_bytes(&mut self, output: &mut [u8], format: Format) -> usize
    where
        Self: Sized,
    {
        match format {
            Format::U8 => process_bytes::<Self, u8, _>(self, output, |_, value| value),
            Format::I16 => process_bytes::<Self, i16, _>(self, output, |_, value| value),
            Format::I32 => process_bytes::<Self, i32, _>(self, output, |_, value| value),
            Format::F32 => process_bytes::<Self, f32, _>(self, output, |_, value| value),
        }
    }

    fn mix_into_bytes(&mut self, output: &mut [u8], format: Format) -> usize
    where
        Self: Sized,
    {
        match format {
            Format::U8 => process_bytes::<Self, u8, _>(self, output, add_existing::<Self, u8>),
            Format::I16 => process_bytes::<Self, i16, _>(self, output, add_existing::<Self, i16>),
            Format::I32 => process_bytes::<Self, i32, _>(self, output, add_existing::<Self, i32>),
            Format::F32 => process_bytes::<Self, f32, _>(self, output, add_existing::<Self, f32>),
        }
    }
}

fn add_existing<St: Stream, S: Sample>(existing: S, value: Value<St>) -> Value<St> {
    St::Representation::add(St::Representation::from_sample(existing), value)
}

fn process_samples<St, S, F>(stream: &mut St, output: &mut [S], mut combine: F) -> usize
where
    St: Stream,
    S: Sample,
    F: FnMut(S, Value<St>) -> Value<St>,
{
    let channels = stream.channels() as usize;
    let mut frame = silent_frame::<St::Representation>();
    let mut frames = 0;

    for output in output.chunks_exact_mut(channels) {
        if !stream.next_frame(&mut frame[..channels]) {
            break;
        }

        for (output, value) in output.iter_mut().zip(&frame[..channels]) {
            *output = St::Representation::to_sample(combine(*output, *value));
        }

        frames += 1;
    }

    frames
}

fn process_bytes<St, S, F>(stream: &mut St, output: &mut [u8], mut combine: F) -> usize
where
    St: Stream,
    S: Sample,
    F: FnMut(S, Value<St>) -> Value<St>,
{
    let channels = stream.channels() as usize;
    let mut frame = silent_frame::<St::Representation>();
    let mut frames = 0;

    for output in output.chunks_exact_mut(channels * S::SIZE) {
        if !stream.next_frame(&mut frame[..channels]) {
            break;
        }

        for (output, value) in output.chunks_exact_mut(S::SIZE).zip(&frame[..channels]) {
            St::Representation::to_sample::<S>(combine(S::read_le(output), *value))
                .write_le(output);
        }

        frames += 1;
    }

    frames
}

pub struct SampleSource<'a, S: Sample, R: Representation = DefaultRepresentation> {
    samples: &'a [S],
    channels: u8,
    representation: PhantomData<R>,
}

impl<'a, S: Sample, R: Representation> SampleSource<'a, S, R> {
    pub fn new(samples: &'a [S], channels: u8) -> Result<Self> {
        Ok(Self {
            samples,
            channels: check_channels(channels)?,
            representation: PhantomData,
        })
    }

    pub fn remaining(&self) -> &'a [S] {
        self.samples
    }
}

impl<S: Sample, R: Representation> Stream for SampleSource<'_, S, R> {
    type Representation = R;

    fn channels(&self) -> u8 {
        self.channels
    }

    fn next_frame(&mut self, frame: &mut [R::Value]) -> bool {
        let Some((current, remaining)) = self.samples.split_at_checked(self.channels as usize)
        else {
            return false;
        };

        for (value, sample) in frame.iter_mut().zip(current) {
            *value = R::from_sample(*sample);
        }

        self.samples = remaining;
        true
    }
}

pub struct ByteSource<'a, R: Representation = DefaultRepresentation> {
    bytes: &'a [u8],
    format: Format,
    channels: u8,
    representation: PhantomData<R>,
}

impl<'a, R: Representation> ByteSource<'a, R> {
    pub fn new(bytes: &'a [u8], format: Format, channels: u8) -> Result<Self> {
        Ok(Self {
            bytes,
            format,
            channels: check_channels(channels)?,
            representation: PhantomData,
        })
    }

    pub fn remaining(&self) -> &'a [u8] {
        self.bytes
    }

    fn read(format: Format, bytes: &[u8]) -> R::Value {
        match format {
            Format::U8 => R::from_sample(u8::read_le(bytes)),
            Format::I16 => R::from_sample(i16::read_le(bytes)),
            Format::I32 => R::from_sample(i32::read_le(bytes)),
            Format::F32 => R::from_sample(f32::read_le(bytes)),
        }
    }
}

impl<R: Representation> Stream for ByteSource<'_, R> {
    type Representation = R;

    fn channels(&self) -> u8 {
        self.channels
    }

    fn next_frame(&mut self, frame: &mut [R::Value]) -> bool {
        let size = self.format.sample_size();
        let Some((current, remaining)) = self.bytes.split_at_checked(self.channels as usize * size)
        else {
            return false;
        };

        for (value, bytes) in frame.iter_mut().zip(current.chunks_exact(size)) {
            *value = Self::read(self.format, bytes);
        }

        self.bytes = remaining;
        true
    }
}

pub struct Amplify<St: Stream> {
    stream: St,
    gain: <St::Representation as Representation>::Gain,
    mute: bool,
}

impl<St: Stream> Stream for Amplify<St> {
    type Representation = St::Representation;

    fn channels(&self) -> u8 {
        self.stream.channels()
    }

    fn next_frame(&mut self, frame: &mut [Value<Self>]) -> bool {
        if !self.stream.next_frame(frame) {
            return false;
        }

        if self.mute {
            frame.fill(St::Representation::SILENCE);
        } else {
            for value in frame {
                *value = St::Representation::amplify(*value, self.gain);
            }
        }

        true
    }
}

pub struct Remap<St: Stream> {
    stream: St,
    channels: u8,
}

impl<St: Stream> Stream for Remap<St> {
    type Representation = St::Representation;

    fn channels(&self) -> u8 {
        self.channels
    }

    fn next_frame(&mut self, frame: &mut [Value<Self>]) -> bool {
        let input_channels = self.stream.channels() as usize;
        let mut input = silent_frame::<St::Representation>();

        if !self.stream.next_frame(&mut input[..input_channels]) {
            return false;
        }

        let input = &input[..input_channels];

        if input_channels == 1 {
            frame.fill(input[0]);
        } else if frame.len() == 1 {
            frame[0] = St::Representation::average(input);
        } else {
            for (index, value) in frame.iter_mut().enumerate() {
                *value = input
                    .get(index)
                    .copied()
                    .unwrap_or(St::Representation::SILENCE);
            }
        }

        true
    }
}

pub struct Mix<L: Stream, Ri: Stream> {
    left: L,
    right: Ri,
}

impl<L, Ri> Stream for Mix<L, Ri>
where
    L: Stream,
    Ri: Stream<Representation = L::Representation>,
{
    type Representation = L::Representation;

    fn channels(&self) -> u8 {
        self.left.channels().max(self.right.channels())
    }

    fn next_frame(&mut self, frame: &mut [Value<Self>]) -> bool {
        let mut right = silent_frame::<L::Representation>();
        let left_channels = self.left.channels() as usize;
        let right_channels = self.right.channels() as usize;

        frame.fill(L::Representation::SILENCE);

        let left_available = self.left.next_frame(&mut frame[..left_channels]);
        let right_available = self.right.next_frame(&mut right[..right_channels]);

        if !left_available {
            frame.fill(L::Representation::SILENCE);
        }

        if !right_available {
            return left_available;
        }

        for (value, right) in frame.iter_mut().zip(&right[..right_channels]) {
            *value = L::Representation::add(*value, *right);
        }

        true
    }
}

pub struct Map<St: Stream, F> {
    stream: St,
    function: F,
}

impl<St, F> Stream for Map<St, F>
where
    St: Stream,
    F: FnMut(Value<St>) -> Value<St>,
{
    type Representation = St::Representation;

    fn channels(&self) -> u8 {
        self.stream.channels()
    }

    fn next_frame(&mut self, frame: &mut [Value<Self>]) -> bool {
        if !self.stream.next_frame(frame) {
            return false;
        }

        for value in frame {
            *value = (self.function)(*value);
        }

        true
    }
}

pub fn source(bytes: &[u8], format: Format, channels: u8) -> Result<ByteSource<'_>> {
    ByteSource::new(bytes, format, channels)
}

pub fn source_using<R: Representation>(
    bytes: &[u8],
    format: Format,
    channels: u8,
) -> Result<ByteSource<'_, R>> {
    ByteSource::new(bytes, format, channels)
}

pub fn samples<S: Sample>(samples: &[S], channels: u8) -> Result<SampleSource<'_, S>> {
    SampleSource::new(samples, channels)
}

pub fn samples_using<R: Representation, S: Sample>(
    samples: &[S],
    channels: u8,
) -> Result<SampleSource<'_, S, R>> {
    SampleSource::new(samples, channels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Fixed, Float};

    fn chain<R: Representation>() {
        let input = [0x00u8, 0x80, 0xC0];
        let mut output = [0u8; 12];

        let frames = source_using::<R>(&input, Format::U8, 1)
            .unwrap()
            .to_channels(2)
            .unwrap()
            .volume(Volume::from_percent(50))
            .write_bytes(&mut output, Format::I16);

        assert_eq!(frames, 3);

        let samples: [i16; 6] = core::array::from_fn(|index| i16::read_le(&output[index * 2..]));
        assert_eq!(samples, [-16_384, -16_384, 0, 0, 8_192, 8_192]);
    }

    fn mix_streams<R: Representation>() {
        let music = [1000i16, 1000, 2000, 2000, 3000, 3000];
        let beep = [500i16];
        let mut output = [0i16; 8];

        let frames = samples_using::<R, _>(&music, 2)
            .unwrap()
            .mix(
                samples_using::<R, _>(&beep, 1)
                    .unwrap()
                    .volume(Volume::from_percent(200)),
            )
            .write_samples(&mut output);

        assert_eq!(frames, 3);
        assert_eq!(output, [2000, 1000, 2000, 2000, 3000, 3000, 0, 0]);
    }

    fn mix_into_existing<R: Representation>() {
        let mut output = [0u8; 4];
        100i16.write_le(&mut output[..2]);
        i16::MAX.write_le(&mut output[2..]);

        let frames = samples_using::<R, _>(&[50i16, 10], 2)
            .unwrap()
            .mix_into_bytes(&mut output, Format::I16);

        assert_eq!(frames, 1);
        assert_eq!(i16::read_le(&output[..2]), 150);
        assert_eq!(i16::read_le(&output[2..]), i16::MAX);
    }

    fn downmix<R: Representation>() {
        let mut output = [0i16; 2];

        samples_using::<R, _>(&[100i16, 300, -200, -400], 2)
            .unwrap()
            .to_channels(1)
            .unwrap()
            .write_samples(&mut output);

        assert_eq!(output, [200, -300]);
    }

    fn map_and_mute<R: Representation>() {
        let mut output = [1i16; 2];

        samples_using::<R, _>(&[100i16, 200], 1)
            .unwrap()
            .map(|value| R::add(value, value))
            .write_samples(&mut output);
        assert_eq!(output, [200, 400]);

        samples_using::<R, _>(&[100i16, 200], 1)
            .unwrap()
            .volume(Volume::MUTE)
            .write_samples(&mut output);
        assert_eq!(output, [0, 0]);
    }

    #[test]
    fn fixed() {
        chain::<Fixed>();
        mix_streams::<Fixed>();
        mix_into_existing::<Fixed>();
        downmix::<Fixed>();
        map_and_mute::<Fixed>();
    }

    #[test]
    fn float() {
        chain::<Float>();
        mix_streams::<Float>();
        mix_into_existing::<Float>();
        downmix::<Float>();
        map_and_mute::<Float>();
    }

    #[test]
    fn stops_when_output_is_full_and_keeps_the_rest() {
        let input = [1u8, 2, 3, 4, 5, 6];
        let mut stream = source(&input, Format::U8, 2).unwrap();
        let mut output = [0u8; 5];

        assert_eq!(stream.write_bytes(&mut output, Format::U8), 2);
        assert_eq!(output, [1, 2, 3, 4, 0]);
        assert_eq!(stream.remaining(), [5, 6]);

        assert_eq!(stream.write_bytes(&mut output, Format::U8), 1);
        assert_eq!(stream.remaining(), []);
        assert_eq!(stream.write_bytes(&mut output, Format::U8), 0);
    }

    #[test]
    fn ignores_incomplete_frames() {
        let mut output = [0i16; 4];

        assert_eq!(
            samples(&[1i16, 2, 3], 2)
                .unwrap()
                .write_samples(&mut output),
            1
        );
        assert_eq!(
            source(&[0, 0, 0], Format::I16, 1)
                .unwrap()
                .write_samples(&mut output),
            1
        );
    }

    #[test]
    fn rejects_invalid_channel_counts() {
        assert!(samples(&[0i16], 0).is_err());
        assert!(samples(&[0i16], 9).is_err());
        assert!(samples(&[0i16], 1).unwrap().to_channels(9).is_err());
        assert!(samples(&[0i16], 1).unwrap().to_channels(8).is_ok());
    }
}
