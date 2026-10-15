import math
from pathlib import Path

import numpy
import soundfile

directory = Path(__file__).parent

frames = 401
state = 2024
values = []
for index in range(frames):
    state = (state * 1103515245 + 12345) & 0x7FFFFFFF
    noise = (state >> 8) % 4001 - 2000
    tone = 7000000 * math.sin(2 * math.pi * 1000 * index / 48000)
    values.append([int(tone) + noise, -int(tone) - noise])

values[0] = [-(1 << 23), (1 << 23) - 1]
values[1] = [(1 << 23) - 1, -(1 << 23)]
values[2] = [0, -1]

samples = numpy.array(values, dtype=numpy.int32) << 8
soundfile.write(directory / "pcm_24.wav", samples, 48000, subtype="PCM_24")

decoded, _ = soundfile.read(directory / "pcm_24.wav", dtype="int32")
assert (decoded == samples).all()
(directory / "pcm_24_decoded.bin").write_bytes(decoded.astype("<i4").tobytes())

mono = numpy.ascontiguousarray(samples[:, 0])
soundfile.write(directory / "pcm_24_extensible.wav", mono, 48000, subtype="PCM_24", format="WAVEX")

decoded, _ = soundfile.read(directory / "pcm_24_extensible.wav", dtype="int32")
assert (decoded == mono).all()
