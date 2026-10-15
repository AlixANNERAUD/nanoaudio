use nanoaudio_adpcm::ImaAdpcm;
use nanoaudio_core::Error;
use nanoaudio_core::codec::{Decoder, Encoder, Seek};

fn signal(samples: usize) -> Vec<i16> {
    let mut state = 0x2545_F491u32;

    (0..samples)
        .map(|index| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            ((index as f32 * 0.05).sin() * 20_000.0) as i16 + (state % 2_001) as i16 - 1_000
        })
        .collect()
}

fn round_trip<const MAXIMUM_CHANNELS: usize>(
    mut codec: ImaAdpcm<MAXIMUM_CHANNELS>,
    input: &[i16],
) -> (Vec<u8>, Vec<i16>) {
    let blocks = input.len() / Encoder::minimum_input(&codec);
    let mut encoded = vec![0; blocks * codec.block_align() as usize];
    let progress = codec.encode(input, &mut encoded).unwrap();

    assert_eq!(progress.produced, encoded.len());

    let mut decoded = vec![0; blocks * Decoder::minimum_output(&codec)];
    let progress = codec.decode(&encoded, &mut decoded).unwrap();

    assert_eq!(progress.consumed, encoded.len());
    (encoded, decoded)
}

#[test]
fn limited_codecs_match_the_default() {
    for channels in [1, 2] {
        let input = signal(4 * 505 * channels as usize);
        let expected = round_trip(
            ImaAdpcm::new(16_000, channels, 256 * channels as u16).unwrap(),
            &input,
        );

        assert_eq!(
            round_trip(
                ImaAdpcm::<2>::new_limited(16_000, channels, 256 * channels as u16).unwrap(),
                &input
            ),
            expected
        );
        assert_eq!(
            round_trip(
                ImaAdpcm::<3>::with_samples_per_block_limited(16_000, channels, 505).unwrap(),
                &input
            ),
            expected
        );
    }

    let input = signal(4 * 505);
    let expected = round_trip(ImaAdpcm::new(16_000, 1, 256).unwrap(), &input);

    assert_eq!(
        round_trip(ImaAdpcm::<1>::new_limited(16_000, 1, 256).unwrap(), &input),
        expected
    );
}

#[test]
fn limited_codecs_reject_more_channels() {
    assert_eq!(
        ImaAdpcm::<1>::new_limited(16_000, 2, 512),
        Err(Error::TooManyChannels)
    );
    assert_eq!(
        ImaAdpcm::<2>::new_limited(16_000, 3, 768),
        Err(Error::TooManyChannels)
    );
    assert_eq!(
        ImaAdpcm::<2>::new_limited(16_000, 0, 256),
        Err(Error::TooManyChannels)
    );
    assert_eq!(
        ImaAdpcm::<2>::with_samples_per_block_limited(16_000, 3, 505),
        Err(Error::TooManyChannels)
    );
}

#[test]
fn limited_codecs_seek() {
    let input = signal(4 * 505 * 2);
    let (encoded, decoded) = round_trip(ImaAdpcm::new(16_000, 2, 512).unwrap(), &input);
    let mut codec = ImaAdpcm::<2>::new_limited(16_000, 2, 512).unwrap();
    let request = codec.seek(700).unwrap();
    let mut output = vec![0; 3 * 505 * 2];
    let progress = codec
        .decode(&encoded[request.byte_offset as usize..], &mut output)
        .unwrap();

    assert_eq!(output[..progress.produced], decoded[700 * 2..]);
}

#[test]
fn limited_codecs_are_smaller() {
    assert!(size_of::<ImaAdpcm<2>>() < size_of::<ImaAdpcm>());
    assert!(size_of::<ImaAdpcm<1>>() <= size_of::<ImaAdpcm<2>>());
}
