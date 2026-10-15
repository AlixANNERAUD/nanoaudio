import math
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

directory = Path(__file__).parent
encoder, decoder = sys.argv[1], sys.argv[2]

configurations = [
    ("sbc_44100_joint_stereo", 44100, 2, ["-s", "8", "-B", "16", "-b", "53", "-j"], 128, 32),
    ("sbc_48000_stereo", 48000, 2, ["-s", "8", "-B", "16", "-b", "51"], 128, 32),
    ("sbc_16000_mono", 16000, 1, ["-s", "4", "-B", "12", "-b", "20"], 48, 120),
    ("sbc_32000_snr", 32000, 2, ["-s", "4", "-B", "8", "-b", "35", "-j", "-S"], 32, 128),
    ("sbc_44100_dual_channel", 44100, 2, ["-s", "8", "-B", "4", "-b", "32", "-d"], 32, 128),
    ("msbc", 16000, 1, ["-m"], 120, 48),
]


def channel(length, sample_rate, frequency, seed):
    state = seed
    samples = []
    for index in range(length):
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        noise = (state >> 15) - 32768
        time = index / sample_rate
        position = index / length
        tone = math.sin(2 * math.pi * frequency * time)
        if position < 0.25:
            value = 16000 * tone
        elif position < 0.35:
            value = 0
        elif position < 0.6:
            value = 12000 * math.sin(2 * math.pi * (100 + sample_rate / 5 * (position - 0.35)) * time)
        elif position < 0.75:
            value = noise * 0.3
        elif position < 0.85:
            value = 60000 * tone
        else:
            value = 300 * tone + noise / 256
        samples.append(max(-32768, min(32767, int(value))))
    return samples


def interleave(length, sample_rate, channels):
    left = channel(length, sample_rate, 440, 12345)
    if channels == 1:
        return left
    right = channel(length, sample_rate, 1250, 54321)
    return [sample for pair in zip(left, right) for sample in pair]


def write_au(path, samples, sample_rate, channels):
    header = struct.pack(">4sIIIII", b".snd", 24, len(samples) * 2, 3, sample_rate, channels)
    path.write_bytes(header + struct.pack(">%dh" % len(samples), *samples))


def read_au(path):
    data = path.read_bytes()
    offset = struct.unpack(">I", data[4:8])[0]
    body = data[offset:]
    return struct.unpack(">%dh" % (len(body) // 2), body)


with tempfile.TemporaryDirectory() as temporary:
    temporary = Path(temporary)
    for name, sample_rate, channels, options, frame_samples, frames in configurations:
        samples = interleave(frame_samples * frames, sample_rate, channels)
        source = temporary / (name + ".au")
        stream = directory / (name + ".sbc")
        decoded = temporary / (name + "_decoded.au")
        write_au(source, samples, sample_rate, channels)
        stream.write_bytes(subprocess.run([encoder, *options, str(source)], check=True, capture_output=True).stdout)
        subprocess.run([decoder, *(["-m"] if name == "msbc" else []), "-f", str(decoded), str(stream)], check=True)
        output = read_au(decoded)
        (directory / (name + ".pcm")).write_bytes(struct.pack("<%dh" % len(output), *output))
