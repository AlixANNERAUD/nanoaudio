import math
import os
import struct
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

directory = Path(__file__).parent
converter = os.environ.get("QOACONV", "qoaconv")


def clamp(value):
    return max(-32768, min(32767, int(value)))


def signal(frames, sample_rate, channels, seed):
    state = seed
    samples = []
    for index in range(frames):
        for channel in range(channels):
            state = (state * 1103515245 + 12345) & 0x7FFFFFFF
            noise = (state >> 16) % 1201 - 600
            frequency = 440 * (channel + 1)
            tone = 14000 * math.sin(2 * math.pi * frequency * index / sample_rate)
            chirp = 7000 * math.sin(2 * math.pi * (60 + index / 3) * index / sample_rate)
            value = tone + chirp + noise
            if frames * 4 // 10 <= index < frames * 5 // 10:
                value = (state >> 20) % 7 - 3
            if frames * 5 // 10 <= index < frames * 5 // 10 + 200:
                value = 0
            if frames * 7 // 10 <= index < frames * 7 // 10 + 64:
                value = 32767 if (index + channel) % 2 else -32768
            if frames * 7 // 10 + 64 <= index < frames * 7 // 10 + 128:
                value = 32767 if channel == 0 else -32768
            samples.append(clamp(value))
    return samples


def write_wav(path, samples, sample_rate, channels):
    with wave.open(str(path), "wb") as file:
        file.setnchannels(channels)
        file.setsampwidth(2)
        file.setframerate(sample_rate)
        file.writeframes(struct.pack("<%dh" % len(samples), *samples))


def read_wav(path):
    with wave.open(str(path), "rb") as file:
        return file.readframes(file.getnframes())


def generate(name, frames, sample_rate, channels, seed):
    samples = signal(frames, sample_rate, channels, seed)
    pcm = struct.pack("<%dh" % len(samples), *samples)
    with tempfile.TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        write_wav(temporary / "input.wav", samples, sample_rate, channels)
        subprocess.run([converter, temporary / "input.wav", temporary / "encoded.qoa"], check=True, stdout=sys.stderr)
        subprocess.run([converter, temporary / "encoded.qoa", temporary / "decoded.wav"], check=True, stdout=sys.stderr)
        encoded = (temporary / "encoded.qoa").read_bytes()
        decoded = read_wav(temporary / "decoded.wav")
    (directory / f"qoa_{name}_input.bin").write_bytes(pcm)
    (directory / f"qoa_{name}.qoa").write_bytes(encoded)
    (directory / f"qoa_{name}_decoded.bin").write_bytes(decoded)


generate("mono", 10253, 16000, 1, 12345)
generate("stereo", 5153, 44100, 2, 54321)
