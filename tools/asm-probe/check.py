import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

PROBE = Path(__file__).resolve().parent
ROOT = PROBE.parent.parent
TARGET_DIRECTORY = ROOT / "target" / "asm-probe"

TARGETS = {
    "m0plus": ("thumbv6m-none-eabi", "cortex-m0plus"),
    "m23": ("thumbv8m.base-none-eabi", "cortex-m23"),
    "m4": ("thumbv7em-none-eabihf", "cortex-m4"),
    "esp32c3": ("riscv32imc-unknown-none-elf", None),
    "m55": ("thumbv8m.main-none-eabihf", "cortex-m55"),
}

FIXED_NARROW = [
    "probe_fixed_volume_i16",
    "probe_fixed_mix_i16",
    "probe_fixed_volume_u8",
]

FIXED_I24 = [
    "probe_fixed_volume_i24",
    "probe_fixed_mix_i24",
]

SATURATING = [
    "probe_fixed_volume_i16",
    "probe_fixed_mix_i16",
]

CALL_FREE = FIXED_NARROW + [
    "probe_g711_decode_mu_law",
    "probe_g711_decode_a_law",
    "probe_g711_encode_mu_law",
    "probe_g711_encode_a_law",
]

HELIUM = [
    "probe_fixed_volume_i16",
    "probe_fixed_mix_i16",
    "probe_float_volume_i16",
    "probe_float_mix_i16",
    "probe_float_convert_i16",
]

HARD_FLOAT = ("m4", "m55")

FLOAT_CALL_FREE = [
    "probe_float_volume_i16",
    "probe_float_mix_i16",
    "probe_float_convert_i16",
]

FLOAT_STAGES = [
    "probe_float_stream_i16",
]

FLAT = [
    "probe_g711_encode_mu_law",
    "probe_g711_encode_a_law",
]

INLINED = [
    "probe_qoa_decode",
    "probe_qoa_decode_stereo",
]

NO_WIDE_MULTIPLY_IN_LOOPS = [
    "probe_fixed_resample_linear_i16",
    "probe_fixed_stream_resample_linear_i16",
    "probe_sbc_decode",
    "probe_sbc_encode",
]

SPLIT_GAIN = ("m0plus", "m23", "m4", "esp32c3")

NO_WIDE_MULTIPLY_IN_STAGE_LOOPS = [
    "probe_fixed_stream_i16",
    "probe_fixed_stream_i24",
    "probe_fixed_stream_i32",
    "probe_fixed_volume_i32",
    "probe_fixed_mix_i32",
]

NO_WIDE_DIVISION_IN_LOOPS = [
    "probe_sbc_decode",
]

NO_DIVISION_IN_LOOPS = [
    "probe_ms_adpcm_decode",
]

NO_DIVISION_IN_INNER_LOOPS = [
    "probe_ms_adpcm_encode",
]

SYNTH_FIXED = [
    "probe_synth_fixed_render",
]

SYNTH_FLOAT = [
    "probe_synth_float_render",
]

SCALAR = [
    "probe_qoa_decode",
    "probe_qoa_encode",
    "probe_qoa_decode_stereo",
    "probe_qoa_encode_stereo",
]

SAMPLE_CONVERSION = re.compile(r"6Sample\d+(from_f32|to_f32|from_i32|to_i32|convert)")
WIDE_MULTIPLY = re.compile(r"__aeabi_lmul|__muldi3")
WIDE_PRODUCT = re.compile(r"__aeabi_lmul|__muldi3|^(smull|umull|smlal|umlal|mulh|mulhu|mulhsu)\s")
WIDE_DIVISION = re.compile(r"__aeabi_ldivmod|__aeabi_uldivmod|__divdi3|__udivdi3|__moddi3|__umoddi3|__divmoddi4|__udivmoddi4")
DIVISION = re.compile(r"__aeabi_u?idiv|__aeabi_u?ldivmod|__u?(div|mod)[sd]i3|__u?divmod[sd]i4|div_rem")
MEMORY = re.compile(r"__aeabi_mem|memset|memcpy|memmove")
CALL = re.compile(r"^(bl|blx|call|jal|jalr|tail)$")
LABEL = re.compile(r"^(\.L\w+):")
BRANCH_TARGET = re.compile(r"^\.LBB\w+$")
SYMBOL = re.compile(r"^([A-Za-z_$][\w$.]*):")
VECTOR_REGISTER = re.compile(r"\bq[0-7]\b")
COMMENT = re.compile(r"\s+(@|#|//)\s.*$")


