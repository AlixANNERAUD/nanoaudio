use core::marker::PhantomData;

use nanoaudio_core::{Error, Format, I24, Result, Sample};

use crate::{DefaultRepresentation, Fixed, Float, Representation, RepresentationKind, Volume};

pub fn mix<S: Sample>(output: &mut [S], input: &[S], volume: Volume) -> usize {
    mix_using::<DefaultRepresentation, S>(output, input, volume)
}

pub fn mix_using<R: Representation, S: Sample>(
    output: &mut [S],
    input: &[S],
    volume: Volume,
) -> usize {
    let count = output.len().min(input.len());

    if volume.is_mute() {
        return count;
    }

    let gain = R::gain(volume);

    for (output, input) in output.iter_mut().zip(input) {
        *output = mix_sample::<R, S>(*output, *input, gain);
    }

    count
}

pub fn mix_bytes(output: &mut [u8], input: &[u8], format: Format, volume: Volume) -> usize {
    mix_bytes_using::<DefaultRepresentation>(output, input, format, volume)
}

pub fn mix_bytes_with(
    kind: RepresentationKind,
    output: &mut [u8],
    input: &[u8],
    format: Format,
    volume: Volume,
) -> usize {
    match kind {
        RepresentationKind::Fixed => mix_bytes_using::<Fixed>(output, input, format, volume),
        RepresentationKind::Float => mix_bytes_using::<Float>(output, input, format, volume),
    }
}

pub fn mix_bytes_using<R: Representation>(
    output: &mut [u8],
    input: &[u8],
    format: Format,
    volume: Volume,
) -> usize {
    match format {
        Format::U8 => mix_bytes_typed::<R, u8>(output, input, volume),
        Format::I16 => mix_bytes_typed::<R, i16>(output, input, volume),
        Format::I24 => mix_bytes_typed::<R, I24>(output, input, volume),
        Format::I32 => mix_bytes_typed::<R, i32>(output, input, volume),
        Format::F32 => mix_bytes_typed::<R, f32>(output, input, volume),
    }
}

fn mix_bytes_typed<R: Representation, S: Sample>(
    output: &mut [u8],
    input: &[u8],
    volume: Volume,
) -> usize {
    let count = (output.len() / S::SIZE).min(input.len() / S::SIZE);

    if volume.is_mute() {
        return count;
    }

    let gain = R::gain(volume);

    for (output, input) in output
        .chunks_exact_mut(S::SIZE)
        .zip(input.chunks_exact(S::SIZE))
    {
        mix_sample::<R, S>(S::read_le(output), S::read_le(input), gain).write_le(output);
    }

    count
}

fn mix_sample<R: Representation, S: Sample>(output: S, input: S, gain: R::Gain) -> S {
    R::to_sample(R::add(
        R::from_sample(output),
        R::amplify(R::from_sample(input), gain),
    ))
}

