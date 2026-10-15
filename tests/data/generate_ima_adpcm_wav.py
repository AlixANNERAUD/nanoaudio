import math
import struct
from pathlib import Path

import numpy
import soundfile

directory = Path(__file__).parent


def chunks(file):
    offset = 12
    while offset < len(file):
        identifier = file[offset:offset + 4]
        size = struct.unpack("<I", file[offset + 4:offset + 8])[0]
        yield identifier, file[offset + 8:offset + 8 + size]
        offset += 8 + size + (size & 1)


def truncate(file, length):
    body = b""
    for identifier, data in chunks(file):
        if identifier == b"fact":
            continue
        if identifier == b"data":
            data = data[:length]
        body += identifier + struct.pack("<I", len(data)) + data + b"\0" * (len(data) & 1)
    return b"RIFF" + struct.pack("<I", len(body) + 4) + b"WAVE" + body


def signal(frames, channels):
    state = 99
    values = []
    for index in range(frames):
        frame = []
        for channel in range(channels):
            state = (state * 1103515245 + 12345) & 0x7FFFFFFF
            noise = (state >> 16) % 1201 - 600
            tone = 14000 * math.sin(2 * math.pi * (300 + 250 * channel) * index / 8000)
            frame.append(int(tone) + noise)
        values.append(frame)
    return numpy.array(values, dtype=numpy.int16)


def generate(name, channels, frames, groups, extra):
    path = directory / f"{name}.wav"
    soundfile.write(path, signal(frames, channels), 8000, subtype="IMA_ADPCM")

    decoded, _ = soundfile.read(path, dtype="int16", always_2d=True)
    samples_per_block = 505
    assert decoded.shape == ((frames + samples_per_block - 1) // samples_per_block * samples_per_block, channels)
    (directory / f"{name}_decoded.bin").write_bytes(decoded.astype("<i2").tobytes())

    block_align = 256 * channels
    length = 2 * block_align + 4 * channels + groups * 4 * channels + extra
    truncated_path = directory / f"{name}_truncated.wav"
    truncated_path.write_bytes(truncate(path.read_bytes(), length))

    present = 2 * samples_per_block + 1 + groups * 8
    truncated, _ = soundfile.read(truncated_path, dtype="int16", always_2d=True)
    assert (truncated[:present] == decoded[:present]).all()


generate("ima_adpcm_mono", 1, 1300, 13, 3)
generate("ima_adpcm_stereo", 2, 1300, 9, 4)