def build(name, opt_level):
    triple, cpu = TARGETS[name]
    environment = dict(os.environ)
    flags = environment.get("RUSTFLAGS", "") + " --emit=asm,link"
    if cpu:
        flags += " -C target-cpu=" + cpu
    environment["RUSTFLAGS"] = flags.strip()
    environment["CARGO_PROFILE_RELEASE_OPT_LEVEL"] = opt_level
    target_directory = TARGET_DIRECTORY / opt_level
    subprocess.run(
        [
            "cargo",
            "build",
            "--quiet",
            "--release",
            "--manifest-path",
            str(PROBE / "Cargo.toml"),
            "--target",
            triple,
            "--target-dir",
            str(target_directory),
        ],
        check=True,
        env=environment,
    )
    newest = {}
    for path in (target_directory / triple / "release" / "deps").glob("nanoaudio_*.s"):
        crate = path.name.rsplit("-", 1)[0]
        if crate not in newest or path.stat().st_mtime > newest[crate].stat().st_mtime:
            newest[crate] = path
    if "nanoaudio_asm_probe" not in newest:
        sys.exit("no assembly produced for " + triple)
    return "\n".join(path.read_text() for path in newest.values())


def functions(assembly):
    result = {}
    current = None
    for line in assembly.splitlines():
        line = COMMENT.sub("", line)
        stripped = line.strip()
        if not stripped or stripped.startswith(("@", "#", "//")):
            continue
        symbol = SYMBOL.match(line)
        if symbol:
            current = symbol.group(1)
            result[current] = []
            continue
        if current is None:
            continue
        if stripped.startswith(".Lfunc_end"):
            current = None
            continue
        label = LABEL.match(stripped)
        if label:
            result[current].append(label.group(1) + ":")
        elif not stripped.startswith("."):
            result[current].append(stripped)
    return result


def instructions(lines):
    return [line for line in lines if not LABEL.match(line)]


def operands(line):
    return re.split(r"[\s,{}]+", line)[1:]


def terminates(line):
    mnemonic = line.split()[0]
    if mnemonic in ("b", "b.w", "b.n", "bx", "j", "jr", "ret", "tail", "mret"):
        return True
    return mnemonic in ("pop", "pop.w", "ldm", "ldm.w") and "pc" in operands(line)


def blocks(lines):
    result = []
    current = ([], [])
    for line in lines:
        label = LABEL.match(line)
        if label:
            if current[1]:
                result.append(current)
                current = ([], [])
            current[0].append(label.group(1))
            continue
        current[1].append(line)
        if terminates(line) or any(BRANCH_TARGET.match(operand) for operand in operands(line)):
            result.append(current)
            current = ([], [])
    if current[0] or current[1]:
        result.append(current)
    index = {label: position for position, block in enumerate(result) for label in block[0]}
    successors = []
    for position, (_, body) in enumerate(result):
        last = body[-1] if body else ""
        targets = [index[operand] for operand in operands(last) if operand in index] if last else []
        if not (last and terminates(last)) and position + 1 < len(result):
            targets.append(position + 1)
        successors.append(targets)
    return [body for _, body in result], successors


