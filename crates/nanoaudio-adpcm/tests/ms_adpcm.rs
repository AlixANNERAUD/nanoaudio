use nanoaudio_adpcm::{MsAdpcm, MsAdpcmCoefficients, MsAdpcmState};
use nanoaudio_core::codec::{Decoder, Encoder};

fn signal(samples: usize) -> Vec<i16> {
    let mut state = 0x2545_F491u32;

    (0..samples)
        .map(|index| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            ((index as f32 * 0.03).sin() * 24_000.0) as i16 + (state % 4_001) as i16 - 2_000
        })
        .collect()
}

fn reference_decode(block: &[u8], channels: usize, samples_per_block: usize) -> Vec<i16> {
    let coefficients = MsAdpcmCoefficients::STANDARD;
    let field = |index: usize, channel: usize| {
        let offset = channels + (index * channels + channel) * 2;

        i16::from_le_bytes([block[offset], block[offset + 1]])
    };
    let mut states: Vec<MsAdpcmState> = (0..channels)
        .map(|channel| {
            MsAdpcmState::new(
                coefficients.as_slice()[block[channel] as usize],
                field(0, channel),
                field(1, channel),
                field(2, channel),
            )
        })
        .collect();
    let mut output: Vec<i16> = states.iter().map(|state| state.before_previous()).collect();

    output.extend(states.iter().map(|state| state.previous()));

    let nibbles = block[7 * channels..]
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0F]);

    for (index, nibble) in nibbles.take((samples_per_block - 2) * channels).enumerate() {
        output.push(states[index % channels].decode(nibble));
    }

    output
}

#[test]
fn decode_matches_the_interleaved_nibbles() {
    for (channels, samples_per_block) in [(1, 63), (1, 64), (2, 63), (2, 64)] {
        let mut codec =
            MsAdpcm::with_samples_per_block(8_000, channels, samples_per_block).unwrap();
        let block_samples = samples_per_block as usize * channels as usize;
        let input = signal(3 * block_samples);
        let mut encoded = vec![0u8; 3 * codec.block_align() as usize];

        codec.encode(&input, &mut encoded).unwrap();

        let mut decoded = vec![0i16; input.len()];
        let progress = codec.decode(&encoded, &mut decoded).unwrap();
        let expected: Vec<i16> = encoded
            .chunks_exact(codec.block_align() as usize)
            .flat_map(|block| {
                reference_decode(block, channels as usize, samples_per_block as usize)
            })
            .collect();

        assert_eq!(progress.produced, decoded.len());
        assert_eq!(decoded, expected);
    }
}
