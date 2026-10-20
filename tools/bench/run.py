import argparse
import datetime
import json
import os
import platform
import shutil
import statistics
import struct
import subprocess
import sys
from pathlib import Path

TOOL = Path(__file__).resolve().parent
ROOT = TOOL.parent.parent
TARGET = ROOT / "target" / "bench"
DATA = TARGET / "data"
FLASH = TOOL / "flash"
FLASH_TARGET = ROOT / "target" / "bench-flash"
OUTPUT = ROOT / "docs" / "benchmarks.md"

FLASH_TARGETS = [
    ("thumbv6m-none-eabi", "Cortex-M0+"),
    ("thumbv7em-none-eabihf", "Cortex-M4F"),
    ("riscv32imc-unknown-none-elf", "RV32IMC"),
]
OPT_LEVELS = ["s", "3"]
NO_CAS_TARGETS = ["thumbv6m-none-eabi", "riscv32imc-unknown-none-elf"]
SINGLE_CORE = "--cfg portable_atomic_unsafe_assume_single_core"
PROBES = [
    ("G.711", "g711", "g711_codec"),
    ("IMA ADPCM", "ima_adpcm", "ima_adpcm_codec"),
    ("MS ADPCM", "ms_adpcm", "ms_adpcm_codec"),
    ("QOA", "qoa", "qoa_codec"),
    ("FLAC", "flac", None),
    ("G.722", "g722", "g722_codec"),
    ("SBC", "sbc", "sbc_codec"),
    ("MP3", "mp3", None),
    ("Opus", "opus", None),
    ("DSP pipeline", "dsp", None),
]

FLAC_SCRIPT = """
import sys
import numpy
import soundfile
samples = numpy.fromfile(sys.argv[1], dtype="<i2").reshape(-1, 2)
soundfile.write(sys.argv[2], samples, 44100, format="FLAC", subtype="PCM_16")
"""

OPUS_SCRIPT = """
import sys
import numpy
import soundfile
samples = numpy.fromfile(sys.argv[1], dtype="<i2").reshape(-1, 2)
soundfile.write(sys.argv[2], samples, 48000, format="OGG", subtype="OPUS", compression_level=0.775)
"""

MP3_SCRIPT = """
import sys
import lameenc
encoder = lameenc.Encoder()
encoder.set_bit_rate(128)
encoder.set_in_sample_rate(44100)
encoder.set_channels(2)
encoder.set_quality(2)
data = open(sys.argv[1], "rb").read()
open(sys.argv[2], "wb").write(encoder.encode(data) + encoder.flush())
"""


def log(message):
    print(message, file=sys.stderr, flush=True)


def output(command, **options):
    try:
        return subprocess.run(command, capture_output=True, text=True, check=True, **options).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def host_triple():
    for line in output(["rustc", "-vV"]).splitlines():
        if line.startswith("host:"):
            return line.split()[1]
    sys.exit("cannot find the host triple")


def build(name, manifest, directory, target=None, rustflags="", options=(), opt_level=None):
    environment = dict(os.environ)
    environment["RUSTFLAGS"] = rustflags
    if opt_level:
        environment["CARGO_PROFILE_RELEASE_OPT_LEVEL"] = opt_level
    command = ["cargo", "build", "--quiet", "--release", "--manifest-path", str(manifest), "--target-dir", str(directory)]
    if target:
        command += ["--target", target]
    command += list(options)
    log("building " + name)
    subprocess.run(command, check=True, env=environment)
    return directory / (target or "") / "release"


def bench_binary(variant, target=None, rustflags="", options=()):
    directory = build("nanoaudio-bench (" + variant + ")", TOOL / "Cargo.toml", TARGET / variant, target, rustflags, options)
    suffix = ".wasm" if target and target.startswith("wasm") else ""
    return directory / ("nanoaudio-bench" + suffix)


def run_tool(prefix, arguments):
    result = subprocess.run(prefix + arguments, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "exit status " + str(result.returncode))
    return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]


