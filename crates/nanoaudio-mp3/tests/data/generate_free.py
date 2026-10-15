import struct
from pathlib import Path

directory = Path(__file__).parent


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


class Bits:
    def __init__(self, data):
        self.value = int.from_bytes(data, "big")
        self.length = len(data) * 8
        self.position = 0

    def read(self, count):
        self.position += count
        return (self.value >> (self.length - self.position)) & ((1 << count) - 1)


def side_layout(header):
    mpeg1 = (header[1] >> 3) & 3 == 3
    channels = 1 if header[3] >> 6 == 3 else 2
    if mpeg1:
        return 17 if channels == 1 else 32, 9, (3 if channels == 2 else 5) + 4 * channels, 2 * channels, 59
    return 9 if channels == 1 else 17, 8, channels, channels, 63


def parse(data):
    frames = []
    position = 0
    while position + 4 <= len(data):
        header = data[position : position + 4]
        assert header[0] == 0xFF and header[1] & 1 == 1
        length = frame_length(header)
        side_size, begin_bits, skipped, blocks, block_bits = side_layout(header)
        side = data[position + 4 : position + 4 + side_size]
        bits = Bits(side)
        begin = bits.read(begin_bits)
        bits.read(skipped)
        main = 0
        for _ in range(blocks):
            main += bits.read(12)
            bits.read(block_bits - 12)
        frames.append((bytes(header), bytes(side), begin, (main + 7) // 8, data[position + 4 + side_size : position + length]))
        position += length
    assert position == len(data)
    return frames


def free_header(header, padded):
    return bytes([header[0], header[1], (header[2] & 0x0C) | (0x02 if padded else 0), header[3]])


def with_begin(side, begin, begin_bits):
    bits = len(side) * 8
    value = int.from_bytes(side, "big")
    mask = ((1 << begin_bits) - 1) << (bits - begin_bits)
    return ((value & ~mask) | (begin << (bits - begin_bits))).to_bytes(len(side), "big")


def repack(name, source, unpadded, rate=None, tagged=False):
    data = (directory / source).read_bytes()
    frames = parse(data)
    side_size, begin_bits, _, _, _ = side_layout(frames[0][0])
    reservoir = 511 if begin_bits == 9 else 255
    stream = b""
    starts = []
    for header, side, begin, size, payload in frames:
        starts.append(len(stream))
        stream += payload
    tag = frames[0] if tagged else None
    audio = frames[1:] if tagged else frames
    audio_starts = starts[1:] if tagged else starts
    samples = 1152 if begin_bits == 9 else 576
    sample_rate = {0: [11025, 12000, 8000], 2: [22050, 24000, 16000], 3: [44100, 48000, 32000]}[(frames[0][0][1] >> 3) & 3][(frames[0][0][2] >> 2) & 3]
    sizes = []
    for index in range(len(frames)):
        if rate is None or (tagged and index == 0):
            sizes.append(unpadded)
        else:
            audio_index = index - (1 if tagged else 0)
            average = samples * rate * 125
            sizes.append((audio_index + 1) * average // sample_rate - audio_index * average // sample_rate)
            assert sizes[-1] in (unpadded, unpadded + 1)
    payloads = bytearray()
    used = 0
    heads = []
    if tag is not None:
        header, side, _, _, payload = tag
        body = bytearray(payload)[: unpadded - 4 - side_size]
        body += bytes(unpadded - 4 - side_size - len(body))
        heads.append(free_header(header, False) + side)
        payloads += body
        used = len(payloads)
    for index, (header, side, begin, size, payload) in enumerate(audio):
        length = sizes[index + (1 if tagged else 0)]
        payload_start = len(payloads)
        payloads += bytes(length - 4 - side_size)
        block = stream[audio_starts[index] - begin : audio_starts[index] - begin + size]
        assert len(block) == size
        block_start = max(used, payload_start - reservoir)
        assert block_start + size <= len(payloads), (name, index)
        payloads[block_start : block_start + size] = block
        heads.append(free_header(header, length > unpadded) + with_begin(side, payload_start - block_start, begin_bits))
        used = block_start + size
    output = bytearray()
    offsets = []
    position = 0
    for index, head in enumerate(heads):
        offsets.append(len(output))
        length = sizes[index] - 4 - side_size
        output += head + payloads[position : position + length]
        position += length
    if tagged:
        offsets = offsets[1:]
    if tag is not None:
        offset = 4 + side_size
        assert output[offset : offset + 4] == b"Info"
        count, = struct.unpack(">I", output[offset + 8 : offset + 12])
        assert count == len(audio)
        total = len(output)
        output[offset + 12 : offset + 16] = struct.pack(">I", total)
        output[offset + 16 : offset + 116] = bytes(min(255, offsets[min(count - 1, percent * count // 100)] * 256 // total) for percent in range(100))
        lame = offset + 120
        assert output[lame : lame + 4] == b"LAME"
        output[lame + 28 : lame + 32] = struct.pack(">I", total)
    (directory / name).write_bytes(bytes(output))


repack("free_cbr.mp3", "gapless_cbr.mp3", 1150, tagged=True)
repack("free_low.mp3", "seek_cbr_low.mp3", 146, rate=45)
