mod codecs;
mod dsp;
mod harness;
mod memory;
mod signal;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use harness::{Measurement, Settings, measure};

enum Value {
    Text(String),
    Integer(usize),
    Number(f64),
    Missing,
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Value::Text(text.to_string())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Value::Text(text)
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Value::Integer(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        if value.is_finite() {
            Value::Number(value)
        } else {
            Value::Missing
        }
    }
}

impl From<Option<usize>> for Value {
    fn from(value: Option<usize>) -> Self {
        value.map_or(Value::Missing, Value::Integer)
    }
}

fn emit(fields: Vec<(&str, Value)>) {
    let mut line = String::from("{");

    for (index, (key, value)) in fields.into_iter().enumerate() {
        if index > 0 {
            line.push(',');
        }

        write!(line, "\"{key}\":").unwrap();

        match value {
            Value::Text(text) => {
                line.push('"');

                for character in text.chars() {
                    match character {
                        '"' => line.push_str("\\\""),
                        '\\' => line.push_str("\\\\"),
                        _ => line.push(character),
                    }
                }

                line.push('"');
            }
            Value::Integer(value) => write!(line, "{value}").unwrap(),
            Value::Number(value) => write!(line, "{value}").unwrap(),
            Value::Missing => line.push_str("null"),
        }
    }

    line.push('}');
    println!("{line}");
}

fn timing(fields: &mut Vec<(&str, Value)>, measurement: &Measurement) {
    fields.push(("repetitions", measurement.repetitions.into()));
    fields.push(("median_ns", measurement.median.into()));
    fields.push(("low_ns", measurement.low.into()));
    fields.push(("high_ns", measurement.high.into()));
    fields.push(("minimum_ns", measurement.minimum.into()));
}

struct Options {
    data: Option<PathBuf>,
    filter: Option<String>,
    settings: Settings,
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        data: None,
        filter: None,
        settings: Settings {
            runs: 11,
            target: Duration::from_millis(40),
        },
    };
    let mut iterator = arguments.iter();

    while let Some(argument) = iterator.next() {
        let mut value = || {
            iterator
                .next()
                .cloned()
                .ok_or_else(|| format!("missing value for {argument}"))
        };

        match argument.as_str() {
            "--data" => options.data = Some(PathBuf::from(value()?)),
            "--filter" => options.filter = Some(value()?),
            "--runs" => {
                options.settings.runs = value()?.parse().map_err(|_| "invalid --runs")?;
            }
            "--target-ms" => {
                let milliseconds = value()?.parse().map_err(|_| "invalid --target-ms")?;
                options.settings.target = Duration::from_millis(milliseconds);
            }
            _ => return Err(format!("unknown argument {argument}")),
        }
    }

    Ok(options)
}

fn speed(options: &Options) {
    for case in codecs::cases(options.data.as_deref(), options.filter.as_deref()) {
        let mut output = vec![0i16; case.output_capacity()];
        let expected = case.decoded().len();
        let measurement = measure(options.settings, || {
            (case.decode)(&case.encoded, &mut output)
        });
        let mut fields = vec![
            ("kind", "speed".into()),
            ("codec", case.codec.into()),
            ("configuration", case.configuration.clone().into()),
            ("direction", "decode".into()),
            ("sample_rate", (case.sample_rate as usize).into()),
            ("channels", (case.channels as usize).into()),
            ("samples", expected.into()),
        ];
        timing(&mut fields, &measurement);
        emit(fields);

        if let (Some(encode), Some(source)) = (&case.encode, &case.source) {
            let mut output = vec![0u8; case.encoded.len() + 65_536];
            let measurement = measure(options.settings, || encode(source, &mut output));
            let mut fields = vec![
                ("kind", "speed".into()),
                ("codec", case.codec.into()),
                ("configuration", case.configuration.clone().into()),
                ("direction", "encode".into()),
                ("sample_rate", (case.sample_rate as usize).into()),
                ("channels", (case.channels as usize).into()),
                ("samples", source.len().into()),
            ];
            timing(&mut fields, &measurement);
            emit(fields);
        }
    }
}