pub fn fill_silence(output: &mut [u8], format: Format) {
    match format {
        Format::U8 => output.fill(u8::SILENCE),
        Format::I16 | Format::I24 | Format::I32 | Format::F32 => output.fill(0),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mixer<const SOURCES: usize, R: Representation = DefaultRepresentation> {
    volumes: [Volume; SOURCES],
    master: Volume,
    representation: PhantomData<R>,
}

impl<const SOURCES: usize, R: Representation> Mixer<SOURCES, R> {
    pub const fn new() -> Self {
        Self {
            volumes: [Volume::UNITY; SOURCES],
            master: Volume::UNITY,
            representation: PhantomData,
        }
    }

    pub const fn representation(&self) -> RepresentationKind {
        R::KIND
    }

    pub const fn sources(&self) -> usize {
        SOURCES
    }

    pub const fn master_volume(&self) -> Volume {
        self.master
    }

    pub fn set_master_volume(&mut self, volume: Volume) {
        self.master = volume;
    }

    pub fn volume(&self, source: usize) -> Result<Volume> {
        self.volumes
            .get(source)
            .copied()
            .ok_or(Error::InvalidSource)
    }

    pub fn set_volume(&mut self, source: usize, volume: Volume) -> Result<()> {
        *self.volumes.get_mut(source).ok_or(Error::InvalidSource)? = volume;

        Ok(())
    }

    pub fn effective_volume(&self, source: usize) -> Result<Volume> {
        Ok(self.volume(source)?.combine(self.master))
    }

    pub fn clear<S: Sample>(&self, output: &mut [S]) {
        output.fill(S::SILENCE);
    }

    pub fn add<S: Sample>(&self, source: usize, output: &mut [S], input: &[S]) -> Result<usize> {
        Ok(mix_using::<R, S>(
            output,
            input,
            self.effective_volume(source)?,
        ))
    }

    pub fn clear_bytes(&self, output: &mut [u8], format: Format) {
        fill_silence(output, format);
    }

    pub fn add_bytes(
        &self,
        source: usize,
        output: &mut [u8],
        input: &[u8],
        format: Format,
    ) -> Result<usize> {
        Ok(mix_bytes_using::<R>(
            output,
            input,
            format,
            self.effective_volume(source)?,
        ))
    }
}

impl<const SOURCES: usize, R: Representation> Default for Mixer<SOURCES, R> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix_samples<R: Representation>() {
        let mut output = [100i16, i16::MAX, 0];

        assert_eq!(mix_using::<R, _>(&mut output, &[50, 1], Volume::UNITY), 2);
        assert_eq!(output, [150, i16::MAX, 0]);

        assert_eq!(
            mix_using::<R, _>(&mut output, &[100, 100, 100], Volume::from_percent(50)),
            3
        );
        assert_eq!(output, [200, i16::MAX, 50]);
    }

    fn mix_unsigned_bytes<R: Representation>() {
        let mut output = [0u8; 2];
        fill_silence(&mut output, Format::U8);

        assert_eq!(
            mix_bytes_with(
                R::KIND,
                &mut output,
                &[0x90, 0x70],
                Format::U8,
                Volume::UNITY
            ),
            2
        );
        assert_eq!(
            mix_bytes_with(
                R::KIND,
                &mut output,
                &[0x90, 0x70],
                Format::U8,
                Volume::UNITY
            ),
            2
        );
        assert_eq!(output, [0xA0, 0x60]);
    }

    fn mixer_applies_source_and_master_volumes<R: Representation>() {
        let mut mixer = Mixer::<2, R>::new();
        assert_eq!(mixer.representation(), R::KIND);
        mixer.set_volume(0, Volume::from_percent(50)).unwrap();
        mixer.set_master_volume(Volume::from_percent(50));

        let mut output = [1i16; 2];
        mixer.clear(&mut output);

        mixer.add(0, &mut output, &[400, -400]).unwrap();
        mixer.add(1, &mut output, &[1000, 1000]).unwrap();

        assert_eq!(output, [600, 400]);
    }

    #[test]
    fn fixed() {
        mix_samples::<Fixed>();
        mix_unsigned_bytes::<Fixed>();
        mixer_applies_source_and_master_volumes::<Fixed>();
    }

    #[test]
    fn float() {
        mix_samples::<Float>();
        mix_unsigned_bytes::<Float>();
        mixer_applies_source_and_master_volumes::<Float>();
    }

    #[test]
    fn float_mixing_keeps_precision_until_output() {
        let mut output = [0.75f32];

        mix_using::<Float, _>(&mut output, &[0.5], Volume::from_percent(50));

        assert_eq!(output, [1.0]);
    }

    #[test]
    fn mixer_bytes() {
        let mixer = Mixer::<1>::new();
        let mut input = [0u8; 4];
        0.25f32.write_le(&mut input);
        let mut output = [0xFFu8; 4];

        mixer.clear_bytes(&mut output, Format::F32);
        assert_eq!(mixer.add_bytes(0, &mut output, &input, Format::F32), Ok(1));
        assert_eq!(f32::read_le(&output), 0.25);
    }

    #[test]
    fn mixer_rejects_unknown_source() {
        let mut mixer = Mixer::<1>::new();
        let mut output = [0i16; 1];

        assert_eq!(mixer.set_volume(1, Volume::MUTE), Err(Error::InvalidSource));
        assert_eq!(mixer.add(1, &mut output, &[1]), Err(Error::InvalidSource));
    }
}
