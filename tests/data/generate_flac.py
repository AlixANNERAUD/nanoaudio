import io
import struct
from pathlib import Path

import numpy
import pyflac
import soundfile

directory = Path(__file__).parent
generator = numpy.random.default_rng(9639)


def flac_bytes(samples, sample_rate, subtype, **options):
    buffer = io.BytesIO()
    soundfile.write(buffer, samples, sample_rate, format="FLAC", subtype=subtype, **options)
    return buffer.getvalue()


def pyflac_bytes(samples, sample_rate, blocksize):
    output = io.BytesIO()

    def write(buffer, size, samples_count, frame):
        output.write(bytes(buffer))

    encoder = pyflac.StreamEncoder(
        sample_rate=sample_rate,
        write_callback=write,
        seek_callback=output.seek,
        tell_callback=output.tell,
        blocksize=blocksize,
        compression_level=8,
        streamable_subset=False,
        verify=True,
    )
    encoder.process(samples)
    encoder.finish()
    return output.getvalue()


def read_soundfile(data):
    samples, _ = soundfile.read(io.BytesIO(data), dtype="int32", always_2d=True)
    return samples


def insert_blocks(data, blocks):
    assert data[:4] == b"fLaC"
    position = 4
    existing = []
    while True:
        header = data[position]
        length = int.from_bytes(data[position + 1 : position + 4], "big")
        existing.append((header & 0x7F, data[position + 4 : position + 4 + length]))
        position += 4 + length
        if header & 0x80:
            break
    ordered = existing[:1] + blocks + existing[1:]
    output = bytearray(b"fLaC")
    for index, (kind, body) in enumerate(ordered):
        last = 0x80 if index == len(ordered) - 1 else 0
        output.append(last | kind)
        output += len(body).to_bytes(3, "big")
        output += body
    return bytes(output + data[position:])


def picture(size):
    mime = b"image/png"
    description = b"cover"
    data = generator.integers(0, 256, size, dtype=numpy.uint8).tobytes()
    return (
        struct.pack(">II", 3, len(mime))
        + mime
        + struct.pack(">I", len(description))
        + description
        + struct.pack(">IIIII", 64, 64, 24, 0, len(data))
        + data
    )


def tone(frames, sample_rate, frequency, phase, amplitude, noise):
    time = numpy.arange(frames) / sample_rate
    signal = amplitude * numpy.sin(2 * numpy.pi * frequency * time + phase)
    return signal + generator.normal(0, noise, frames)


def save(name, data):
    (directory / name).write_bytes(data)


def save_expected(name, samples, dtype):
    (directory / name).write_bytes(samples.astype(numpy.dtype(dtype).newbyteorder("<")).tobytes())


def check(data, samples, shift):
    decoded = read_soundfile(data) >> shift
    assert numpy.array_equal(decoded, samples.reshape(decoded.shape)), "mismatch"
    return decoded


stereo = numpy.stack(
    [
        tone(4500, 44100, 440, 0.0, 12000, 300),
        tone(4500, 44100, 440, 0.1, 11000, 300),
    ],
    axis=1,
).astype(numpy.int16)

stereo_default = flac_bytes(stereo, 44100, "PCM_16")
expected = check(stereo_default, stereo, 16)
save("flac_stereo_16.flac", stereo_default)
save_expected("flac_stereo_16.bin", expected, numpy.int16)

for level, name in [(0.0, "flac_stereo_16_level_0.flac"), (1.0, "flac_stereo_16_level_8.flac")]:
    data = flac_bytes(stereo, 44100, "PCM_16", compression_level=level)
    check(data, stereo, 16)
    save(name, data)

blocks = pyflac_bytes(stereo, 44100, 1000)
check(blocks, stereo, 16)
save("flac_stereo_16_block_1000.flac", blocks)

buffer = io.BytesIO()
with soundfile.SoundFile(buffer, "w", 44100, 2, "PCM_16", format="FLAC") as file:
    file.title = "nanoaudio"
    file.artist = "fixture"
    file.write(stereo)
metadata = insert_blocks(buffer.getvalue(), [(6, picture(8000)), (2, b"test" + bytes(20)), (1, bytes(1500))])
check(metadata, stereo, 16)
save("flac_stereo_16_metadata.flac", metadata)

mono = numpy.clip(
    numpy.sin(2 * numpy.pi * numpy.cumsum(numpy.linspace(200, 3000, 6000)) / 16000) * 20000
    + generator.normal(0, 50, 6000),
    -32768,
    32767,
).astype(numpy.int16)
data = flac_bytes(mono, 16000, "PCM_16")
expected = check(data, mono, 16)
save("flac_mono_16.flac", data)
save_expected("flac_mono_16.bin", expected, numpy.int16)

noise = generator.integers(-32768, 32768, 3000).astype(numpy.int16)
data = flac_bytes(noise, 22050, "PCM_16")
expected = check(data, noise, 16)
save("flac_noise_16.flac", data)
save_expected("flac_noise_16.bin", expected, numpy.int16)

eight = numpy.clip(tone(4500, 8000, 300, 0.0, 100, 4), -128, 127).astype(numpy.int16)
data = flac_bytes(eight << 8, 8000, "PCM_S8")
expected = check(data, eight, 24)
save("flac_mono_8.flac", data)
save_expected("flac_mono_8.bin", expected, numpy.int16)

silence = numpy.zeros((4500, 2), dtype=numpy.int16)
data = flac_bytes(silence, 44100, "PCM_16")
check(data, silence, 16)
save("flac_silence_16.flac", data)

deep = numpy.stack(
    [
        tone(2500, 48000, 1000, 0.0, 6_000_000, 2000),
        tone(2500, 48000, 1000, 0.3, 5_000_000, 50000),
    ],
    axis=1,
).astype(numpy.int32)
data = flac_bytes(deep << 8, 48000, "PCM_24")
expected = check(data, deep, 8)
save("flac_stereo_24.flac", data)
save_expected("flac_stereo_24.bin", expected, numpy.int32)

wasted = (tone(2000, 48000, 500, 0.0, 1500, 20).astype(numpy.int32)) << 12
data = flac_bytes(wasted << 8, 48000, "PCM_24")
expected = check(data, wasted, 8)
save("flac_wasted_24.flac", data)
save_expected("flac_wasted_24.bin", expected, numpy.int32)

surround = numpy.stack(
    [tone(1500, 48000, 200 * (channel + 1), channel, 8000, 100) for channel in range(6)],
    axis=1,
).astype(numpy.int16)
data = flac_bytes(surround, 48000, "PCM_16")
expected = check(data, surround, 16)
save("flac_surround_16.flac", data)
save_expected("flac_surround_16.bin", expected, numpy.int16)

wide = numpy.stack(
    [
        tone(2500, 96000, 700, 0.0, 2_000_000_000, 1_000_000),
        tone(2500, 96000, 700, 0.0, -2_000_000_000, 1_000_000),
    ],
    axis=1,
).astype(numpy.int32)
data = pyflac_bytes(wide, 96000, 1024)
save("flac_stereo_32.flac", data)
save_expected("flac_stereo_32.bin", wide, numpy.int32)