fn quality(options: &Options) {
    for case in codecs::cases(options.data.as_deref(), options.filter.as_deref()) {
        let seconds = case.frames() as f64 / case.sample_rate as f64;
        let pcm = case.frames() * case.channels as usize * 2;
        let decoded = case.decoded();
        let (snr, delay) = match &case.source {
            Some(source) if case.codec != "MP3" => {
                let (snr, delay) = codecs::snr(source, &decoded, case.channels as usize);
                (snr.into(), Some(delay))
            }
            _ => (Value::Missing, None),
        };
        let exact = case
            .source
            .as_ref()
            .is_some_and(|source| source.as_slice() == decoded.as_slice());

        emit(vec![
            ("kind", "quality".into()),
            ("codec", case.codec.into()),
            ("configuration", case.configuration.clone().into()),
            ("lossless", if case.lossless { "yes" } else { "no" }.into()),
            ("exact", if exact { "yes" } else { "no" }.into()),
            ("sample_rate", (case.sample_rate as usize).into()),
            ("channels", (case.channels as usize).into()),
            ("encoded_bytes", case.encoded.len().into()),
            ("pcm_bytes", pcm.into()),
            (
                "bitrate_kbps",
                (case.encoded.len() as f64 * 8.0 / seconds / 1000.0).into(),
            ),
            ("snr_db", snr),
            ("delay_frames", delay.into()),
        ]);
    }
}

fn memory(options: &Options) {
    for entry in memory::entries(options.data.as_deref()) {
        emit(vec![
            ("kind", "memory".into()),
            ("pointer_bits", (usize::BITS as usize).into()),
            ("codec", entry.item.into()),
            ("direction", entry.direction.into()),
            ("configuration", entry.configuration.into()),
            ("state_bytes", entry.state.into()),
            ("input_bytes", entry.input.into()),
            ("output_bytes", entry.output.into()),
        ]);
    }
}

fn dsp(options: &Options) {
    let data = dsp::Data::new();

    for (representation, operations) in dsp::representations(&data) {
        for mut operation in operations {
            let measurement = measure(options.settings, &mut operation.run);
            let mut fields = vec![
                ("kind", "dsp".into()),
                ("operation", operation.name.into()),
                ("representation", representation.into()),
                ("samples", data.samples().into()),
            ];
            timing(&mut fields, &measurement);
            emit(fields);
        }
    }

    for mut operation in dsp::conversion_operations(&data) {
        let measurement = measure(options.settings, &mut operation.run);
        let mut fields = vec![
            ("kind", "dsp".into()),
            ("operation", operation.name.into()),
            ("representation", "Any".into()),
            ("samples", data.samples().into()),
        ];
        timing(&mut fields, &measurement);
        emit(fields);
    }
}

fn write_signal(arguments: &[String]) -> Result<(), String> {
    let [sample_rate, channels, path] = arguments else {
        return Err("usage: signal <sample rate> <channels> <path>".into());
    };
    let sample_rate = sample_rate.parse().map_err(|_| "invalid sample rate")?;
    let channels = channels.parse().map_err(|_| "invalid channel count")?;
    let bytes: Vec<u8> = signal::generate(sample_rate, channels)
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();

    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

fn run(arguments: &[String]) -> Result<(), String> {
    let Some((command, rest)) = arguments.split_first() else {
        return Err("usage: nanoaudio-bench <speed|quality|memory|dsp|signal> [options]".into());
    };

    match command.as_str() {
        "signal" => write_signal(rest),
        "speed" => parse(rest).map(|options| speed(&options)),
        "quality" => parse(rest).map(|options| quality(&options)),
        "memory" => parse(rest).map(|options| memory(&options)),
        "dsp" => parse(rest).map(|options| dsp(&options)),
        _ => Err(format!("unknown command {command}")),
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
