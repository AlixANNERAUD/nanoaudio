import io
import math
import struct
from pathlib import Path

import lameenc
from mutagen.mp3 import MP3

directory = Path(__file__).parent
delay = 576


def signal(sample_rate, channels, frames):
    state = 1789
    samples = []
    for index in range(frames):
        time = index / sample_rate
        for channel in range(channels):
            state = (state * 1103515245 + 12345) % 2**31
            noise = (state >> 16) % 4001 - 2000
            tone = 6000 * math.sin(2 * math.pi * (200 + 1800 * time + 300 * channel) * time)
            samples.append(int(tone + noise))
    return struct.pack("<%dh" % len(samples), *samples)


def encode(sample_rate, channels, frames, bit_rate=None, quality=None):
    encoder = lameenc.Encoder()
    encoder.set_in_sample_rate(sample_rate)
    encoder.set_channels(channels)
    encoder.set_quality(2)
    if quality is None:
        encoder.set_bit_rate(bit_rate)
    else:
        encoder.set_vbr(4)
        encoder.set_vbr_quality(quality)
    return encoder.encode(signal(sample_rate, channels, frames)) + encoder.flush()


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


def tagged(name, marker, audio, frames):
    mpeg1 = (audio[1] >> 3) & 3 == 3
    samples_per_frame = 1152 if mpeg1 else 576
    offsets = frame_offsets(audio)
    header = bytearray(audio[:4])
    header[1] |= 0x01
    header[2] = (9 << 4) | (header[2] & 0x0C)
    frame = bytearray(frame_length(header))
    frame[:4] = header
    total = len(frame) + len(audio)
    count = len(offsets)
    padding = count * samples_per_frame - delay - frames
    assert 529 <= padding < 4096
    table = bytes(min(255, (len(frame) + offsets[min(count - 1, percent * count // 100)]) * 256 // total) for percent in range(100))
    body = marker + struct.pack(">III", 0x0F, count, total) + table + struct.pack(">I", 57)
    lame = b"LAME3.100" + bytes([0x03, 160]) + bytes(8) + bytes([0, 128])
    lame += bytes([delay >> 4, ((delay & 0x0F) << 4) | (padding >> 8), padding & 0xFF])
    lame += bytes(4) + struct.pack(">I", total) + bytes(4)
    assert len(lame) == 36
    body += lame
    offset = 36 if mpeg1 and audio[3] >> 6 != 3 else (21 if mpeg1 or audio[3] >> 6 != 3 else 13)
    frame[offset : offset + len(body)] = body
    data = bytes(frame) + audio
    info = MP3(io.BytesIO(data)).info
    assert round(info.length * info.sample_rate) == frames
    (directory / name).write_bytes(data)


tagged("gapless_cbr.mp3", b"Info", encode(44100, 2, 30000, bit_rate=128), 30000)
tagged("gapless_vbr.mp3", b"Xing", encode(44100, 2, 40000, quality=4), 40000)
tagged("gapless_low.mp3", b"Info", encode(22050, 1, 20000, bit_rate=32), 20000)
