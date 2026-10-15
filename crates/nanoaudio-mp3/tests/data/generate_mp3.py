import math
import struct
from pathlib import Path

import lameenc

directory = Path(__file__).parent


def encode(name, sample_rate, channels, bit_rate, frequencies, seconds):
    frames = int(sample_rate * seconds)
    samples = []
    for index in range(frames):
        for channel in range(channels):
            value = 12000 * math.sin(2 * math.pi * frequencies[channel] * index / sample_rate)
            samples.append(int(value))

    encoder = lameenc.Encoder()
    encoder.set_bit_rate(bit_rate)
    encoder.set_in_sample_rate(sample_rate)
    encoder.set_channels(channels)
    encoder.set_quality(2)
    data = encoder.encode(struct.pack("<%dh" % len(samples), *samples)) + encoder.flush()
    (directory / name).write_bytes(data)


encode("mono_440.mp3", 16000, 1, 32, [440], 0.5)
encode("stereo_440_1000.mp3", 44100, 2, 128, [440, 1000], 0.5)
