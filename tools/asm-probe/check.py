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

FLAT = [
    "probe_g711_encode_mu_law",
    "probe_g711_encode_a_law",
]

WIDE_MULTIPLY = re.compile(r"__aeabi_lmul|__muldi3")
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
    expected = set(CALL_FREE + HELIUM + FIXED_I24)
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
    for function in FLAT:
        for member in closure(bodies, function):
            if nested(bodies[member]):
                failures.append(name + ": loop inside the sample loop of " + function)
    for function in CALL_FREE:
        for member in closure(bodies, function):
            for body in loops(bodies[member]):
                for call in calls(body):
                    failures.append(name + ": call in hot loop of " + function + ": " + call)
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
