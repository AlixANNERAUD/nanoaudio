import math
import os
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path

import numpy
import soundfile

directory = Path(__file__).parent


def chunks(file):
    offset = 12
    while offset < len(file):
        identifier = file[offset:offset + 4]
        size = struct.unpack("<I", file[offset + 4:offset + 8])[0]
        yield identifier, offset + 8, size
        offset += 8 + size + (size & 1)


def chunk(file, name):
    return next((start, size) for identifier, start, size in chunks(file) if identifier == name)


def signal(frames, channels):
    state = 4242
    values = []
    for index in range(frames):
        frame = []
        for channel in range(channels):
            state = (state * 1103515245 + 12345) & 0x7FFFFFFF
            noise = (state >> 16) % 801 - 400
            tone = 12000 * math.sin(2 * math.pi * (440 + 330 * channel) * index / 8000)
            chirp = 6000 * math.sin(2 * math.pi * (100 + index / 4) * index / 8000)
            burst = 30000 if 700 <= index < 720 else 0
            frame.append(max(-32768, min(32767, int(tone + chirp + noise) + burst)))
        values.append(frame)
    return numpy.array(values, dtype=numpy.int16)


def read(path):
    decoded, _ = soundfile.read(path, dtype="int16", always_2d=True)
    return decoded


def save(name, decoded):
    (directory / name).write_bytes(decoded.astype("<i2").tobytes())


for name, channels in (("mono", 1), ("stereo", 2)):
    path = directory / f"ms_adpcm_{name}.wav"
    soundfile.write(path, signal(1300, channels), 8000, subtype="MS_ADPCM")
    save(f"ms_adpcm_{name}_decoded.bin", read(path))

stereo = (directory / "ms_adpcm_stereo.wav").read_bytes()
format_start, _ = chunk(stereo, b"fmt ")
data_start, data_size = chunk(stereo, b"data")
block_align = struct.unpack("<H", stereo[format_start + 12:format_start + 14])[0]
count = struct.unpack("<H", stereo[format_start + 20:format_start + 22])[0]
table = format_start + 22
pairs = [stereo[table + 4 * index:table + 4 * index + 4] for index in range(count)]

permuted = bytearray(stereo)
permuted[table:table + 4 * count] = b"".join(reversed(pairs))
for block in range(data_start, data_start + data_size, block_align):
    for channel in range(2):
        permuted[block + channel] = count - 1 - permuted[block + channel]
(directory / "ms_adpcm_permuted.wav").write_bytes(bytes(permuted))
assert (read(directory / "ms_adpcm_permuted.wav") != read(directory / "ms_adpcm_stereo.wav")).any()

mono = (directory / "ms_adpcm_mono.wav").read_bytes()
data_start, _ = chunk(mono, b"data")
generator = numpy.random.default_rng(7)
corrupt = bytearray(mono)
for block in range(2):
    start = data_start + block * 256
    corrupt[start:start + 256] = generator.integers(0, 256, 256, dtype=numpy.uint8).tobytes()
    corrupt[start] = block * 5
    corrupt[start + 1:start + 3] = struct.pack("<h", 16 + block * 20000)
(directory / "ms_adpcm_corrupt.wav").write_bytes(bytes(corrupt))
save("ms_adpcm_corrupt_decoded.bin", read(directory / "ms_adpcm_corrupt.wav"))

(directory / "ms_adpcm_input.bin").write_bytes(signal(1000, 2).astype("<i2").tobytes())

with tempfile.TemporaryDirectory() as encoded_directory:
    subprocess.run(
        ["cargo", "test", "--test", "ms_adpcm", "encoder_matches_libsndfile"],
        cwd=directory.parent.parent,
        env=os.environ | {"NANOAUDIO_GENERATED": encoded_directory},
        check=False,
    )
    for name in ("mono", "stereo"):
        source = Path(encoded_directory) / f"ms_adpcm_encoded_{name}.wav"
        shutil.copyfile(source, directory / source.name)
        save(f"ms_adpcm_encoded_{name}_decoded.bin", read(directory / source.name))
