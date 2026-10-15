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
        size = struct.unpack(">I", file[offset + 4:offset + 8])[0]
        yield identifier, file[offset + 8:offset + 8 + size]
        offset += 8 + size + (size & 1)


def chunk(identifier, body):
    return identifier + struct.pack(">I", len(body)) + body + b"\0" * (len(body) & 1)


def form(kind, body):
    return b"FORM" + struct.pack(">I", len(body) + 4) + kind + body


def signal(frames, channels):
    state = 1234
    values = []
    for index in range(frames):
        frame = []
        for channel in range(channels):
            state = (state * 1103515245 + 12345) & 0x7FFFFFFF
            noise = ((state >> 8) % 20001 - 10000) / 1000000
            tone = 0.8 * math.sin(2 * math.pi * (440 + 220 * channel) * index / 8000)
            frame.append(max(-1.0, min(1.0, tone + noise)))
        values.append(frame)
    values[0] = [-1.0] * channels
    values[1] = [1.0 - 2 ** -31] * channels
    values[2] = [0.0] * channels
    return numpy.array(values, dtype=numpy.float64)


def write(name, frames, channels, sample_rate, subtype, endian="FILE"):
    path = directory / name
    soundfile.write(path, signal(frames, channels), sample_rate, format="AIFF", subtype=subtype, endian=endian)
    return path


def save(name, path, dtype, little_endian_dtype):
    decoded, _ = soundfile.read(path, dtype=dtype, always_2d=True)
    (directory / name).write_bytes(decoded.astype(little_endian_dtype).tobytes())
    return decoded


save("aiff_pcm_s8_decoded.bin", write("aiff_pcm_s8.aiff", 301, 1, 8000, "PCM_S8"), "int32", "<i4")
pcm_16 = save("aiff_pcm_16_decoded.bin", write("aiff_pcm_16.aiff", 300, 2, 44100, "PCM_16"), "int32", "<i4")
save("aiff_pcm_24_decoded.bin", write("aiff_pcm_24.aiff", 300, 2, 48000, "PCM_24"), "int32", "<i4")
save("aiff_pcm_32_decoded.bin", write("aiff_pcm_32.aiff", 300, 2, 96000, "PCM_32"), "int32", "<i4")
save("aiff_float_decoded.bin", write("aiff_float.aifc", 300, 2, 48000, "FLOAT"), "float32", "<f4")
save("aiff_alaw_decoded.bin", write("aiff_alaw.aifc", 301, 1, 8000, "ALAW"), "int16", "<i2")
save("aiff_ulaw_decoded.bin", write("aiff_ulaw.aifc", 300, 2, 8000, "ULAW"), "int16", "<i2")
sowt_16 = save("aiff_sowt_16_decoded.bin", write("aiff_sowt_16.aifc", 300, 2, 22050, "PCM_16", "LITTLE"), "int32", "<i4")
assert (sowt_16 == pcm_16).all()

little_endian_32 = write("aiff_sowt_32.aifc", 300, 1, 11025, "PCM_32", "LITTLE")
original, _ = soundfile.read(little_endian_32, dtype="int32", always_2d=True)
file = little_endian_32.read_bytes()
compression = file.index(b"23ni")
little_endian_32.write_bytes(file[:compression] + b"sowt" + file[compression + 4:])
assert (save("aiff_sowt_32_decoded.bin", little_endian_32, "int32", "<i4") == original).all()

parts = dict(chunks((directory / "aiff_pcm_16.aiff").read_bytes()))
common = parts[b"COMM"] + b"NONE" + bytes([14]) + b"not compressed" + b"\0"
sound = parts[b"SSND"]
sound = struct.pack(">II", 4, 0) + b"\xAA\xBB\xCC\xDD" + sound[8:]
body = chunk(b"FVER", struct.pack(">I", 0xA2805140))
body += chunk(b"ANNO", b"odd")
body += chunk(b"SSND", sound)
body += chunk(b"COMM", common)
(directory / "aiff_none.aifc").write_bytes(form(b"AIFC", body))
assert (save("aiff_none_decoded.bin", directory / "aiff_none.aifc", "int32", "<i4") == pcm_16).all()
