import argparse
import bisect
import math
import os
import struct
import subprocess
import sys
import time
from pathlib import Path

from elftools.elf.elffile import ELFFile
from unicorn import (
    UC_ARCH_ARM,
    UC_ARCH_RISCV,
    UC_HOOK_BLOCK,
    UC_MODE_MCLASS,
    UC_MODE_RISCV32,
    UC_MODE_THUMB,
    Uc,
    UcError,
)
from unicorn import arm_const, riscv_const

TOOL = Path(__file__).resolve().parent
ROOT = TOOL.parent.parent
TARGET_DIRECTORY = ROOT / "target" / "emu"
DATA = ROOT / "crates"

STACK_TOP = 0x7010_0000
STACK_SIZE = 0x10_0000
SYSTEM = 0xE000_0000
TIMEOUT = 600 * 1_000_000
FAILED = 0xFFFF_FFFF


class Target:
    def __init__(self, key, label, triple, architecture, cpu):
        self.key = key
        self.label = label
        self.triple = triple
        self.architecture = architecture
        self.cpu = cpu

    @property
    def thumb(self):
        return self.architecture == "arm"


TARGETS = [
    Target("m0plus", "M0+", "thumbv6m-none-eabi", "arm", arm_const.UC_CPU_ARM_CORTEX_M0),
    Target("m4f", "M4F", "thumbv7em-none-eabihf", "arm", arm_const.UC_CPU_ARM_CORTEX_M4),
    Target("m33", "M33F", "thumbv8m.main-none-eabihf", "arm", arm_const.UC_CPU_ARM_CORTEX_M33),
    Target("rv32imc", "RV32IMC", "riscv32imc-unknown-none-elf", "riscv", None),
]


class Case:
    def __init__(self, identifier, group, name, unit, unit_frames, rate, channels, source, encoder=None, reference=None):
        self.identifier = identifier
        self.group = group
        self.name = name
        self.unit = unit
        self.unit_frames = unit_frames
        self.rate = rate
        self.channels = channels
        self.source = source
        self.encoder = encoder
        self.reference = reference

    @property
    def decodes(self):
        return self.name.endswith("decode") or " decode" in self.name

    @property
    def encodes(self):
        return " encode" in self.name


