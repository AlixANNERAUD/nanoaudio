import io
import math
import struct
import zlib
from pathlib import Path

import lameenc
import numpy
import soundfile
from mutagen.aiff import AIFF
from mutagen.flac import FLAC, Picture
from mutagen.id3 import (
    APIC,
    COMM,
    ID3,
    TALB,
    TCOM,
    TCON,
    TDRC,
    TIT2,
    TPE1,
    TPE2,
    TPOS,
    TRCK,
    TXXX,
    Encoding,
    ParseID3v1,
)
from mutagen.mp3 import MP3
from mutagen.mp3._util import VBRIHeader, XingHeader
from mutagen.wave import WAVE

directory = Path(__file__).parent
expected = []

TITLE = "Café crème 𝄞"
ARTIST = "Ünïcødé Ärtist 🎵"
ALBUM = "Ålbum ÿÿ"


def record(name, kind, key, value):
    for part in (kind, str(key), str(value)):
        assert "\t" not in part and "\n" not in part
    expected.append(f"{name}\t{kind}\t{key}\t{value}")


def picture_bytes(length):
    state = 77
    data = bytearray(b"\x89PNG\r\n\x1a\n")
    while len(data) < length:
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        data.append((state >> 16) & 0xFF)
    data[20:24] = b"\xff\x00\xff\xe0"
    data[40:42] = b"\xff\xff"
    data[-1] = 0xFF
    return bytes(data)


PICTURE = picture_bytes(320)


def mp3(sample_rate, channels, bit_rate, seconds):
    frames = int(sample_rate * seconds)
    samples = []
    for index in range(frames):
        for channel in range(channels):
            samples.append(int(9000 * math.sin(2 * math.pi * (440 + 300 * channel) * index / sample_rate)))
    encoder = lameenc.Encoder()
    encoder.set_bit_rate(bit_rate)
    encoder.set_in_sample_rate(sample_rate)
    encoder.set_channels(channels)
    encoder.set_quality(2)
    return encoder.encode(struct.pack("<%dh" % len(samples), *samples)) + encoder.flush(), frames


def frame_offsets(data):
    offsets = []
    position = 0
    while position + 4 <= len(data):
        header = data[position:position + 4]
        assert header[0] == 0xFF
        version = (header[1] >> 3) & 3
        bitrate_index = header[2] >> 4
        rate_index = (header[2] >> 2) & 3
        padding = (header[2] >> 1) & 1
        if version == 3:
            bitrate = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320][bitrate_index]
            sample_rate = [44100, 48000, 32000][rate_index]
            length = 144000 * bitrate // sample_rate + padding
        else:
            bitrate = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][bitrate_index]
            sample_rate = [22050, 24000, 16000][rate_index] // (1 if version == 2 else 2)
            length = 72000 * bitrate // sample_rate + padding
        offsets.append(position)
        position += length
    assert position == len(data)
    return offsets


def first_frame(audio):
    header = bytearray(audio[:4])
    header[2] &= ~0x02
    version = (header[1] >> 3) & 3
    bitrate_index = header[2] >> 4
    rate_index = (header[2] >> 2) & 3
    if version == 3:
        bitrate = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320][bitrate_index]
        sample_rate = [44100, 48000, 32000][rate_index]
        length = 144000 * bitrate // sample_rate
    else:
        bitrate = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][bitrate_index]
        sample_rate = [22050, 24000, 16000][rate_index] // (1 if version == 2 else 2)
        length = 72000 * bitrate // sample_rate
    frame = bytearray(length)
    frame[:4] = header
    return frame, version, header[3] >> 6