def generate_data(native):
    DATA.mkdir(parents=True, exist_ok=True)
    raw = DATA / "signal_44100_2.raw"
    run_tool([str(native)], ["signal", "44100", "2", str(raw)])
    raw_48000 = DATA / "signal_48000_2.raw"
    run_tool([str(native)], ["signal", "48000", "2", str(raw_48000)])
    notes = []
    if not shutil.which("uv"):
        notes.append("uv was not found, so FLAC, MP3 and Opus were not measured.")
        return notes
    for name, packages, script, source in [
        ("signal.flac", ["soundfile", "numpy"], FLAC_SCRIPT, raw),
        ("signal.mp3", ["lameenc"], MP3_SCRIPT, raw),
        ("signal.opus", ["soundfile", "numpy"], OPUS_SCRIPT, raw_48000),
    ]:
        command = ["uv", "run", "--quiet", "--no-project"]
        for package in packages:
            command += ["--with", package]
        command += ["python", "-I", "-c", script, str(source), str(DATA / name)]
        log("generating " + name)
        try:
            subprocess.run(command, check=True)
        except (OSError, subprocess.CalledProcessError):
            notes.append("Generating " + name + " failed, so it was not measured.")
    return notes


def x86_runner(host):
    if host.startswith("x86_64"):
        return host, []
    if host == "aarch64-apple-darwin" and output(["arch", "-x86_64", "/usr/bin/true"]) is not None:
        return "x86_64-apple-darwin", ["arch", "-x86_64"]
    return None, None


def speed_runs(host, arguments, notes):
    data_arguments = ["--data", str(DATA), "--runs", str(arguments.runs), "--target-ms", str(arguments.target_ms)]
    results = {}
    native = bench_binary("native")
    passes = []
    for index in range(arguments.passes):
        log("measuring native speed, pass " + str(index + 1))
        passes.append(run_tool([str(native)], ["speed"] + data_arguments))
    results["native"] = passes

    scalar = bench_binary("native-mp3-scalar", options=["--no-default-features"])
    log("measuring MP3 without SIMD")
    results["native-mp3-scalar"] = [run_tool([str(scalar)], ["speed", "--filter", "mp3"] + data_arguments)]

    if not arguments.skip_wasm:
        if shutil.which("wasmtime"):
            wasm_arguments = ["--data", "/data", "--runs", str(arguments.runs), "--target-ms", str(arguments.target_ms)]
            for variant, flags in [("wasm", ""), ("wasm-simd128", "-C target-feature=+simd128")]:
                binary = bench_binary(variant, "wasm32-wasip1", flags)
                log("measuring " + variant)
                prefix = ["wasmtime", "run", "--dir", str(DATA) + "::/data", str(binary)]
                results[variant] = [run_tool(prefix, ["speed"] + wasm_arguments)]
                if variant == "wasm":
                    results["memory-32"] = run_tool(prefix, ["memory", "--data", "/data"])
        else:
            notes.append("wasmtime was not found, so WebAssembly was not measured.")

    if not arguments.skip_x86:
        triple, prefix = x86_runner(host)
        if triple is None:
            notes.append("No way to run x86_64 code was found, so x86_64 was not measured.")
        else:
            for variant, flags in [("x86_64", ""), ("x86_64-v3", "-C target-cpu=x86-64-v3")]:
                binary = bench_binary(variant, triple, flags)
                log("measuring " + variant)
                try:
                    results[variant] = [run_tool(prefix + [str(binary)], ["speed"] + data_arguments)]
                except RuntimeError as error:
                    notes.append(variant + " could not run here (" + str(error).splitlines()[-1][:120] + ").")
    return native, results


def section_sizes(path):
    data = path.read_bytes()
    wide = data[4] == 2
    order = "<" if data[5] == 1 else ">"
    if wide:
        offset = struct.unpack_from(order + "Q", data, 0x28)[0]
        size, count, names = struct.unpack_from(order + "HHH", data, 0x3A)
    else:
        offset = struct.unpack_from(order + "I", data, 0x20)[0]
        size, count, names = struct.unpack_from(order + "HHH", data, 0x2E)
    sections = []
    for index in range(count):
        start = offset + index * size
        if wide:
            name, _, _, _, position, length = struct.unpack_from(order + "IIQQQQ", data, start)
        else:
            name, _, _, _, position, length = struct.unpack_from(order + "IIIIII", data, start)
        sections.append((name, position, length))
    table = sections[names][1]
    result = {}
    for name, _, length in sections:
        end = data.index(b"\0", table + name)
        label = data[table + name:end].decode()
        result[label] = result.get(label, 0) + length
    return result


