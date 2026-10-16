import argparse
import json
import os
import shutil
import subprocess
import sys
import time

sys.dont_write_bytecode = True

import run

HEADING = "## Other Rust crates"
EMU = run.ROOT / "tools" / "emu"
EMU_CASES = [
    ("mp3 decode", "MP3 decode, 44.1 kHz stereo 128 kbit/s", "nanoaudio-mp3", "MP3 decode, 128 kbit/s", "frame (1152)"),
    ("mp3 decode", "MP3 decode, nanomp3-core frame decoder", "nanomp3-core 0.2", "MP3 decode, 128 kbit/s", "frame (1152)"),
    ("spectrum, 1024", "Spectrum, 1024 points, float", "nanoaudio-analysis `Spectrum`, `Float`", "Spectrum, 1024 points", "block (1024)"),
    ("spectrum, 1024", "Spectrum, 1024 points, microfft alone", "microfft 0.6 alone, same window and power", "Spectrum, 1024 points", "block (1024)"),
]


def section(text):
    start = text.find("\n" + HEADING + "\n")
    if start < 0:
        return None, None
    end = text.find("\n## ", start + 1)
    return start + 1, (end + 1 if end >= 0 else len(text))


def replace_section(content):
    text = run.OUTPUT.read_text() if run.OUTPUT.exists() else "# Benchmarks\n"
    start, end = section(text)
    if start is None:
        anchor = text.find("\n## Reproducing\n")
        start = end = anchor + 1 if anchor >= 0 else len(text)
        if start == len(text) and not text.endswith("\n\n"):
            content = "\n" + content
    run.OUTPUT.write_text(text[:start] + content + text[end:])


def emulate(notes):
    if not shutil.which("uv"):
        notes.append("uv was not found, so the instruction counts were not measured.")
        return {}
    counts = {}
    for name in sorted({case[0] for case in EMU_CASES}):
        run.log("counting instructions for " + name)
        command = ["uv", "run", "--quiet", "--isolated", "--no-project", "--with-requirements", str(EMU / "requirements.txt"), "python", str(EMU / "run.py"), "--filter", name]
        result = subprocess.run(command, capture_output=True, text=True, cwd=run.ROOT)
        if result.returncode != 0:
            notes.append("tools/emu failed (" + (result.stderr.strip().splitlines() or ["no output"])[-1][:120] + "), so the instruction counts are missing.")
            return {}
        lines = result.stdout.split("\n\n")[1].splitlines()
        header = [cell.strip() for cell in lines[0].strip("|").split("|")]
        for line in lines[2:]:
            cells = [cell.strip() for cell in line.strip("|").split("|")]
            counts[cells[0]] = dict(zip(header, cells))
    return counts


def size(value):
    if value < 1024:
        return "{} B".format(value)
    return "{:.1f} KiB".format(value / 1024)


def ratio(value, reference):
    if value is None or reference is None:
        return ""
    return "{:.2f}×".format(value / reference)


def decode_rows(rows):
    groups = []
    for row in rows:
        if row["group"] not in groups:
            groups.append(row["group"])
    lines = []
    for group in groups:
        entries = [row for row in rows if row["group"] == group]
        reference = entries[0]["median_ns"] / entries[0]["samples"]
        for row in entries:
            value = row["median_ns"] / row["samples"]
            memory = []
            if row["state_bytes"] is not None:
                memory.append(size(row["state_bytes"]) + " state")
            if row["heap_bytes"]:
                memory.append(size(row["heap_bytes"]) + " heap")
            agreement = row["agreement"]
            if row["snr_db"] is not None:
                agreement += ", SNR {:.0f} dB".format(row["snr_db"])
            lines.append([
                group + ", " + row["configuration"] if row is entries[0] else "",
                row["library"],
                run.number(value),
                ratio(value, reference),
                ", ".join(memory) or "none",
                agreement,
            ])
    return lines


def hertz(value):
    if value is None:
        return "n/a"
    if value < 10_000:
        return "< 10 kHz"
    return "{:.1f} kHz".format(value / 1000)