def xing_mp3():
    audio, samples = mp3(44100, 2, 128, 0.3)
    offsets = frame_offsets(audio)
    frame, version, mode = first_frame(audio)
    offset = 36 if version == 3 and mode != 3 else 21
    total = len(frame) + len(audio)
    count = len(offsets)
    table = bytearray()
    for percent in range(100):
        index = min(count - 1, percent * count // 100)
        table.append(min(255, (len(frame) + offsets[index]) * 256 // total))
    delay = 576
    padding = count * 1152 - delay - samples
    assert 0 <= padding < 4096
    body = b"Xing" + struct.pack(">III", 0x0F, count, total) + bytes(table) + struct.pack(">I", 57)
    lame = bytearray(b"LAME3.100")
    lame += bytes([0x03, 160])
    lame += struct.pack(">I", 0)
    lame += struct.pack(">HH", 0, 0)
    lame += bytes([0, 128])
    lame += bytes([delay >> 4, ((delay & 0x0F) << 4) | (padding >> 8), padding & 0xFF])
    lame += bytes([0, 0])
    lame += struct.pack(">H", 0)
    lame += struct.pack(">I", total)
    lame += struct.pack(">HH", 0, 0)
    assert len(lame) == 36
    body += lame
    frame[offset:offset + len(body)] = body
    data = bytes(frame) + audio
    (directory / "metadata_xing.mp3").write_bytes(data)

    name = "metadata_xing.mp3"
    info = MP3(io.BytesIO(data)).info
    stream = io.BytesIO(data)
    stream.seek(offset)
    xing = XingHeader(stream)
    record(name, "mpeg", "sample_rate", info.sample_rate)
    record(name, "mpeg", "channels", info.channels)
    record(name, "mpeg", "frames", xing.frames)
    record(name, "mpeg", "bytes", xing.bytes)
    record(name, "mpeg", "table", bytes(xing.toc).hex())
    record(name, "mpeg", "quality", xing.vbr_scale)
    record(name, "mpeg", "encoder", xing.lame_version_desc)
    record(name, "mpeg", "encoder_delay", xing.lame_header.encoder_delay_start)
    record(name, "mpeg", "encoder_padding", xing.lame_header.encoder_padding_end)
    record(name, "mpeg", "lowpass_frequency", xing.lame_header.lowpass_filter)
    record(name, "mpeg", "music_length", xing.lame_header.music_length)
    record(name, "mpeg", "samples", round(info.length * info.sample_rate))
    assert round(info.length * info.sample_rate) == samples


def vbri_mp3():
    audio, samples = mp3(16000, 1, 32, 0.5)
    offsets = frame_offsets(audio)
    frame, version, mode = first_frame(audio)
    count = len(offsets)
    per_entry = 4
    entries = []
    for start in range(0, count, per_entry):
        end = min(count, start + per_entry)
        stop = offsets[end] if end < count else len(audio)
        entries.append(stop - offsets[start])
    body = b"VBRI" + struct.pack(">HHHII", 1, 0x1234, 75, len(frame) + len(audio), count)
    body += struct.pack(">HHHH", len(entries), 1, 2, per_entry)
    body += b"".join(struct.pack(">H", entry) for entry in entries)
    assert 36 + len(body) <= len(frame)
    frame[36:36 + len(body)] = body
    data = bytes(frame) + audio
    (directory / "metadata_vbri.mp3").write_bytes(data)

    name = "metadata_vbri.mp3"
    info = MP3(io.BytesIO(data)).info
    stream = io.BytesIO(data)
    stream.seek(36)
    vbri = VBRIHeader(stream)
    record(name, "mpeg", "sample_rate", info.sample_rate)
    record(name, "mpeg", "channels", info.channels)
    record(name, "mpeg", "frames", vbri.frames)
    record(name, "mpeg", "bytes", vbri.bytes)
    record(name, "mpeg", "quality", vbri.quality)
    record(name, "mpeg", "frames_per_entry", vbri.toc_frames)
    record(name, "mpeg", "table", ",".join(str(entry * vbri.toc_scale_factor) for entry in vbri.toc))
    record(name, "mpeg", "samples", round(info.length * info.sample_rate))


def describe_id3(name, tags):
    record(name, "id3v2", "version", "%d.%d" % tags.version[1:])
    record(name, "id3v2", "size", tags.size)
    for frame in tags.values():
        identifier = type(frame).__name__
        if identifier in ("APIC", "PIC"):
            mime = frame.mime
            record(
                name,
                "picture",
                identifier,
                "\x1f".join([mime, str(int(frame.type)), frame.desc, frame.data.hex()]),
            )
        elif identifier in ("COMM", "COM"):
            record(name, "comment", identifier, "\x1f".join([frame.lang, frame.desc] + list(frame.text)))
        elif identifier in ("TXXX", "TXX"):
            record(name, "user_text", identifier, "\x1f".join([frame.desc] + list(frame.text)))
        elif identifier.startswith("T"):
            record(name, "text", identifier, "\x1f".join(str(value) for value in frame.text))


def silent_mp3():
    audio, _ = mp3(16000, 1, 32, 0.15)
    return audio


def id3_files():
    audio = silent_mp3()

    for version in (4, 3):
        name = "metadata_id3v2%d.mp3" % version
        path = directory / name
        path.write_bytes(audio)
        tags = ID3()
        tags.add(TIT2(encoding=Encoding.UTF8, text=[TITLE]))
        tags.add(TPE1(encoding=Encoding.UTF16, text=[ARTIST]))
        tags.add(TALB(encoding=Encoding.UTF16BE, text=[ALBUM]))
        tags.add(TPE2(encoding=Encoding.LATIN1, text=["Album Ártist"]))
        tags.add(TRCK(encoding=Encoding.LATIN1, text=["3/12"]))
        tags.add(TPOS(encoding=Encoding.LATIN1, text=["1/2"]))
        tags.add(TDRC(encoding=Encoding.LATIN1, text=["2024"]))
        tags.add(TCON(encoding=Encoding.LATIN1, text=["Jazz"]))
        tags.add(TCOM(encoding=Encoding.UTF8, text=["Composer Øne", "Composer Twö"]))
        tags.add(COMM(encoding=Encoding.UTF16, lang="fra", desc="Déscription", text=["Commentaire ✓"]))
        tags.add(TXXX(encoding=Encoding.UTF8, desc="MOOD", text=["Calm"]))
        tags.add(APIC(encoding=Encoding.UTF16, mime="image/png", type=3, desc="Front ✓", data=PICTURE))
        if version == 3:
            tags.update_to_v23()
        tags.save(path, v2_version=version, v1=2, padding=lambda info: 64)
        describe_id3(name, ID3(path, translate=False))
        data = path.read_bytes()
        v1 = ParseID3v1(data[-128:])
        record(name, "id3v1", "TIT2", v1["TIT2"].text[0])
        record(name, "id3v1", "TPE1", v1["TPE1"].text[0])
        record(name, "id3v1", "TALB", v1["TALB"].text[0])
        record(name, "id3v1", "TDRC", v1["TDRC"].text[0])
        record(name, "id3v1", "TRCK", v1["TRCK"].text[0])
        record(name, "id3v1", "TCON", v1["TCON"].text[0])
        record(name, "id3v1", "COMM", v1["COMM"].text[0] if "COMM" in v1 else "")


def syncsafe(value):
    return bytes([(value >> 21) & 0x7F, (value >> 14) & 0x7F, (value >> 7) & 0x7F, value & 0x7F])


def unsynchronise(data):
    output = bytearray()
    for index, byte in enumerate(data):
        output.append(byte)
        if byte == 0xFF:
            following = data[index + 1] if index + 1 < len(data) else 0
            if following == 0 or following >= 0xE0 or index + 1 == len(data):
                output.append(0)
    return bytes(output)


def utf16(text):
    return b"\x01" + text.encode("utf-16")


def id3v22():
    frames = b""
    for identifier, body in [
        (b"TT2", b"\x00" + "Café".encode("latin1")),
        (b"TP1", utf16(ARTIST)),
        (b"TAL", b"\x00" + ALBUM.encode("latin1")),
        (b"TRK", b"\x007"),
        (b"COM", b"\x00eng" + b"\x00" + b"Old comment"),
        (b"TXX", b"\x00Key\x00Value"),
        (b"PIC", b"\x01PNG\x03" + "Cover".encode("utf-16") + b"\x00\x00" + PICTURE),
    ]:
        frames += identifier + struct.pack(">I", len(body))[1:] + body
    frames += b"\x00" * 16
    tag = b"ID3\x02\x00\x00" + syncsafe(len(frames)) + frames
    name = "metadata_id3v22.mp3"
    path = directory / name
    path.write_bytes(tag + silent_mp3())
    describe_id3(name, ID3(path, translate=False))


def frame23(identifier, body):
    return identifier + struct.pack(">IH", len(body), 0) + body


def id3v23_unsynchronised():
    frames = b""
    frames += frame23(b"TIT2", b"\x00" + "ÿÿ Title ÿ".encode("latin1"))
    frames += frame23(b"TPE1", utf16("ÿartist ÿ"))
    frames += frame23(b"TALB", b"\x00" + "Album".encode("latin1"))
    frames += frame23(b"APIC", b"\x00image/png\x00\x03Cover\x00" + PICTURE)
    frames += frame23(b"TCON", b"\x00Rock\xff")
    extended = struct.pack(">IHI", 6, 0, 32)
    body = unsynchronise(extended + frames) + b"\x00" * 32
    tag = b"ID3\x03\x00\xc0" + syncsafe(len(body)) + body
    name = "metadata_id3v23_unsynchronised.mp3"
    path = directory / name
    path.write_bytes(tag + silent_mp3())
    describe_id3(name, ID3(path, translate=False))


def frame24(identifier, body, flags=0, unsync=False, compress=False):
    if compress:
        payload = syncsafe(len(body)) + zlib.compress(body)
        flags |= 0x0009
    elif unsync:
        payload = syncsafe(len(body)) + unsynchronise(body)
        flags |= 0x0003
    else:
        payload = body
    return identifier + syncsafe(len(payload)) + struct.pack(">H", flags) + payload


def id3v24_unsynchronised():
    frames = b""
    frames += frame24(b"TIT2", b"\x01" + "ÿÿ Title".encode("utf-16"), unsync=True)
    frames += frame24(b"TPE1", b"\x03" + ARTIST.encode("utf-8"))
    frames += frame24(b"TALB", b"\x00" + "Compressed album".encode("latin1"), compress=True)
    frames += frame24(b"APIC", b"\x03image/png\x00\x08Back\x00" + PICTURE, unsync=True)
    frames += frame24(b"TCON", b"\x00Rock\x00Pop")
    extended = syncsafe(6) + b"\x01\x00"
    body = extended + frames + b"\x00" * 20
    tag = b"ID3\x04\x00\x40" + syncsafe(len(body)) + body
    name = "metadata_id3v24_unsynchronised.mp3"
    path = directory / name
    path.write_bytes(tag + silent_mp3())
    describe_id3(name, ID3(path, translate=False))


def flac():
    name = "metadata.flac"
    path = directory / name
    samples = (numpy.sin(numpy.arange(600) * 0.05) * 8000).astype(numpy.int16)
    soundfile.write(path, samples, 8000, subtype="PCM_16")
    audio = FLAC(path)
    audio.clear()
    audio["TITLE"] = TITLE
    audio["Artist"] = [ARTIST, "Second Ärtist"]
    audio["album"] = ALBUM
    audio["TRACKNUMBER"] = "5"
    audio["DATE"] = "1999-12-31"
    audio["X-CUSTOM"] = "custom value"
    picture = Picture()
    picture.type = 3
    picture.mime = "image/png"
    picture.desc = "Fröñt"
    picture.width = 16
    picture.height = 8
    picture.depth = 24
    picture.colors = 0
    picture.data = PICTURE
    audio.clear_pictures()
    audio.add_picture(picture)
    audio.save(padding=lambda info: 100)

    audio = FLAC(path)
    info = audio.info
    record(name, "stream_info", "sample_rate", info.sample_rate)
    record(name, "stream_info", "channels", info.channels)
    record(name, "stream_info", "bits_per_sample", info.bits_per_sample)
    record(name, "stream_info", "total_samples", info.total_samples)
    record(name, "stream_info", "md5", "%032x" % info.md5_signature)
    record(name, "stream_info", "minimum_block_size", info.min_blocksize)
    record(name, "stream_info", "maximum_block_size", info.max_blocksize)
    record(name, "stream_info", "minimum_frame_size", info.min_framesize)
    record(name, "stream_info", "maximum_frame_size", info.max_framesize)
    record(name, "vorbis", "vendor", audio.tags.vendor)
    for index, (key, value) in enumerate(audio.tags):
        record(name, "vorbis", index, key + "=" + value)
    for index, block in enumerate(audio.metadata_blocks):
        record(name, "block", index, block.code)
    picture = audio.pictures[0]
    record(
        name,
        "picture",
        "PICTURE",
        "\x1f".join(
            [
                picture.mime,
                str(picture.type),
                picture.desc,
                str(picture.width),
                str(picture.height),
                str(picture.depth),
                str(picture.colors),
                picture.data.hex(),
            ]
        ),
    )


def describe_soundfile(name, path, identifiers):
    with soundfile.SoundFile(path) as file:
        record(name, "audio", "frames", file.frames)
        for attribute, identifier in identifiers.items():
            value = getattr(file, attribute)
            if value:
                record(name, "info", identifier, value)


def tag_values():
    tags = ID3()
    tags.add(TIT2(encoding=Encoding.UTF8, text=[TITLE]))
    tags.add(TPE1(encoding=Encoding.UTF16, text=[ARTIST]))
    return tags


def wav():
    name = "metadata.wav"
    path = directory / name
    samples = (numpy.sin(numpy.arange(301) * 0.07) * 8000).astype(numpy.int16)
    with soundfile.SoundFile(path, "w", 8000, 1, "ULAW", format="WAV") as file:
        file.title = "Wave Title"
        file.artist = "Wave Artist"
        file.album = "Wave Album"
        file.date = "2021"
        file.genre = "Ambient"
        file.comment = "Wave comment"
        file.copyright = "Wave copyright"
        file.tracknumber = "9"
        file.software = "generator"
        file.write(samples)
    describe_soundfile(
        name,
        path,
        {
            "title": "INAM",
            "artist": "IART",
            "album": "IPRD",
            "date": "ICRD",
            "genre": "IGNR",
            "comment": "ICMT",
            "copyright": "ICOP",
            "tracknumber": "ITRK",
            "software": "ISFT",
        },
    )
    audio = WAVE(path)
    audio.add_tags()
    for frame in tag_values().values():
        audio.tags.add(frame)
    audio.save(v2_version=4)
    describe_id3(name, WAVE(path).tags)


def aiff():
    name = "metadata.aiff"
    path = directory / name
    samples = (numpy.sin(numpy.arange(201) * 0.03) * 8000).astype(numpy.int16)
    with soundfile.SoundFile(path, "w", 8000, 1, "PCM_16", format="AIFF") as file:
        file.title = "Aiff Title"
        file.artist = "Aiff Artist"
        file.comment = "Aiff comment"
        file.copyright = "Aiff copyright"
        file.write(samples)
    describe_soundfile(
        name,
        path,
        {"title": "NAME", "artist": "AUTH", "comment": "ANNO", "copyright": "(c) "},
    )
    audio = AIFF(path)
    audio.add_tags()
    for frame in tag_values().values():
        audio.tags.add(frame)
    audio.save(v2_version=3)
    describe_id3(name, AIFF(path).tags)


xing_mp3()
vbri_mp3()
id3_files()
id3v22()
id3v23_unsynchronised()
id3v24_unsynchronised()
flac()
wav()
aiff()

(directory / "metadata_expected.tsv").write_text("\n".join(expected) + "\n", encoding="utf-8")