def loops(lines):
    bodies, successors = blocks(lines)
    count = len(bodies)
    predecessors = [[] for _ in range(count)]
    for source, targets in enumerate(successors):
        for target in targets:
            predecessors[target].append(source)
    everything = set(range(count))
    dominators = [set(everything) for _ in range(count)]
    if count:
        dominators[0] = {0}
    changed = True
    while changed:
        changed = False
        for node in range(1, count):
            incoming = [dominators[predecessor] for predecessor in predecessors[node]]
            updated = (set.intersection(*incoming) if incoming else set()) | {node}
            if updated != dominators[node]:
                dominators[node] = updated
                changed = True
    found = []
    for latch, targets in enumerate(successors):
        for header in targets:
            if header not in dominators[latch]:
                continue
            members = {header}
            pending = [latch]
            while pending:
                node = pending.pop()
                if node not in members:
                    members.add(node)
                    pending.extend(predecessors[node])
            found.append([line for member in sorted(members) for line in bodies[member]])
    return found


def nested(lines):
    found = loops(lines)
    return [body for body in found if any(len(other) < len(body) and set(other) <= set(body) for other in found)]


def innermost(lines):
    found = loops(lines)
    return [body for body in found if not any(len(other) < len(body) and set(other) <= set(body) for other in found)]


def calls(lines):
    return [line for line in instructions(lines) if CALL.match(line.split()[0])]


def closure(bodies, function):
    pending = [function]
    seen = []
    while pending:
        current = pending.pop()
        if current in seen or current not in bodies:
            continue
        seen.append(current)
        for line in instructions(bodies[current]):
            pending.extend(operand for operand in re.split(r"[\s,]+", line)[1:] if operand in bodies)
    return seen


