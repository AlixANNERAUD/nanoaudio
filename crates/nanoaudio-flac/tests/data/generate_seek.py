import io
import struct
from pathlib import Path

import numpy
import pyflac
import tempfile

directory = Path(__file__).parent
generator = numpy.random.default_rng(4242)


def encode(samples, sample_rate, blocksize):
    output = io.BytesIO()
    frames = []

    def write(buffer, size, samples_count, frame):
        if samples_count:
            frames.append(output.tell())
        output.write(bytes(buffer))

    encoder = pyflac.StreamEncoder(
        sample_rate=sample_rate,
        write_callback=write,
        seek_callback=output.seek,
        tell_callback=output.tell,
        blocksize=blocksize,
        compression_level=5,
        streamable_subset=False,
    )
    encoder.process(samples)
    encoder.finish()
    return output.getvalue(), frames


def metadata_end(data):
    position = 4
    while True:
        header = data[position]
        position += 4 + int.from_bytes(data[position + 1 : position + 4], "big")
        if header & 0x80:
            return position


def insert_block(data, kind, body):
    end = metadata_end(data)
    position = 4
    blocks = []
    while position < end:
        header = data[position]
        length = int.from_bytes(data[position + 1 : position + 4], "big")
        blocks.append((header & 0x7F, data[position + 4 : position + 4 + length]))
        position += 4 + length
    blocks.insert(1, (kind, body))
    output = bytearray(b"fLaC")
    for index, (block_kind, block_body) in enumerate(blocks):
        output.append((0x80 if index == len(blocks) - 1 else 0) | block_kind)
        output += len(block_body).to_bytes(3, "big")
        output += block_body
    return bytes(output) + data[end:]


def crc8(data):
    crc = 0
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = ((crc << 1) ^ 0x07) & 0xFF if crc & 0x80 else (crc << 1) & 0xFF
    return crc


def crc16(data):
    crc = 0
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x8005) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def utf8(value):
    if value < 0x80:
        return bytes([value])
    length = 2
    while value >= 1 << (5 * length + 1):
        length += 1
    output = [0x80 | ((value >> (6 * index)) & 0x3F) for index in range(length - 1)]
    first = ((0xFF00 >> length) & 0xFF) | (value >> (6 * (length - 1)))
    return bytes([first] + output[::-1])


def variable(data, frames, blocksize):
    start = metadata_end(data)
    bounds = frames + [len(data)]
    output = bytearray(data[:start])
    for index in range(len(frames)):
        frame = data[bounds[index] : bounds[index + 1]]
        length = 1 if frame[4] < 0x80 else bin(frame[4])[2:].index("0")
        tail = frame[4 + length :]
        extra = {6: 1, 7: 2}.get(frame[2] >> 4, 0) + {12: 1, 13: 2, 14: 2}.get(frame[2] & 0x0F, 0)
        header = bytes([0xFF, 0xF9, frame[2], frame[3]]) + utf8(index * blocksize) + tail[:extra]
        header += bytes([crc8(header)])
        body = header + tail[extra + 1 : -2]
        output += body + crc16(body).to_bytes(2, "big")
    return bytes(output)


def seek_table(start, frames, blocksize, every):
    points = b"".join(
        struct.pack(">QQH", index * blocksize, offset - start, blocksize)
        for index, offset in enumerate(frames)
        if index % every == 0
    )
    return points + struct.pack(">QQH", 0xFFFFFFFFFFFFFFFF, 0, 0) * 3


def check(data, samples):
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / "check.flac"
        path.write_bytes(data)
        decoded, _ = pyflac.FileDecoder(str(path)).process()
        decoded = numpy.round(decoded * 32768).astype(numpy.int16)
    assert numpy.array_equal(decoded.reshape(samples.shape), samples), "mismatch"


length = 24_000
time = numpy.arange(length) / 16_000
envelope = numpy.abs(numpy.sin(numpy.pi * time / 1.7)) * 0.9 + 0.05
mono = numpy.clip(
    envelope * 20_000 * numpy.sin(2 * numpy.pi * 330 * time)
    + generator.normal(0, 1, length) * envelope * 300,
    -32768,
    32767,
).astype(numpy.int16)

data, frames = encode(mono, 16_000, 576)
check(data, mono)
(directory / "flac_seek_mono.flac").write_bytes(data)

stream = variable(data, frames, 576)
check(stream, mono)
(directory / "flac_seek_variable.flac").write_bytes(stream)

stereo = numpy.stack(
    [mono, numpy.clip(mono.astype(numpy.int32) // 2 + generator.normal(0, 60, length), -32768, 32767)],
    axis=1,
).astype(numpy.int16)
data, frames = encode(stereo, 16_000, 576)
table = insert_block(data, 3, seek_table(metadata_end(data), frames, 576, 1))
check(table, stereo)
(directory / "flac_seek_table.flac").write_bytes(table)
