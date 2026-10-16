use nanoaudio_dsp::{Fixed, Float, Stream};
use nanoaudio_synth::{Adsr, FilterSettings, Patch, Synth, Synthesis, Waveform};

use crate::dsp::Operation;

const SAMPLE_RATE: u32 = 44_100;
const CHORD: [u8; 8] = [36, 48, 55, 60, 64, 67, 71, 74];

fn patch(filtered: bool) -> Patch {
    let mut patch = Patch::new(Waveform::Saw, Adsr::new(441, 4_410, 40_000, 4_410));

    if filtered {
        patch.filter = Some(FilterSettings::low_pass(1_500.0, 0.5));
    }

    patch
}

fn render<R: Synthesis + 'static, const VOICES: usize>(
    name: &'static str,
    samples: usize,
    filtered: bool,
) -> Operation {
    let mut output = vec![R::SILENCE; samples];
    let patch = patch(filtered);

    Operation {
        name,
        run: Box::new(move || {
            let mut synth = Synth::<R, VOICES>::new(SAMPLE_RATE);

            for note in &CHORD[..VOICES] {
                synth.note_on(0, *note, 64, &patch);
            }

            synth.render(&mut output);
            output.len()
        }),
    }
}

fn stream<R: Synthesis + 'static>(name: &'static str, samples: usize) -> Operation {
    let mut output = vec![0i16; samples];
    let patch = patch(true);

    Operation {
        name,
        run: Box::new(move || {
            let mut synth = Synth::<R, 8>::new(SAMPLE_RATE);

            for note in CHORD {
                synth.note_on(0, note, 64, &patch);
            }

            synth.write_samples(&mut output)
        }),
    }
}

fn operations<R: Synthesis + 'static>(samples: usize) -> Vec<Operation> {
    vec![
        render::<R, 1>("Synth, one saw voice", samples, false),
        render::<R, 1>("Synth, one filtered saw voice", samples, true),
        render::<R, 8>("Synth, 8 filtered voices", samples, true),
        stream::<R>("Synth, 8 filtered voices, stream to i16", samples),
    ]
}

pub fn representations(samples: usize) -> Vec<(&'static str, Vec<Operation>)> {
    vec![
        ("Fixed", operations::<Fixed>(samples)),
        ("Float", operations::<Float>(samples)),
    ]
}