def resample_rows(rows):
    merged = {}
    for row in rows:
        key = (row["library"], row["setting"])
        entry = merged.setdefault(key, {"row": row})
        entry[(row["input_rate"], row["output_rate"])] = row["median_ns"] / row["samples"]
    lines = []
    for (library, setting), entry in merged.items():
        row = entry["row"]
        lines.append([
            library,
            setting,
            run.number(entry.get((44_100, 48_000))),
            run.number(entry.get((48_000, 44_100))),
            "{:.0f} dB".format(row["stop_band_db"]),
            hertz(row["flat_hz"]),
            hertz(row["half_power_hz"]),
            "{:.0f} dB".format(row["residual_db"]),
            size(row["state_bytes"]),
        ])
    return lines


def write(info, notes, results, counts, seconds, arguments, host):
    lines = []
    add = lines.append
    add(HEADING)
    add("")
    add("The same inputs decoded or processed by nanoaudio and by other crates, to the same output format. These numbers come from `python3 tools/bench/compare.py`, which builds `tools/bench` with its `compare` feature, so the published crates gain no dependency, and rewrites only this section.")
    add("")
    add("- Commit: `" + str(info["commit"]) + "`" + (" with local changes in `crates/`" if info["crates_changed"] else "") + ", " + info["date"] + ".")
    add("- Host: " + str(info["cpu"]) + ", " + info["system"] + " (`" + host + "`), " + str(info["rustc"]) + ".")
    add("- Method: the median of " + str(arguments.runs) + " runs of at least " + str(arguments.target_ms) + " ms each, one pass; the whole run took " + "{:.0f}".format(seconds) + " s. The load average was {:.1f} on {} cores at the start, so treat differences under 5 % as noise.".format(info["load"], os.cpu_count()))
    add("- Every decoder starts from the encoded file in memory, parses its container, and writes interleaved `i16`. Within a format, the time of every library is divided by the number of samples nanoaudio produces.")
    for note in notes:
        add("- " + note)
    add("")
    add("### Decoding")
    add("")
    add("Nanoseconds per sample. `State` is `size_of` the decoder, for the libraries whose decoder is a plain value; `heap` is the most memory a library had allocated at once during one whole decode. `Output` compares each library with the first row, after aligning any delay, except for FLAC and PCM, which are compared with the original signal.")
    add("")
    add(run.markdown_table(["Format", "Library", "ns/sample", "Time vs first row", "Memory", "Output"], decode_rows([row for row in results if row["kind"] == "decode"])))
    add("")
    add("- MP3: minimp3 is C, with its own SSE and NEON code. nanoaudio-mp3 is built on nanomp3, a safe Rust port of minimp3, and gives the same output; minimp3 rounds some negative samples differently, which nanomp3 fixes on purpose.")
    add("- ADPCM: nanoaudio's output matches libsndfile 1.2.2 bit for bit on both files (checked by hand with `soundfile`); symphonia's does not.")
    add("")
    add("### Resampling")
    add("")
    add("Interleaved stereo `f32` in and out, 4 seconds of the test signal, in nanoseconds per input sample. Every resampler is built once and reset before each run, except dasp's, which its API builds around the signal; rubato goes through `process_all_into_buffer` with 1024-frame chunks.")
    add("")
    add("The quality columns use sines at half full scale. `Stop-band` is the loudest output from tones between 22.3 and 23.9 kHz when going from 48 to 44.1 kHz, all of which must be removed. `Flat to` and `-3 dB at` scan tones from 10 kHz upwards in 100 Hz steps when going from 44.1 to 48 kHz: the last tone within 0.1 dB, and the last one above -3 dB. `Residual` is what is left of a 10 kHz tone resampled from 44.1 to 48 kHz once the tone is removed: images, aliasing and rounding. `State` is `size_of` for nanoaudio and the heap that the constructor and the first run allocate for the others.")
    add("")
    add(run.markdown_table(["Library", "Setting", "44.1 → 48 kHz", "48 → 44.1 kHz", "Stop-band", "Flat to", "-3 dB at", "Residual", "State"], resample_rows([row for row in results if row["kind"] == "resample"])))
    add("")
    add("- The rubato 96-tap row has its window and cutoff chosen to match the stop-band and pass band of `Sinc`: it is the like-for-like comparison. The 64-tap row has the same length as `Sinc` and rubato's automatic cutoff.")
    add("- rubato's default and its `Fft` resampler have a deeper stop-band and a wider pass band than `Sinc`. `Fft` only handles fixed ratios, adds a chunk of latency and allocates, but it is the fastest high-quality option on this host, downsampling included.")
    add("- `Sinc` with `Fixed` is timed with `i16` in and out, its native format; its quality is limited by 16-bit samples and coefficients.")
    add("- dasp's `Sinc` does not filter before downsampling, and its residual stays high on low tones too: about -21 dB on a 1 kHz tone with 16 to 128 frames (checked by hand).")
    add("")
    add("### Spectrum")
    add("")
    add("Nanoseconds per transform of a mono 44.1 kHz `i16` signal: a Hann window, the real FFT and the power of every bin. `Spectrum` also takes the samples one frame at a time through `analyze_using` and keeps the last block in its own buffer, as it does in an application.")
    add("")
    fft = [row for row in results if row["kind"] == "fft"]
    libraries = []
    for row in fft:
        if row["library"] not in libraries:
            libraries.append(row["library"])
    rows = []
    for library in libraries:
        values = {row["points"]: row["median_ns"] / row["blocks"] for row in fft if row["library"] == library}
        rows.append([library, run.number(values.get(256)), run.number(values.get(1024))])
    add(run.markdown_table(["Library", "256 points", "1024 points"], rows))
    add("")
    add("realfft and rustfft use NEON here; microfft, which `Float` is built on, does not.")
    add("")
    add("### Sample operations")
    add("")
    add("The DSP operations of the section above, against dasp's per-sample `Sample` methods, in nanoseconds per sample.")
    add("")
    samples = [row for row in results if row["kind"] == "sample"]
    operations = []
    for row in samples:
        if row["operation"] not in operations:
            operations.append(row["operation"])
    rows = []
    for operation in operations:
        first = True
        for row in samples:
            if row["operation"] == operation:
                rows.append([operation if first else "", row["library"], run.number(row["median_ns"] / row["samples"])])
                first = False
    add(run.markdown_table(["Operation", "Library", "ns/sample"], rows))
    add("")
    add("dasp's `f32` to `i16` conversion truncates toward zero, while nanoaudio rounds to the nearest value, and dasp's `add_amp` wraps around on overflow where nanoaudio saturates.")
    add("")
    add("### Microcontrollers")
    add("")
    if counts:
        add("Instructions per unit from `tools/emu`, for the other crates that build for these targets on stable Rust. nanomp3-core is the frame decoder inside nanoaudio-mp3, and microfft the FFT inside `Spectrum` with `Float`: the difference is what nanoaudio adds around them.")
        add("")
        targets = [label for label in ["M0+", "M4F", "M33F", "RV32IMC"] if any(label in row for row in counts.values())]
        rows = []
        for _, name, library, case, unit in EMU_CASES:
            row = counts.get(name)
            if row:
                rows.append([case, unit, library] + [row.get(label, "n/a") for label in targets])
        add(run.markdown_table(["Case", "Unit", "Library"] + targets, rows))
        add("")
    add("The other crates do not run on these targets: symphonia, claxon, hound, qoaudio, rubato, rustfft and realfft need `std`, minimp3 is C, and dasp needs a nightly compiler without `std`.")
    add("")
    replace_section("\n".join(lines) + "\n")
    run.log("updated " + str(run.OUTPUT.relative_to(run.ROOT)))


def main():
    parser = argparse.ArgumentParser(description="Compare nanoaudio with other Rust audio crates and update docs/benchmarks.md.")
    parser.add_argument("--runs", type=int, default=11, help="timed runs per measurement")
    parser.add_argument("--target-ms", type=int, default=40, help="minimum duration of one run")
    parser.add_argument("--skip-emu", action="store_true", help="do not count instructions with tools/emu")
    arguments = parser.parse_args()

    started = time.monotonic()
    load = os.getloadavg()[0] if hasattr(os, "getloadavg") else 0.0
    host = run.host_triple()
    binary = run.bench_binary("compare", options=["--features", "compare"])
    notes = run.generate_data(binary)
    run.log("comparing")
    results = run.run_tool([str(binary)], ["compare", "--data", str(run.DATA), "--runs", str(arguments.runs), "--target-ms", str(arguments.target_ms)])
    counts = {} if arguments.skip_emu else emulate(notes)
    write(run.machine(load), notes, results, counts, time.monotonic() - started, arguments, host)


if __name__ == "__main__":
    main()
