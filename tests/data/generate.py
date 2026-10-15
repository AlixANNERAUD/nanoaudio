import audioop
import math
import struct
from pathlib import Path

directory = Path(__file__).parent


def write(name, data):
    (directory / name).write_bytes(data)


every_sample = struct.pack("<65536h", *range(-32768, 32768))
write("a_law_encoded.bin", audioop.lin2alaw(every_sample, 2))
write("mu_law_encoded.bin", audioop.lin2ulaw(every_sample, 2))
write("a_law_decoded.bin", audioop.alaw2lin(bytes(range(256)), 2))
write("mu_law_decoded.bin", audioop.ulaw2lin(bytes(range(256)), 2))

state = 12345
signal = []
for index in range(6000):
    state = (state * 1103515245 + 12345) & 0x7FFFFFFF
    noise = (state >> 16) % 2001 - 1000
    tone = 20000 * math.sin(2 * math.pi * 440 * index / 16000)
    chirp = 9000 * math.sin(2 * math.pi * (50 + index / 2) * index / 16000)
    burst = 32767 if 3000 <= index < 3050 else 0
    value = int(tone + chirp + noise) + burst
    signal.append(max(-32768, min(32767, value)))

adpcm_input = struct.pack("<%dh" % len(signal), *signal)
encoded, _ = audioop.lin2adpcm(adpcm_input, 2, None)
nibbles = bytearray()
for byte in encoded:
    nibbles.append(byte >> 4)
    nibbles.append(byte & 0x0F)
decoded, _ = audioop.adpcm2lin(encoded, 2, None)

write("adpcm_input.bin", adpcm_input)
write("adpcm_nibbles.bin", bytes(nibbles))
write("adpcm_decoded.bin", decoded)
