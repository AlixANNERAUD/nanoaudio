use crate::{Error, Format, Result, Sample, Volume};

pub fn mix<S: Sample>(output: &mut [S], input: &[S], volume: Volume) -> usize {
    let count = output.len().min(input.len());

    if volume.is_mute() {
        return count;
    }

    for (output, input) in output.iter_mut().zip(input) {
        *output = output.mix(volume.apply(*input));
    }

    count
}

pub fn mix_bytes(output: &mut [u8], input: &[u8], format: Format, volume: Volume) -> usize {
    match format {
        Format::U8 => mix_bytes_typed::<u8>(output, input, volume),
        Format::I16 => mix_bytes_typed::<i16>(output, input, volume),
        Format::I32 => mix_bytes_typed::<i32>(output, input, volume),
        Format::F32 => mix_bytes_typed::<f32>(output, input, volume),
    }
}

fn mix_bytes_typed<S: Sample>(output: &mut [u8], input: &[u8], volume: Volume) -> usize {
    let count = (output.len() / S::SIZE).min(input.len() / S::SIZE);

    if volume.is_mute() {
        return count;
    }

    for (output, input) in output
        .chunks_exact_mut(S::SIZE)
        .zip(input.chunks_exact(S::SIZE))
    {
        S::read_le(output)
            .mix(volume.apply(S::read_le(input)))
            .write_le(output);
    }

    count
}

pub fn fill_silence(output: &mut [u8], format: Format) {
    match format {
        Format::U8 => output.fill(u8::SILENCE),
        Format::I16 | Format::I32 | Format::F32 => output.fill(0),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mixer<const SOURCES: usize> {
    volumes: [Volume; SOURCES],
    master: Volume,
}

impl<const SOURCES: usize> Mixer<SOURCES> {
    pub const fn new() -> Self {
        Self {
            volumes: [Volume::UNITY; SOURCES],
            master: Volume::UNITY,
        }
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
        Ok(mix(output, input, self.effective_volume(source)?))
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
        Ok(mix_bytes(
            output,
            input,
            format,
            self.effective_volume(source)?,
        ))
    }
}

impl<const SOURCES: usize> Default for Mixer<SOURCES> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_samples() {
        let mut output = [100i16, i16::MAX, 0];

        assert_eq!(mix(&mut output, &[50, 1], Volume::UNITY), 2);
        assert_eq!(output, [150, i16::MAX, 0]);

        assert_eq!(
            mix(&mut output, &[100, 100, 100], Volume::from_percent(50)),
            3
        );
        assert_eq!(output, [200, i16::MAX, 50]);
    }

    #[test]
    fn mix_unsigned_bytes() {
        let mut output = [0u8; 2];
        fill_silence(&mut output, Format::U8);

        assert_eq!(
            mix_bytes(&mut output, &[0x90, 0x70], Format::U8, Volume::UNITY),
            2
        );
        assert_eq!(
            mix_bytes(&mut output, &[0x90, 0x70], Format::U8, Volume::UNITY),
            2
        );
        assert_eq!(output, [0xA0, 0x60]);
    }

    #[test]
    fn mixer_applies_source_and_master_volumes() {
        let mut mixer = Mixer::<2>::new();
        mixer.set_volume(0, Volume::from_percent(50)).unwrap();
        mixer.set_master_volume(Volume::from_percent(50));

        let mut output = [1i16; 2];
        mixer.clear(&mut output);

        mixer.add(0, &mut output, &[400, -400]).unwrap();
        mixer.add(1, &mut output, &[1000, 1000]).unwrap();

        assert_eq!(output, [600, 400]);
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