def flash_sizes():
    sizes = {}
    for triple, _ in FLASH_TARGETS:
        for level in OPT_LEVELS:
            directory = build(
                "flash probes for " + triple + " at opt-level " + level,
                FLASH / "Cargo.toml",
                FLASH_TARGET / level,
                triple,
                rustflags=SINGLE_CORE if triple in NO_CAS_TARGETS else "",
                opt_level=level,
            )
            for path in directory.iterdir():
                if path.is_file() and not path.suffix and os.access(path, os.X_OK):
                    sections = section_sizes(path)
                    text = sum(value for key, value in sections.items() if key.startswith(".text"))
                    rodata = sum(value for key, value in sections.items() if key.startswith(".rodata"))
                    sizes[(path.name, triple, level)] = (text, rodata)
    return sizes


def machine(load):
    cpu = None
    if sys.platform == "darwin":
        cpu = output(["sysctl", "-n", "machdep.cpu.brand_string"])
    elif Path("/proc/cpuinfo").exists():
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    return {
        "cpu": cpu or platform.processor() or platform.machine(),
        "system": platform.platform(terse=True),
        "rustc": output(["rustc", "-V"]),
        "wasmtime": output(["wasmtime", "--version"]),
        "commit": output(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"]),
        "crates_changed": output(["git", "-C", str(ROOT), "status", "--porcelain", "--", "crates"]) not in ("", None),
        "date": datetime.date.today().isoformat(),
        "load": load,
    }


def key(row):
    return (row["codec"], row["configuration"], row["direction"])


def per_sample(row):
    return row["median_ns"] / row["samples"]


def speed_table(passes):
    rows = {}
    for results in passes:
        for row in results:
            rows.setdefault(key(row), []).append(row)
    table = {}
    for name, entries in rows.items():
        values = [per_sample(entry) for entry in entries]
        spreads = [(entry["high_ns"] - entry["low_ns"]) / entry["median_ns"] for entry in entries]
        table[name] = {
            "row": entries[0],
            "ns": statistics.mean(values),
            "spread": max(spreads),
            "drift": (max(values) - min(values)) / statistics.mean(values),
        }
    return table


def number(value, digits=1):
    if value is None:
        return "n/a"
    if value >= 100:
        return "{:,.0f}".format(value)
    if value >= 10:
        return "{:.1f}".format(value)
    return "{:.{}f}".format(value, max(digits, 2))


def real_time(row, ns):
    seconds = 1e9 / (ns * row["sample_rate"] * row["channels"])
    return "{:,.0f}×".format(seconds)


def kib(value):
    return "{:.1f}".format(value / 1024)


def markdown_table(header, rows):
    lines = ["| " + " | ".join(header) + " |", "|" + "|".join(" --- " for _ in header) + "|"]
    for row in rows:
        lines.append("| " + " | ".join(str(cell) for cell in row) + " |")
    return "\n".join(lines)


def ordered(table):
    seen = []
    for name in table:
        if name[:2] not in seen:
            seen.append(name[:2])
    return seen


def kept_section():
    if not OUTPUT.exists():
        return None
    text = OUTPUT.read_text()
    start = text.find("\n## Other Rust crates\n")
    if start < 0:
        return None
    end = text.find("\n## ", start + 1)
    return text[start + 1 : end + 1 if end >= 0 else len(text)]


def write(info, notes, speeds, memory, dsp, quality, flash, host, arguments):
    native = speed_table(speeds["native"])
    noise_spread = statistics.median(entry["spread"] for entry in native.values())
    noise_drift = max(entry["drift"] for entry in native.values()) if len(speeds["native"]) > 1 else None
    lines = []
    add = lines.append
    add("# Benchmarks")
    add("")
    add("Numbers to help pick a codec and a configuration. They come from `tools/bench`, are regenerated with `python3 tools/bench/run.py`, and are only as good as the machine they ran on: compare rows with each other rather than trusting absolute values.")
    add("")
    add("- Commit: `" + str(info["commit"]) + "`" + (" with local changes in `crates/`" if info["crates_changed"] else "") + ", " + info["date"] + ".")
    add("- Host: " + str(info["cpu"]) + ", " + info["system"] + " (`" + host + "`).")
    add("- Toolchain: " + str(info["rustc"]) + (", " + info["wasmtime"] if info["wasmtime"] else "") + ".")
    add("- Method: each measurement repeats the whole operation until it lasts at least " + str(arguments.target_ms) + " ms, then takes the median of " + str(arguments.runs) + " such runs after a warm-up run" + (", over " + str(arguments.passes) + " passes averaged" if arguments.passes > 1 else "") + ".")
    noise = "- Noise: the median interquartile range is {:.1f} % of the median".format(noise_spread * 100)
    if noise_drift is not None:
        noise += ", and the largest difference between passes is {:.0f} %".format(noise_drift * 100)
    noise += ". The load average was {:.1f} on {} cores when the run started, so treat differences under 5 % as noise.".format(info["load"], os.cpu_count())
    add(noise)
    for note in notes:
        add("- " + note)
    add("")
    add("The test signal is generated by `tools/bench/src/signal.rs`: 4 seconds of chords with harmonics and vibrato, low-passed noise and decaying noise bursts every quarter second, around -18 dBFS RMS. FLAC, MP3 and Opus files are encoded from the same signal at benchmark time, with libsndfile and LAME through `uv` (Opus from a 48 kHz rendering of it).")
    add("")
    add("## Speed")
    add("")
    add("Nanoseconds per sample (per channel, so a stereo frame counts twice), and how many times faster than real time at that configuration, on one core of the host.")
    add("")
    rows = []
    for codec, configuration in ordered(native):
        decode = native.get((codec, configuration, "decode"))
        encode = native.get((codec, configuration, "encode"))
        rows.append([
            codec,
            configuration,
            number(decode["ns"]) if decode else "",
            real_time(decode["row"], decode["ns"]) if decode else "",
            number(encode["ns"]) if encode else "",
            real_time(encode["row"], encode["ns"]) if encode else "",
        ])
    add(markdown_table(["Codec", "Configuration", "Decode (ns/sample)", "Decode speed", "Encode (ns/sample)", "Encode speed"], rows))
    add("")
    add("The MS ADPCM and QOA encoders try every predictor or scale factor and keep the best, so they encode much more slowly than they decode. Decoding is what runs on the device in most applications.")
    add("")
    add("## Vectorization")
    add("")
    add("FLAC, G.722 and SBC use SIMD on x86, AArch64 and WebAssembly with `simd128`, and the MP3 decoder behind the `nanoaudio-mp3/simd` feature (on by default). NEON is part of the AArch64 baseline and cannot be turned off, so the native numbers above always include it; the honest comparisons are below. Values are ns/sample.")
    add("")
    scalar = speed_table(speeds.get("native-mp3-scalar", []))
    if scalar:
        rows = []
        for name, entry in scalar.items():
            simd = native.get(name)
            rows.append([name[0], name[1], name[2], number(entry["ns"]), number(simd["ns"]) if simd else "n/a", "{:.2f}×".format(entry["ns"] / simd["ns"]) if simd else ""])
        add("MP3 on the host, `simd` feature off and on:")
        add("")
        add(markdown_table(["Codec", "Configuration", "Direction", "Without", "With", "Gain"], rows))
        add("")
    pairs = [
        ("wasm", "wasm-simd128", "WebAssembly under wasmtime, without and with `-C target-feature=+simd128`"),
        ("x86_64", "x86_64-v3", "x86_64, baseline (SSE2) and `-C target-cpu=x86-64-v3` (AVX2)" + (". This ran under Rosetta 2, which translates both builds to Arm code, and AVX2 code translates poorly: these rows show that the x86_64 builds work, not how fast a real x86_64 CPU is" if host == "aarch64-apple-darwin" else "")),
    ]
    for first, second, title in pairs:
        if first not in speeds:
            continue
        before = speed_table(speeds[first])
        after = speed_table(speeds[second]) if second in speeds else {}
        rows = []
        for name, entry in before.items():
            other = after.get(name)
            rows.append([name[0], name[1], name[2], number(entry["ns"]), number(other["ns"]) if other else "n/a", "{:.2f}×".format(entry["ns"] / other["ns"]) if other else ""])
        add(title + ":")
        add("")
        add(markdown_table(["Codec", "Configuration", "Direction", "Without", "With", "Gain"], rows))
        add("")
    add("## Fixed and floating point")
    add("")
    add("The DSP operations on 4 seconds of 44.1 kHz stereo, in ns/sample, with each `Representation`. On an MCU without an FPU, `Float` is emulated in software and much slower, which is why `Fixed` is the default there. The byte-slice and 24-bit paths were being optimised when these numbers were taken, so expect those rows to improve after the commit above.")
    add("")
    operations = []
    values = {}
    for row in dsp:
        if row["operation"] not in operations:
            operations.append(row["operation"])
        values[(row["operation"], row["representation"])] = row["median_ns"] / row["samples"]
    rows = []
    for operation in operations:
        if (operation, "Any") in values:
            continue
        rows.append([operation, number(values.get((operation, "Fixed"))), number(values.get((operation, "Float")))])
    add(markdown_table(["Operation", "Fixed", "Float"], rows))
    add("")
    rows = [[operation, number(values[(operation, "Any")])] for operation in operations if (operation, "Any") in values]
    add("Conversions do not depend on the representation:")
    add("")
    add(markdown_table(["Conversion", "ns/sample"], rows))
    add("")
    add("## Memory")
    add("")
    add("`State` is `size_of` the decoder or encoder, on a 32-bit target (measured on wasm32, which lays out structures like Cortex-M and RISC-V) and on the 64-bit host. `Input` and `Output` are the smallest buffers each call needs, in bytes: whole blocks or frames for block codecs, otherwise what `minimum_input()` and `minimum_output()` report. The total is what a 32-bit application has to reserve, excluding the stack.")
    add("")
    by_key = {}
    for row in memory.get("memory-32", []):
        by_key[(row["codec"], row["direction"], row["configuration"])] = row
    rows = []
    for row in memory["memory-64"]:
        small = by_key.get((row["codec"], row["direction"], row["configuration"]))
        state = small["state_bytes"] if small else None
        total = None
        if state is not None and row["input_bytes"] is not None and row["output_bytes"] is not None:
            total = state + row["input_bytes"] + row["output_bytes"]
        rows.append([
            row["codec"],
            row["direction"],
            row["configuration"],
            "{:,}".format(state) if state is not None else "n/a",
            "{:,}".format(row["state_bytes"]),
            "{:,}".format(row["input_bytes"]) if row["input_bytes"] is not None else "n/a",
            "{:,}".format(row["output_bytes"]),
            "{:,}".format(total) if total is not None else "n/a",
        ])
    add(markdown_table(["Codec", "Direction", "Configuration", "State, 32-bit", "State, 64-bit", "Input", "Output", "Total, 32-bit"], rows))
    add("")
    add("FLAC needs one `i32` per sample of its largest block for every channel, or one `i16` with `i16` blocks for streams of up to 16 bits: pick `MAXIMUM_BLOCK_SIZE` and `MAXIMUM_CHANNELS` to match your files (most encoders use 4096 samples), and give it an input window of at least the largest frame. The MP3 input window defaults to 4 KiB and can be lowered with `Mp3Decoder::with_minimum_input`, down to the largest frame plus the next header (1,445 bytes for Layer III, whose largest frame is 1,441 bytes at 320 kbit/s and 32 kHz); the bit reservoir is kept inside the decoder state. The FLAC input shown is the bound computed from the test file's header.")
    add("")
    add("## Flash")
    add("")
    add("`.text` + `.rodata` in KiB of a `no_std` binary that only decodes (or decodes and encodes) with one codec, built with fat LTO, one codegen unit and `panic = \"abort\"`, minus an empty binary. The probes are in `tools/bench/flash`. Formatting and panic messages are not included, as with a `panic-halt` style handler.")
    add("")
    header = ["Codec", "Probe"]
    for _, label in FLASH_TARGETS:
        for level in OPT_LEVELS:
            header.append(label + ", `" + level + "`")
    rows = []
    for label, decode, codec in PROBES:
        for probe, kind in [(decode, "decode"), (codec, "decode + encode")]:
            if probe is None:
                continue
            if label == "DSP pipeline":
                kind = "volume + mix + conversion"
            row = [label, kind]
            for triple, _ in FLASH_TARGETS:
                for level in OPT_LEVELS:
                    size = flash.get((probe, triple, level))
                    empty = flash.get(("empty", triple, level))
                    row.append(kib(sum(size) - sum(empty)) if size and empty else "n/a")
            rows.append(row)
    add(markdown_table(header, rows))
    add("")
    add("## Embedded CPU")
    add("")
    add("No embedded hardware was used, so there are no cycle counts here. The host numbers rank the codecs, but the gap between them changes on a microcontroller: MP3 decodes in floating point, emulated in software on cores without an FPU, and SBC (like FLAC on high-resolution streams) uses 64-bit products, which take several instructions on a 32-bit core, while G.711, IMA ADPCM, G.722 and QOA decoding stay in 32-bit integers. Measure on the target before committing to a codec close to your CPU budget.")
    add("")
    add("## Quality and compression")
    add("")
    add("Encoded and decoded back with nanoaudio, compared with the original test signal. The SNR is a plain signal-to-noise ratio over the whole band after aligning the codec delay, not a perceptual measure: SBC and MP3 shape their noise to be less audible, so they sound better than their SNR suggests.")
    add("")
    rows = []
    for row in quality:
        lossless = row["lossless"] == "yes"
        if lossless:
            snr = "lossless" + (" (bit-exact)" if row["exact"] == "yes" else "")
        elif row["snr_db"] is None:
            snr = "n/a"
        else:
            snr = "{:.1f} dB".format(row["snr_db"])
        if row["codec"] == "MP3":
            snr = "depends on the encoder (LAME here)"
        if row["codec"] == "Opus":
            snr = "depends on the encoder (libopus through libsndfile here)"
        rows.append([
            row["codec"],
            row["configuration"],
            "{:.0f}".format(row["bitrate_kbps"]),
            "{:.1f}:1".format(row["pcm_bytes"] / row["encoded_bytes"]),
            snr,
        ])
    add(markdown_table(["Codec", "Configuration", "Bitrate (kbit/s)", "Ratio vs 16-bit PCM", "SNR"], rows))
    add("")
    add("Lossless compression depends on the material: the noise bursts in this signal cannot be predicted, so FLAC does worse here than on most music.")
    add("")
    comparison = kept_section()
    if comparison:
        add(comparison.rstrip("\n"))
        add("")
    add("## Reproducing")
    add("")
    add("```sh")
    add("python3 tools/bench/run.py")
    add("```")
    add("")
    add("It needs `rustup` targets `wasm32-wasip1`, `thumbv6m-none-eabi`, `thumbv7em-none-eabihf` and `riscv32imc-unknown-none-elf`, `wasmtime` for the WebAssembly rows, `uv` to encode the FLAC, MP3 and Opus test files, and on Apple silicon the `x86_64-apple-darwin` target and Rosetta 2 for the x86_64 rows. Use `--help` to skip parts. Close other applications first: the numbers move with load, thermal state and power settings.")
    add("")
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text("\n".join(lines))
    log("wrote " + str(OUTPUT.relative_to(ROOT)))


def main():
    parser = argparse.ArgumentParser(description="Run the nanoaudio benchmarks and write docs/benchmarks.md.")
    parser.add_argument("--runs", type=int, default=11, help="timed runs per measurement")
    parser.add_argument("--target-ms", type=int, default=40, help="minimum duration of one run")
    parser.add_argument("--passes", type=int, default=2, help="native passes, to estimate noise")
    parser.add_argument("--skip-wasm", action="store_true", help="do not measure WebAssembly")
    parser.add_argument("--skip-x86", action="store_true", help="do not measure x86_64")
    parser.add_argument("--skip-flash", action="store_true", help="do not build the flash probes")
    arguments = parser.parse_args()

    load = os.getloadavg()[0] if hasattr(os, "getloadavg") else 0.0
    host = host_triple()
    native = bench_binary("native")
    notes = generate_data(native)
    native, speeds = speed_runs(host, arguments, notes)
    memory = {"memory-64": run_tool([str(native)], ["memory", "--data", str(DATA)])}
    if "memory-32" in speeds:
        memory["memory-32"] = speeds.pop("memory-32")
    log("measuring DSP operations")
    dsp = run_tool([str(native)], ["dsp", "--runs", str(arguments.runs), "--target-ms", str(arguments.target_ms)])
    log("measuring quality")
    quality = run_tool([str(native)], ["quality", "--data", str(DATA)])
    flash = {} if arguments.skip_flash else flash_sizes()
    write(machine(load), notes, speeds, memory, dsp, quality, flash, host, arguments)


if __name__ == "__main__":
    main()