def signal(rate, channels, frames):
    chords = [
        (220.00, 277.18, 329.63, 440.00),
        (196.00, 246.94, 293.66, 392.00),
        (174.61, 220.00, 261.63, 349.23),
        (164.81, 207.65, 246.94, 329.63),
    ]
    state = [0x1234_5678 ^ ((channel * 0x9E37_79B9) & 0xFFFF_FFFF) for channel in range(channels)]
    samples = []
    for frame in range(frames):
        chord = chords[(frame * 4 // rate) % len(chords)]
        time_ = frame / rate
        for channel in range(channels):
            state[channel] = (state[channel] * 1_664_525 + 1_013_904_223) & 0xFFFF_FFFF
            noise = (state[channel] >> 8) / (1 << 23) - 1.0
            tone = 0.0
            for note, frequency in enumerate(chord):
                weight = 1.0 if (note + channel) % 2 == 0 else 0.8
                for harmonic, amplitude in enumerate((1.0, 0.45, 0.25, 0.12)):
                    partial = frequency * (harmonic + 1)
                    if partial < rate / 2:
                        tone += weight * amplitude * math.sin(2 * math.pi * partial * time_)
            value = 4_000 * tone + 300 * noise
            samples.append(max(-32_768, min(32_767, int(round(value)))))
    return struct.pack(f"<{len(samples)}h", *samples)


def fixture(crate, name, limit=None):
    data = (DATA / crate / "tests" / "data" / name).read_bytes()
    return data[:limit] if limit else data


CASES = [
    Case(0, "codec", "G.711 µ-law encode", "sample", 1, 8_000, 1, ("signal", 8_000, 1, 2_048)),
    Case(1, "codec", "G.711 µ-law decode", "sample", 1, 8_000, 1, ("encoded",), encoder=0),
    Case(2, "codec", "G.711 A-law encode", "sample", 1, 8_000, 1, ("signal", 8_000, 1, 2_048)),
    Case(3, "codec", "G.711 A-law decode", "sample", 1, 8_000, 1, ("encoded",), encoder=2),
    Case(4, "codec", "IMA ADPCM encode, 16 kHz mono", "block (505)", 505, 16_000, 1, ("signal", 16_000, 1, 2_020)),
    Case(5, "codec", "IMA ADPCM decode, 16 kHz mono", "block (505)", 505, 16_000, 1, ("encoded",), encoder=4),
    Case(6, "codec", "MS ADPCM encode, 44.1 kHz stereo", "block (2036)", 2_036, 44_100, 2, ("signal", 44_100, 2, 4_072)),
    Case(7, "codec", "MS ADPCM decode, 44.1 kHz stereo", "block (2036)", 2_036, 44_100, 2, ("encoded",), encoder=6),
    Case(8, "codec", "QOA encode, 44.1 kHz stereo", "frame (5120)", 5_120, 44_100, 2, ("signal", 44_100, 2, 5_120)),
    Case(9, "codec", "QOA decode, 44.1 kHz stereo", "frame (5120)", 5_120, 44_100, 2, ("encoded",), encoder=8),
    Case(10, "codec", "G.722 encode, 64 kbit/s", "sample", 1, 16_000, 1, ("signal", 16_000, 1, 2_048)),
    Case(11, "codec", "G.722 decode, 64 kbit/s", "sample", 1, 16_000, 1, ("file", "nanoaudio-g722", "g722_encoded.bin", 2_048)),
    Case(12, "codec", "SBC encode, 44.1 kHz joint stereo", "frame (128)", 128, 44_100, 2, ("file", "nanoaudio-sbc", "sbc_44100_joint_stereo.pcm", 4_096)),
    Case(13, "codec", "SBC decode, 44.1 kHz joint stereo", "frame (128)", 128, 44_100, 2, ("file", "nanoaudio-sbc", "sbc_44100_joint_stereo.sbc", 119 * 8), reference=("nanoaudio-sbc", "sbc_44100_joint_stereo.pcm")),
    Case(14, "codec", "SBC encode, mSBC", "frame (120)", 120, 16_000, 1, ("file", "nanoaudio-sbc", "msbc.pcm", 240 * 8)),
    Case(15, "codec", "SBC decode, mSBC", "frame (120)", 120, 16_000, 1, ("file", "nanoaudio-sbc", "msbc.sbc", 57 * 8), reference=("nanoaudio-sbc", "msbc.pcm")),
    Case(16, "codec", "FLAC decode, 44.1 kHz stereo 16-bit", "block (4096)", 4_096, 44_100, 2, ("file", "nanoaudio-flac", "flac_stereo_16.flac", None), reference=("nanoaudio-flac", "flac_stereo_16.bin")),
    Case(17, "codec", "MP3 decode, 44.1 kHz stereo 128 kbit/s", "frame (1152)", 1_152, 44_100, 2, ("file", "nanoaudio-mp3", "gapless_cbr.mp3", 418 * 4)),
    Case(32, "dsp", "Convert i16 to f32", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(33, "dsp", "Convert f32 to i16, dithered", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(34, "dsp", "Volume, fixed", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(35, "dsp", "Volume, float", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(36, "dsp", "Mix, fixed", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(37, "dsp", "Mix, float", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(38, "dsp", "Pipeline (volume, upmix, mix), fixed", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(39, "dsp", "Pipeline (volume, upmix, mix), float", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(40, "dsp", "Resample 44.1 to 48 kHz, linear, fixed", "input frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(41, "dsp", "Resample 44.1 to 48 kHz, linear, float", "input frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(42, "dsp", "Resample 44.1 to 48 kHz, sinc, fixed", "input frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(43, "dsp", "Resample 44.1 to 48 kHz, sinc, float", "input frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(44, "dsp", "Resample 48 to 16 kHz, sinc, fixed", "input frame", 1, 48_000, 2, ("signal", 48_000, 2, 1_024)),
    Case(45, "dsp", "Resample 48 to 16 kHz, sinc, float", "input frame", 1, 48_000, 2, ("signal", 48_000, 2, 1_024)),
    Case(64, "analysis", "Level meter, fixed", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(65, "analysis", "Level meter, float", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(66, "analysis", "Ballistic meter (PPM), fixed", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(67, "analysis", "Ballistic meter (PPM), float", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(68, "analysis", "10 octave bands, fixed", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(69, "analysis", "10 octave bands, float", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(70, "analysis", "DTMF detector, fixed", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(71, "analysis", "DTMF detector, float", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(72, "analysis", "Stereo meter, fixed", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(73, "analysis", "Stereo meter, float", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(74, "analysis", "Voice activity detector", "frame", 1, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(75, "analysis", "Spectrum, 1024 points, fixed", "block (1024)", 1_024, 44_100, 2, ("signal", 44_100, 2, 2_048)),
    Case(76, "analysis", "Spectrum, 1024 points, float", "block (1024)", 1_024, 44_100, 2, ("signal", 44_100, 2, 2_048)),
    Case(77, "analysis", "Spectrum, 256 points, fixed", "block (256)", 256, 44_100, 2, ("signal", 44_100, 2, 1_024)),
    Case(78, "analysis", "Spectrum, 256 points, float", "block (256)", 256, 44_100, 2, ("signal", 44_100, 2, 1_024)),
]


def build(targets, opt_level):
    environment = dict(os.environ, CARGO_PROFILE_RELEASE_OPT_LEVEL=opt_level)
    for target in targets:
        subprocess.run(
            [
                "cargo",
                "build",
                "--quiet",
                "--release",
                "--manifest-path",
                str(TOOL / "Cargo.toml"),
                "--target",
                target.triple,
                "--target-dir",
                str(TARGET_DIRECTORY / opt_level),
            ],
            check=True,
            env=environment,
        )


class Binary:
    def __init__(self, path, target):
        self.target = target
        with open(path, "rb") as file:
            elf = ELFFile(file)
            self.segments = [
                (segment["p_vaddr"], segment["p_memsz"], segment.data())
                for segment in elf.iter_segments()
                if segment["p_type"] == "PT_LOAD"
            ]
            self.symbols = {}
            functions = []
            for symbol in elf.get_section_by_name(".symtab").iter_symbols():
                self.symbols[symbol.name] = symbol["st_value"]
                if symbol["st_info"]["type"] == "STT_FUNC":
                    functions.append((symbol["st_value"] & ~1, symbol.name))
        functions.sort()
        self.starts = [start for start, _ in functions]
        self.names = [name for _, name in functions]
        self.lengths = {}

    def address(self, name):
        return self.symbols[name] & ~1

    def instructions(self, memory, address, size):
        key = (address << 16) | size
        count = self.lengths.get(key)
        if count is not None:
            return count
        data = bytes(memory.mem_read(address, size))
        count = 0
        offset = 0
        while offset < size:
            halfword = data[offset] | (data[offset + 1] << 8)
            if self.target.thumb:
                wide = (halfword >> 11) in (0b11101, 0b11110, 0b11111)
            else:
                wide = (halfword & 0b11) == 0b11
            offset += 4 if wide else 2
            count += 1
        self.lengths[key] = count
        return count

    def function(self, address):
        index = bisect.bisect_right(self.starts, address) - 1
        return self.names[index] if index >= 0 else "?"


def emulator(binary):
    target = binary.target
    if target.thumb:
        machine = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
        machine.ctl_set_cpu_model(target.cpu)
        stack = arm_const.UC_ARM_REG_SP
    else:
        machine = Uc(UC_ARCH_RISCV, UC_MODE_RISCV32)
        stack = riscv_const.UC_RISCV_REG_SP
    mapped = set()
    for address, size, data in binary.segments:
        for page in range(address & ~0xFFF, (address + size + 0xFFF) & ~0xFFF, 0x1000):
            if page not in mapped:
                machine.mem_map(page, 0x1000)
                mapped.add(page)
        machine.mem_write(address, data)
    machine.mem_map(STACK_TOP - STACK_SIZE, STACK_SIZE)
    if target.thumb:
        try:
            machine.mem_map(SYSTEM, 0x10_0000)
        except UcError:
            pass
    machine.reg_write(stack, STACK_TOP)
    return machine


def execute(binary, case, payload, profile):
    machine = emulator(binary)
    machine.mem_write(binary.symbols["INPUT"], payload)
    machine.mem_write(binary.symbols["LENGTH"], len(payload).to_bytes(4, "little"))
    machine.mem_write(binary.symbols["CASE"], case.identifier.to_bytes(4, "little"))
    thumb = 1 if binary.target.thumb else 0
    begin = binary.address("begin")
    end = binary.address("end")
    finished = binary.address("finished")
    program_counter = arm_const.UC_ARM_REG_PC if binary.target.thumb else riscv_const.UC_RISCV_REG_PC

    def run(start, until):
        machine.emu_start(start | thumb, until, timeout=TIMEOUT)
        if machine.reg_read(program_counter) & ~1 != until:
            raise RuntimeError(f"{case.name} on {binary.target.label} stopped at {machine.reg_read(program_counter):#x}")

    run(binary.address("_start"), begin)
    total = [0]
    functions = {}
    if profile:
        def hook(memory, address, size, _):
            count = binary.instructions(memory, address, size)
            total[0] += count
            name = binary.function(address)
            functions[name] = functions.get(name, 0) + count
    else:
        def hook(memory, address, size, _):
            total[0] += binary.instructions(memory, address, size)

    handle = machine.hook_add(UC_HOOK_BLOCK, hook)
    run(begin, end)
    machine.hook_del(handle)
    run(end, finished)
    result = int.from_bytes(machine.mem_read(binary.symbols["RESULT"], 4), "little")
    if result == FAILED:
        raise RuntimeError(f"{case.name} failed on {binary.target.label}")
    output = bytes(machine.mem_read(binary.symbols["OUTPUT"], result))
    return total[0], output, functions


def frames(case, payload, output):
    if case.decodes:
        return len(output) // 2 // case.channels
    return len(payload) // 2 // case.channels


def snr(decoded, reference):
    count = min(len(decoded), len(reference)) // 2
    decoded = struct.unpack(f"<{count}h", decoded[: count * 2])
    reference = struct.unpack(f"<{count}h", reference[: count * 2])
    power = sum(value * value for value in reference)
    noise = sum((a - b) ** 2 for a, b in zip(decoded, reference))
    return math.inf if noise == 0 else 10 * math.log10(power / noise)


def check(case, outputs):
    notes = []
    values = list(outputs.values())
    if case.group == "codec" and any(value != values[0] for value in values[1:]):
        differing = [label for label, value in outputs.items() if value != values[0]]
        notes.append("output differs on " + ", ".join(differing))
    if case.reference:
        reference = fixture(*case.reference)
        ratio = snr(values[0], reference)
        notes.append("bit-exact" if ratio == math.inf else f"SNR {ratio:.1f} dB")
    return "; ".join(notes)


def payload_for(case, outputs_by_case, target):
    kind = case.source[0]
    if kind == "signal":
        return signal(*case.source[1:])
    if kind == "file":
        return fixture(*case.source[1:])
    return outputs_by_case[(case.encoder, target.key)]


def number(value):
    if value >= 100:
        return f"{value:,.0f}"
    return f"{value:.1f}"


def table(headers, rows, alignments):
    widths = [max(len(str(row[index])) for row in [headers] + rows) for index in range(len(headers))]
    lines = []
    for row in [headers]:
        lines.append("| " + " | ".join(str(cell).ljust(widths[index]) for index, cell in enumerate(row)) + " |")
    lines.append("|" + "|".join(("-" * (widths[index] + 1) + ":") if alignments[index] == "right" else ("-" * (widths[index] + 2)) for index in range(len(headers))) + "|")
    for row in rows:
        cells = []
        for index, cell in enumerate(row):
            cell = str(cell)
            cells.append(cell.rjust(widths[index]) if alignments[index] == "right" else cell.ljust(widths[index]))
        lines.append("| " + " | ".join(cells) + " |")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Counts instructions executed per frame on emulated microcontrollers.")
    parser.add_argument("--targets", default=",".join(target.key for target in TARGETS), help="comma-separated targets: " + ", ".join(target.key for target in TARGETS))
    parser.add_argument("--filter", default="", help="only cases whose group or name contains this text, case-insensitive")
    parser.add_argument("--opt-level", default="3", help="optimization level, 3 by default")
    parser.add_argument("--functions", type=int, default=0, metavar="N", help="also list the N functions that execute the most instructions in each case")
    parser.add_argument("--no-build", action="store_true", help="use the binaries already built")
    arguments = parser.parse_args()

    keys = arguments.targets.split(",")
    targets = [target for target in TARGETS if target.key in keys]
    if len(targets) != len(keys):
        parser.error(f"unknown target in {arguments.targets}")
    selected = [
        case
        for case in CASES
        if arguments.filter.lower() in case.group or arguments.filter.lower() in case.name.lower()
    ]
    needed = {case.encoder for case in selected if case.encoder is not None}
    cases = [case for case in CASES if case in selected or case.identifier in needed]

    started = time.monotonic()
    if not arguments.no_build:
        build(targets, arguments.opt_level)
    binaries = {
        target.key: Binary(TARGET_DIRECTORY / arguments.opt_level / target.triple / "release" / "nanoaudio-emu", target)
        for target in targets
    }

    counts = {}
    outputs_by_case = {}
    profiles = []
    notes = {}
    for case in cases:
        outputs = {}
        for target in targets:
            payload = payload_for(case, outputs_by_case, target)
            total, output, functions = execute(binaries[target.key], case, payload, arguments.functions > 0)
            outputs_by_case[(case.identifier, target.key)] = output
            outputs[target.label] = output
            measured = frames(case, payload, output)
            counts[(case.identifier, target.key)] = total / measured
            if arguments.functions and case in selected:
                top = sorted(functions.items(), key=lambda item: -item[1])[: arguments.functions]
                profiles.append((case.name, target.label, [(name, count / measured * case.unit_frames) for name, count in top]))
        notes[case.identifier] = check(case, outputs)

    rows = []
    for case in selected:
        rows.append(
            [case.name, case.unit]
            + [number(counts[(case.identifier, target.key)] * case.unit_frames) for target in targets]
            + [notes[case.identifier]]
        )
    print(f"Instructions per unit, opt-level {arguments.opt_level}:\n")
    print(table(["Case", "Unit"] + [target.label for target in targets] + ["Check"], rows, ["left", "left"] + ["right"] * len(targets) + ["left"]))

    rows = []
    for case in selected:
        rows.append(
            [case.name, f"{case.rate / 1000:g} kHz"]
            + [f"{counts[(case.identifier, target.key)] * case.rate / 1e6:.2f}" for target in targets]
        )
    print("\nMillions of instructions per second of audio, in real time:\n")
    print(table(["Case", "Rate"] + [target.label for target in targets], rows, ["left", "left"] + ["right"] * len(targets)))

    for name, label, top in profiles:
        print(f"\n{name} on {label}, instructions per unit:")
        for function, count in top:
            print(f"  {number(count):>12}  {function}")

    print(f"\nMeasured in {time.monotonic() - started:.0f} s.", file=sys.stderr)


if __name__ == "__main__":
    main()
