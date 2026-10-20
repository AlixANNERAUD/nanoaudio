import struct
from pathlib import Path

import numpy
import soundfile

directory = Path(__file__).parent
rate = 48000


def tones(frequencies, seconds):
    time = numpy.arange(int(rate * seconds)) / rate
    return numpy.stack([0.35 * numpy.sin(2 * numpy.pi * frequency * time) for frequency in frequencies], axis=1)


def chirp(seconds):
    time = numpy.arange(int(rate * seconds)) / rate
    return 0.3 * numpy.sin(2 * numpy.pi * (100 * time + 3000 * time**2))


def crc(data):
    value = 0
    for byte in data:
        value ^= byte << 24
        for _ in range(8):
            value = ((value << 1) ^ 0x04C11DB7) if value & 0x80000000 else value << 1
            value &= 0xFFFFFFFF
    return value


def set_gain(path, gain):
    data = bytearray(path.read_bytes())
    count = data[26]
    body = 27 + count
    struct.pack_into("<h", data, body + 16, gain)
    data[22:26] = bytes(4)
    end = body + sum(data[27:body])
    struct.pack_into("<I", data, 22, crc(data[:end]))
    path.write_bytes(data)


def encode(name, data, level, gain=0, reference=True):
    path = directory / (name + ".opus")
    soundfile.write(path, data, rate, format="OGG", subtype="OPUS", compression_level=level)
    if gain:
        set_gain(path, gain)
    if not reference:
        return
    decoded, _ = soundfile.read(path, dtype="int16")
    (directory / (name + "_opus_decoded.bin")).write_bytes(decoded.astype("<i2").tobytes())


noise = numpy.random.default_rng(1).normal(0, 0.05, int(rate * 0.3))
encode("mono_440", tones([440], 0.2), 0.5)
encode("stereo_440_1000", tones([440, 1000], 0.2), 0.5)
encode("mono_hybrid", (chirp(0.3) + noise)[:, None], 0.9)
encode("stereo_low_bitrate", numpy.stack([chirp(0.3), noise * 4], axis=1), 1.0)
encode("mono_gain", tones([440], 0.2), 0.5, -1536)
music = tones([220, 330], 4.0) + tones([523, 659], 4.0) * 0.5 + numpy.random.default_rng(2).normal(0, 0.02, (rate * 4, 2))
encode("stereo_long", music * 0.6, 0.7, reference=False)
