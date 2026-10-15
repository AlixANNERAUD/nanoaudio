import math
import random
import struct
from pathlib import Path

import lameenc

directory = Path(__file__).parent
generator = random.Random(1789)


def signal(sample_rate, channels, seconds):
    samples = []
    for index in range(int(sample_rate * seconds)):
        time = index / sample_rate
        loudness = 0.15 + 0.85 * abs(math.sin(math.pi * time / 1.3))
        for channel in range(channels):
            tone = math.sin(2 * math.pi * (330 + 220 * channel) * time) + 0.4 * math.sin(2 * math.pi * 2750 * time)
            noise = generator.uniform(-1, 1) if time % 1.0 > 0.5 else 0
            samples.append(int(max(-32768, min(32767, 9000 * loudness * (tone + 0.8 * noise)))))
    return struct.pack("<%dh" % len(samples), *samples)


def encode(sample_rate, channels, seconds, bit_rate=None, quality=None):
    encoder = lameenc.Encoder()
    encoder.set_in_sample_rate(sample_rate)
    encoder.set_channels(channels)
    encoder.set_quality(2)
    if quality is None:
        encoder.set_bit_rate(bit_rate)
    else:
        encoder.set_vbr(4)
        encoder.set_vbr_quality(quality)
    return encoder.encode(signal(sample_rate, channels, seconds)) + encoder.flush()


def frame_length(header):
    version = (header[1] >> 3) & 3
    bitrate_index = header[2] >> 4
    rate_index = (header[2] >> 2) & 3
    padding = (header[2] >> 1) & 1
    if version == 3:
        bitrate = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320][bitrate_index]
        return 144000 * bitrate // [44100, 48000, 32000][rate_index] + padding
    bitrate = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][bitrate_index]
    return 72000 * bitrate // ([22050, 24000, 16000][rate_index] // (1 if version == 2 else 2)) + padding


def frame_offsets(data):
    offsets = []
    position = 0
    while position + 4 <= len(data):
        assert data[position] == 0xFF
        offsets.append(position)
        position += frame_length(data[position : position + 4])
    assert position == len(data)
    return offsets


def tag_frame(audio, bitrate_index):
    header = bytearray(audio[:4])
    header[1] |= 0x01
    header[2] = (bitrate_index << 4) | (header[2] & 0x0C)
    frame = bytearray(frame_length(header))
    frame[:4] = header
    return frame


def xing(audio, marker):
    offsets = frame_offsets(audio)
    frame = tag_frame(audio, 9)
    total = len(frame) + len(audio)
    count = len(offsets)
    table = bytes(min(255, (len(frame) + offsets[min(count - 1, percent * count // 100)]) * 256 // total) for percent in range(100))
    body = marker + struct.pack(">III", 0x07, count, total) + table
    offset = 36 if (audio[1] >> 3) & 3 == 3 and audio[3] >> 6 != 3 else 21
    frame[offset : offset + len(body)] = body
    return bytes(frame) + audio


def vbri(audio):
    offsets = frame_offsets(audio)
    frame = tag_frame(audio, 9)
    count = len(offsets)
    per_entry = 3
    entries = []
    for start in range(0, count, per_entry):
        end = min(count, start + per_entry)
        entries.append((offsets[end] if end < count else len(audio)) - offsets[start])
    body = b"VBRI" + struct.pack(">HHHII", 1, 0, 75, len(frame) + len(audio), count)
    body += struct.pack(">HHHH", len(entries), 1, 2, per_entry)
    body += b"".join(struct.pack(">H", entry) for entry in entries)
    assert 36 + len(body) <= len(frame)
    frame[36 : 36 + len(body)] = body
    return bytes(frame) + audio


def id3(size):
    body = bytes([0xFF, 0xFB, 0x90, 0x00] * (size // 4))
    length = bytes([(len(body) >> shift) & 0x7F for shift in (21, 14, 7, 0)])
    return b"ID3\x03\x00\x00" + length + body


constant = encode(44100, 2, 2.0, bit_rate=128)
(directory / "seek_cbr_info.mp3").write_bytes(id3(1200) + xing(constant, b"Info"))

variable = encode(44100, 2, 3.0, quality=4)
(directory / "seek_vbr_xing.mp3").write_bytes(xing(variable, b"Xing"))
(directory / "seek_vbr_vbri.mp3").write_bytes(vbri(variable))

low = encode(22050, 1, 2.0, bit_rate=32)
(directory / "seek_cbr_low.mp3").write_bytes(low)
