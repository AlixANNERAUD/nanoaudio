# Instruction counts

Runs the codecs and the DSP and analysis paths on emulated microcontrollers, with [Unicorn](https://www.unicorn-engine.org/), and counts the instructions they execute per frame or per block. It catches what static instruction counts miss: library calls inside loops (software division, soft float, `memcpy`), and paths that only run on real data.

The counts are not cycles: there are no wait states, caches or pipeline stalls, and a multiply or a division counts as one instruction like an addition. They are good for comparing two versions of the code on one target, or for a first estimate of the clock a codec needs.

## Running

The script builds `src/main.rs` for each target with `cargo`, then runs it in Python with `unicorn` and `pyelftools`. Install them in an isolated environment, not system-wide. With [uv](https://docs.astral.sh/uv/), from the repository root:

```sh
uv run --isolated --no-project --with-requirements tools/emu/requirements.txt python tools/emu/run.py
```

Or with a virtual environment inside this directory (ignored by git):

```sh
python3 -m venv tools/emu/.venv
tools/emu/.venv/bin/pip install -r tools/emu/requirements.txt
tools/emu/.venv/bin/python tools/emu/run.py
```

The Rust targets must be installed: `thumbv6m-none-eabi`, `thumbv7em-none-eabihf`, `thumbv8m.main-none-eabihf` and `riscv32imc-unknown-none-elf`. A full run takes under a minute on a laptop.

Options:

- `--targets m0plus,m4f`: only some targets, out of `m0plus`, `m4f`, `m33` and `rv32imc`.
- `--filter sbc`: only the cases whose group (`codec`, `dsp`, `analysis`) or name contains the text.
- `--opt-level s`: builds with another optimization level, `3` by default.
- `--functions 5`: also lists, for each case and target, the 5 functions that execute the most instructions. With LTO most code is inlined into its caller, so a codec often shows up under the harness function that calls it.
- `--no-build`: reuses the binaries from the last build, in `target/emu`.

## Output

The first table gives instructions per unit: per sample for G.711 and G.722, per codec frame or block for the others (its length in frames is in brackets), per frame for DSP and analysis, and per input frame for the resamplers. A frame is one sample on every channel.

The second table gives millions of instructions per second of audio at the rate of the case. On a core that runs about one instruction per cycle, it is a lower bound for the clock in MHz.

The `Check` column compares decoded fixtures with their reference (bit-exact for FLAC, the signal-to-noise ratio for SBC), and says if a codec gives different output on different targets.

## Targets

| Target | Rust target | Unicorn model |
| --- | --- | --- |
| Cortex-M0+ | `thumbv6m-none-eabi` | Cortex-M0, which has the same instruction set |
| Cortex-M4F | `thumbv7em-none-eabihf` | Cortex-M4 with its FPU |
| Cortex-M33F | `thumbv8m.main-none-eabihf` | Cortex-M33 with its FPU |
| RV32IMC | `riscv32imc-unknown-none-elf` | Generic RV32 |

Unicorn 2.1 has no Cortex-M55 model and does not run Helium (MVE) instructions, so the M55 is not covered.

An instruction skipped by an IT block counts as executed, since the core still issues it.

## Cases

Codecs encode a generated signal (chords with harmonics and a little noise) and decode what they encoded on the same target, except G.722, SBC, FLAC and MP3, which decode the fixtures in `crates/*/tests/data`. Inputs are short, a few frames each, to keep a run fast: the numbers are averages over those frames, setup excluded.

DSP and analysis cases run on 44.1 kHz stereo 16-bit audio, in fixed point and in floating point. Floating point on the M0+ and RV32IMC goes through software routines, which is where most of their cost comes from.

Only the code between the `begin` and `end` calls in `src/main.rs` is counted. To add a case, add an arm to `run` in `src/main.rs` and a `Case` with the same number to `CASES` in `run.py`.
