import struct
import sys
from array import array
from pathlib import Path

from G722 import G722

directory = Path(__file__).parent
length = 32000
block = 1600


def clamp(value):
    return max(-32768, min(32767, value))


def wave(phase):
    position = ((phase >> 16) & 0xFFFF) - 32768
    return min(32767, position * (32768 - abs(position)) >> 13)


def signal():
    state = 12345
    tone = 0
    chirp = 0
    increment = 0
    samples = []
    for index in range(length):
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        noise = (state >> 15) - 32768
        tone = (tone + 117_964_800) & 0xFFFFFFFF
        increment += 2_000
        chirp = (chirp + increment) & 0xFFFFFFFF
        if index < 4000:
            value = wave(tone) >> 1
        elif index < 8000:
            value = 0
        elif index < 14000:
            value = wave(chirp) * 19661 >> 15
        elif index < 20000:
            value = noise * 9830 >> 15
        elif index < 24000:
            value = wave(tone) * 2
        elif index < 28000:
            value = (wave(tone) >> 7) + (noise >> 9)
        else:
            value = 24000 if (index // 40) % 2 == 0 else -24000
        samples.append(clamp(value))
    return samples


def random_bytes(count):
    state = 987654321
    values = bytearray()
    for _ in range(count):
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        values.append(state >> 23)
    return bytes(values)


def digests(samples):
    output = bytearray()
    for start in range(0, len(samples), block):
        chunk = samples[start : start + block]
        value = 0xCBF29CE484222325
        for byte in struct.pack("<%dh" % len(chunk), *chunk):
            value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
        output += struct.pack("<Q", value)
    return bytes(output)


def pack(codes, bits):
    return bytes((code >> 6) << (bits - 2) | (code & 0x3F) >> (8 - bits) for code in codes)


def decode(codes, bit_rate):
    return list(G722(16000, bit_rate).decode(pack(codes, bit_rate // 8000)))


samples = signal()
encoded = G722(16000, 64000).encode(array("h", samples))
noise = random_bytes(4000)

(directory / "g722_encoded.bin").write_bytes(encoded)
(directory / "g722_decoded.bin").write_bytes(
    b"".join(
        digests(decode(codes, bit_rate))
        for codes in [encoded, noise]
        for bit_rate in [64000, 56000, 48000]
    )
)

if len(sys.argv) > 2:
    Path(sys.argv[1]).write_bytes(struct.pack("<%dh" % length, *samples))
    Path(sys.argv[2]).write_bytes(struct.pack("<%dh" % length, *decode(encoded, 64000)))