def analyse(name, assembly, opt_level):
    failures = []
    report = []
    bodies = functions(assembly)
    probes = sorted(function for function in bodies if function.startswith("probe_"))
    for function in probes:
        reached = closure(bodies, function)
        total = sum(len(instructions(bodies[member])) for member in reached)
        sizes = [len(instructions(body)) for member in reached for body in loops(bodies[member])]
        report.append("{:<8} {:<26} instructions {:>4}  loops {}".format(name, function, total, sizes))
    expected = set(
        CALL_FREE + HELIUM + FIXED_I24 + FLAT + INLINED + SCALAR + NO_WIDE_MULTIPLY_IN_LOOPS
        + NO_WIDE_MULTIPLY_IN_STAGE_LOOPS + NO_WIDE_DIVISION_IN_LOOPS + NO_DIVISION_IN_LOOPS
        + NO_DIVISION_IN_INNER_LOOPS
        + SYNTH_FIXED + SYNTH_FLOAT
    )
    for function in sorted(expected - set(probes)):
        failures.append(name + ": missing " + function)
    for function in FIXED_NARROW:
        lines = [line for member in closure(bodies, function) for line in bodies[member]]
        if any(WIDE_MULTIPLY.search(line) for line in lines):
            failures.append(name + ": 64-bit multiply in " + function)
        if name == "m4" and function in SATURATING and not any(line.startswith("ssat") for line in lines):
            failures.append(name + ": no ssat in " + function)
    for function in FIXED_I24:
        lines = [line for member in closure(bodies, function) for line in bodies[member]]
        if any(WIDE_MULTIPLY.search(line) for line in lines):
            failures.append(name + ": 64-bit multiply in " + function)
    for function in NO_WIDE_MULTIPLY_IN_LOOPS:
        for member in closure(bodies, function):
            for body in loops(bodies[member]):
                if any(WIDE_MULTIPLY.search(line) for line in body):
                    failures.append(name + ": 64-bit multiply in a loop of " + function)
    if name in SPLIT_GAIN:
        for function in NO_WIDE_MULTIPLY_IN_STAGE_LOOPS:
            for member in closure(bodies, function):
                for body in loops(bodies[member]):
                    if any(WIDE_PRODUCT.search(line) for line in body):
                        failures.append(name + ": 64-bit multiply in a loop of " + function)
    for function in NO_WIDE_DIVISION_IN_LOOPS:
        for member in closure(bodies, function):
            for body in loops(bodies[member]):
                if any(WIDE_DIVISION.search(line) for line in body):
                    failures.append(name + ": 64-bit division in a loop of " + function)
    for function in NO_DIVISION_IN_LOOPS:
        for member in closure(bodies, function):
            for body in loops(bodies[member]):
                for call in calls(body):
                    if DIVISION.search(call):
                        failures.append(name + ": division call in a loop of " + function + ": " + call)
    for function in NO_DIVISION_IN_INNER_LOOPS:
        for member in closure(bodies, function):
            for body in innermost(bodies[member]):
                for call in calls(body):
                    if DIVISION.search(call):
                        failures.append(name + ": division call in an inner loop of " + function + ": " + call)
    for function in SYNTH_FIXED + SYNTH_FLOAT:
        if function in SYNTH_FLOAT and name not in HARD_FLOAT:
            continue
        lines = [line for member in closure(bodies, function) for line in instructions(bodies[member])]
        for call in calls(lines):
            inside = opt_level in ("s", "z") and "15nanoaudio_synth" in call
            if not MEMORY.search(call) and not inside:
                failures.append(name + ": call in " + function + ": " + call)
        if name in SPLIT_GAIN[:2] and any(WIDE_PRODUCT.search(line) for line in lines):
            failures.append(name + ": 64-bit multiply in " + function)
    for function in FLAT:
        for member in closure(bodies, function):
            if nested(bodies[member]):
                failures.append(name + ": loop inside the sample loop of " + function)
    for function in CALL_FREE:
        for member in closure(bodies, function):
            for body in loops(bodies[member]):
                for call in calls(body):
                    failures.append(name + ": call in hot loop of " + function + ": " + call)
    for function in FLOAT_CALL_FREE + FLOAT_STAGES:
        if function not in probes:
            failures.append(name + ": missing " + function)
    if name in HARD_FLOAT:
        for function in FLOAT_CALL_FREE:
            for member in closure(bodies, function):
                for body in loops(bodies[member]):
                    for call in calls(body):
                        failures.append(name + ": call in hot loop of " + function + ": " + call)
        for function in FLOAT_STAGES:
            for member in closure(bodies, function):
                for body in loops(bodies[member]):
                    for call in calls(body):
                        if SAMPLE_CONVERSION.search(call):
                            failures.append(name + ": sample conversion call in a loop of " + function + ": " + call)
    if opt_level not in ("s", "z"):
        for function in INLINED:
            for member in closure(bodies, function):
                for body in innermost(bodies[member]):
                    for call in calls(body):
                        failures.append(name + ": call in hot loop of " + function + ": " + call)
    if name == "m55":
        for function in SCALAR:
            for member in closure(bodies, function):
                for body in loops(bodies[member]):
                    if any(VECTOR_REGISTER.search(line) for line in instructions(body)):
                        failures.append(name + ": vector instructions in hot loop of " + function)
    if name == "m55" and opt_level not in ("s", "z"):
        for function in HELIUM:
            reached = closure(bodies, function)
            if not any(VECTOR_REGISTER.search(line) for member in reached for line in instructions(bodies[member])):
                failures.append(name + ": no Helium vector instructions in " + function)
    return report, failures


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("targets", nargs="*", default=list(TARGETS))
    parser.add_argument("--opt-level", default="3")
    parser.add_argument("--dump", metavar="FUNCTION")
    arguments = parser.parse_args()
    failures = []
    for name in arguments.targets:
        assembly = build(name, arguments.opt_level)
        if arguments.dump:
            bodies = functions(assembly)
            for member in closure(bodies, arguments.dump):
                print(member + ":")
                print("\n".join(bodies[member]))
            continue
        report, problems = analyse(name, assembly, arguments.opt_level)
        print("\n".join(report))
        failures.extend(problems)
    for failure in failures:
        print("error: " + failure, file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
