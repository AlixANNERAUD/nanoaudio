use nanoaudio_metadata::{
    Block, BlockContent, BlockType, Chunk, ChunkFormat, Container, Content, Cursor, Encoding,
    Error, Field, Frame, Id3v1, Id3v2Tag, Location, Riff, Text, VbrHeader, detect, genre,
};

const FILES: [&str; 11] = [
    "metadata_xing.mp3",
    "metadata_vbri.mp3",
    "metadata_id3v24.mp3",
    "metadata_id3v23.mp3",
    "metadata_id3v22.mp3",
    "metadata_id3v23_unsynchronised.mp3",
    "metadata_id3v24_unsynchronised.mp3",
    "metadata.flac",
    "metadata.wav",
    "metadata.aiff",
    "mono_440.mp3",
];

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn expected(name: &str, kinds: &[&str]) -> Vec<String> {
    let table = read("metadata_expected.tsv");

    String::from_utf8(table)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\t');
            let file = fields.next()?;
            let kind = fields.next()?;
            let key = fields.next()?;
            let value = fields.next()?;

            (file == name && kinds.contains(&kind)).then(|| format!("{kind}\t{key}\t{value}"))
        })
        .collect()
}

fn value(name: &str, kind: &str, key: &str) -> String {
    let prefix = format!("{kind}\t{key}\t");

    expected(name, &[kind])
        .into_iter()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
        .unwrap_or_else(|| panic!("{name}: no {kind} {key}"))
}

fn string(text: Text<'_>) -> String {
    text.chars().collect()
}

fn values(text: Text<'_>) -> Vec<String> {
    text.values().map(string).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn latin1(bytes: &[u8]) -> String {
    string(Text::new(Encoding::Latin1, bytes))
}

fn upgrade(identifier: &[u8]) -> String {
    let upgraded: &[u8] = match identifier {
        b"TT2" => b"TIT2",
        b"TP1" => b"TPE1",
        b"TAL" => b"TALB",
        b"TRK" => b"TRCK",
        b"COM" => b"COMM",
        b"TXX" => b"TXXX",
        b"PIC" => b"APIC",
        other => other,
    };

    latin1(upgraded)
}

fn describe_content(frame: &Frame<'_>, content: Content<'_>, file: &[u8], lines: &mut Vec<String>) {
    let identifier = upgrade(frame.identifier());

    match content {
        Content::Text(text) => {
            lines.push(format!("text\t{identifier}\t{}", values(text).join("\x1f")))
        }
        Content::UserText { description, value } => {
            let mut parts = vec![string(description)];
            parts.extend(values(value));
            lines.push(format!("user_text\t{identifier}\t{}", parts.join("\x1f")));
        }
        Content::Comment {
            language,
            description,
            text,
        } => {
            let mut parts = vec![latin1(&language), string(description)];
            parts.extend(values(text));
            lines.push(format!("comment\t{identifier}\t{}", parts.join("\x1f")));
        }
        Content::Picture(picture) => {
            let data = picture.data.slice(file, 0).unwrap();

            lines.push(format!(
                "picture\t{identifier}\t{}\x1f{}\x1f{}\x1f{}",
                latin1(picture.mime_type),
                picture.picture_type,
                string(picture.description),
                hex(data)
            ));
        }
        _ => {}
    }
}

fn describe_decoded(frame: &Frame<'_>, lines: &mut Vec<String>) {
    let mut buffer = vec![0; frame.location.length];
    let decoded = frame.decode(&mut buffer).unwrap();
    let content = frame.parse(decoded, 0).unwrap();
    let identifier = upgrade(frame.identifier());

    match content {
        Content::Picture(picture) => {
            let data = picture.data.slice(decoded, 0).unwrap();

            lines.push(format!(
                "picture\t{identifier}\t{}\x1f{}\x1f{}\x1f{}",
                latin1(picture.mime_type),
                picture.picture_type,
                string(picture.description),
                hex(data)
            ));
        }
        content => describe_content(frame, content, &[], lines),
    }
}

fn describe_tag(file: &[u8], tag: Id3v2Tag) -> Vec<String> {
    let mut lines = vec![
        format!("id3v2\tversion\t{}.{}", tag.major_version, tag.revision),
        format!("id3v2\tsize\t{}", tag.size()),
    ];

    for frame in tag.frames().items(file, 0) {
        let frame = frame.unwrap();

        match frame.content().unwrap() {
            Content::Unsynchronised => describe_decoded(&frame, &mut lines),
            Content::Compressed => {
                lines.push(format!("compressed\t{}", latin1(frame.identifier())))
            }
            content => describe_content(&frame, content, file, &mut lines),
        }
    }

    lines
}

fn id3_expected(name: &str) -> Vec<String> {
    expected(name, &["id3v2", "text", "user_text", "comment", "picture"])
}

#[test]
fn id3v2_matches_mutagen() {
    for name in [
        "metadata_id3v24.mp3",
        "metadata_id3v23.mp3",
        "metadata_id3v22.mp3",
        "metadata_id3v23_unsynchronised.mp3",
    ] {
        let file = read(name);
        let Container::Id3v2(tag) = detect(&file, 0).unwrap() else {
            panic!("{name}: not an ID3v2 tag");
        };

        assert_eq!(describe_tag(&file, tag), id3_expected(name), "{name}");
    }
}

#[test]
fn id3v24_unsynchronised_and_compressed_frames() {
    let name = "metadata_id3v24_unsynchronised.mp3";
    let file = read(name);
    let tag = Id3v2Tag::parse(&file, 0).unwrap();
    let mut wanted = id3_expected(name);
    let index = wanted
        .iter()
        .position(|line| line.starts_with("text\tTALB"))
        .unwrap();
    wanted[index] = "compressed\tTALB".to_owned();

    assert!(tag.flags.extended_header);
    assert_eq!(describe_tag(&file, tag), wanted);

    let frames: Vec<_> = tag.frames().items(&file, 0).map(Result::unwrap).collect();
    assert!(frames[0].flags.unsynchronised && frames[0].flags.data_length_indicator);
    assert_eq!(frames[0].content(), Ok(Content::Unsynchronised));
    assert!(frames[2].flags.compressed);
    assert_eq!(frames[2].content(), Ok(Content::Compressed));
}

#[test]
fn id3v2_fields_and_end() {
    let file = read("metadata_id3v24.mp3");
    let tag = Id3v2Tag::parse(&file, 0).unwrap();
    let fields: Vec<_> = tag
        .frames()
        .items(&file, 0)
        .map(|frame| frame.unwrap().field())
        .collect();

    assert_eq!(
        fields,
        [
            Some(Field::Title),
            Some(Field::Artist),
            Some(Field::TrackNumber),
            Some(Field::Album),
            Some(Field::DiscNumber),
            Some(Field::Date),
            Some(Field::Genre),
            None,
            Some(Field::AlbumArtist),
            Some(Field::Composer),
            Some(Field::Comment),
            None,
        ]
    );

    let size: usize = value("metadata_id3v24.mp3", "id3v2", "size")
        .parse()
        .unwrap();
    assert_eq!(tag.end(), size);
    assert!(matches!(
        detect(&file[size..], size),
        Ok(Container::Mpeg(_))
    ));
}

#[test]
fn id3v2_picture_is_located_not_copied() {
    let file = read("metadata_id3v24.mp3");
    let tag = Id3v2Tag::parse(&file, 0).unwrap();
    let frame = tag
        .frames()
        .items(&file, 0)
        .map(Result::unwrap)
        .find(|frame| frame.identifier() == b"APIC")
        .unwrap();
    let header = frame.location.offset + 64;
    let prefix = &file[..header];
    let partial = frame.within(prefix, 0);

    assert!(!partial.is_complete());

    let Ok(Content::Picture(picture)) = partial.content() else {
        panic!("picture header not parsed from a prefix");
    };

    assert_eq!(picture.mime_type, b"image/png");
    assert_eq!(picture.data.end(), frame.location.end());
    assert_eq!(
        hex(picture.data.slice(&file, 0).unwrap()),
        value("metadata_id3v24.mp3", "picture", "APIC")
            .rsplit('\x1f')
            .next()
            .unwrap()
    );
}

fn stream_tag(file: &[u8], tag: Id3v2Tag, capacity: usize) -> (Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut cursor = tag.frames();
    let mut start = tag.location.offset;
    let mut length = capacity;
    let mut reads = 0;
    let window = |start: usize, length: usize| &file[start..(start + length).min(file.len())];

    loop {
        let bytes = window(start, length);
        reads += 1;

        match cursor.advance(bytes, start) {
            Ok(Some(frame)) => {
                let content = match frame.content() {
                    Err(Error::Needs(location)) => {
                        reads += 1;
                        let frame =
                            frame.within(window(location.offset, location.length), location.offset);
                        frame.content().unwrap()
                    }
                    content => content.unwrap(),
                };

                match content {
                    Content::Unsynchronised => {
                        reads += 1;
                        let location = frame.location;
                        let frame =
                            frame.within(window(location.offset, location.length), location.offset);
                        describe_decoded(&frame, &mut lines);
                    }
                    content => describe_content(&frame, content, file, &mut lines),
                }
            }
            Ok(None) => break,
            Err(Error::Needs(location)) => {
                assert!(
                    location.offset < start
                        || location.end() > start + bytes.len()
                        || bytes.len() < length,
                    "needs bytes already provided"
                );
                start = location.offset;
                length = location.length.max(capacity);
            }
            Err(error) => panic!("{error}"),
        }
    }

    (lines, reads)
}

#[test]
fn id3v2_streams_through_a_small_window() {
    for name in [
        "metadata_id3v24.mp3",
        "metadata_id3v23.mp3",
        "metadata_id3v22.mp3",
        "metadata_id3v23_unsynchronised.mp3",
    ] {
        let file = read(name);
        let tag = Id3v2Tag::parse(&file[..10], 0).unwrap();

        for capacity in [10, 16, 33, 64, 200] {
            let (lines, reads) = stream_tag(&file, tag, capacity);
            let mut wanted = id3_expected(name);
            wanted.drain(..2);

            assert_eq!(lines, wanted, "{name} with a {capacity} byte window");
            assert!(reads > 1);
        }
    }
}

#[test]
fn id3v2_prefixes_report_what_is_missing() {
    let file = read("metadata_id3v24.mp3");

    for length in 0..10 {
        assert_eq!(
            Id3v2Tag::parse(&file[..length], 0),
            Err(Error::Needs(Location::new(0, 10)))
        );
        assert_eq!(
            detect(&file[..length], 0),
            Err(Error::Needs(Location::new(
                0,
                if length < 4 { 4 } else { 10 }
            )))
        );
    }

    let tag = Id3v2Tag::parse(&file, 0).unwrap();
    let complete = tag.frames().items(&file, 0).count();

    for length in 10..tag.end() {
        let prefix = &file[..length];
        let mut frames = tag.frames().items(prefix, 0);
        let mut count = 0;

        for frame in frames.by_ref() {
            match frame {
                Ok(frame) => {
                    count += 1;
                    assert!(frame.location.offset <= length);
                    assert_eq!(
                        frame.data.len(),
                        frame.location.length.min(length - frame.location.offset)
                    );

                    match frame.content() {
                        Ok(_) => {}
                        Err(Error::Needs(location)) => assert!(location.end() > length),
                        Err(error) => panic!("{error}"),
                    }
                }
                Err(Error::Needs(location)) => {
                    assert!(location.end() > length);
                    assert!(location.offset < tag.end());
                }
                Err(error) => panic!("{error}"),
            }
        }

        assert!(count <= complete);
    }
}

#[test]
fn id3v1_matches_mutagen() {
    for name in ["metadata_id3v24.mp3", "metadata_id3v23.mp3"] {
        let file = read(name);
        let location = Id3v1::location(file.len()).unwrap();
        let tag = Id3v1::parse(location.slice(&file, 0).unwrap()).unwrap();
        let get = |key| value(name, "id3v1", key);

        assert_eq!(string(tag.title), get("TIT2"));
        assert_eq!(string(tag.artist), get("TPE1"));
        assert_eq!(string(tag.album), get("TALB"));
        assert_eq!(string(tag.year), get("TDRC"));
        assert_eq!(string(tag.comment), get("COMM"));
        assert_eq!(tag.track.unwrap().to_string(), get("TRCK"));
        assert_eq!(tag.genre.unwrap().to_string(), get("TCON"));
        assert_eq!(tag.genre_name(), Some("Jazz"));
        assert_eq!(Id3v1::parse(&file), Some(tag));
    }

    assert_eq!(Id3v1::location(100), None);
    assert_eq!(Id3v1::parse(&[0; 128]), None);
    assert_eq!(Id3v1::parse(b"TAG"), None);
    assert_eq!(genre(0), Some("Blues"));
    assert_eq!(genre(191), Some("Psybient"));
    assert_eq!(genre(192), None);

    let mut version_one = [0u8; 128];
    version_one[..3].copy_from_slice(b"TAG");
    version_one[3..8].copy_from_slice(b"Title");
    version_one[97..127].fill(b'c');
    version_one[127] = 255;
    let tag = Id3v1::parse(&version_one).unwrap();
    assert_eq!(tag.track, None);
    assert_eq!(tag.genre, None);
    assert_eq!(string(tag.comment), "c".repeat(30));
}

#[test]
fn xing_and_lame_match_mutagen() {
    let name = "metadata_xing.mp3";
    let file = read(name);
    let get = |key| value(name, "mpeg", key);
    let Container::Mpeg(frame) = detect(&file, 0).unwrap() else {
        panic!("not an MPEG stream");
    };

    assert_eq!(frame.sample_rate.to_string(), get("sample_rate"));
    assert_eq!(frame.channels().to_string(), get("channels"));

    let Some(VbrHeader::Xing(xing)) = VbrHeader::parse(&file, 0).unwrap() else {
        panic!("no Xing header");
    };
    let lame = xing.lame.unwrap();

    assert!(!xing.info);
    assert_eq!(xing.frames.unwrap().to_string(), get("frames"));
    assert_eq!(xing.bytes.unwrap().to_string(), get("bytes"));
    assert_eq!(hex(xing.table.unwrap()), get("table"));
    assert_eq!(xing.quality.unwrap().to_string(), get("quality"));
    assert_eq!(&lame.encoder, b"LAME3.100");
    assert_eq!(lame.encoder_delay.to_string(), get("encoder_delay"));
    assert_eq!(lame.encoder_padding.to_string(), get("encoder_padding"));
    assert_eq!(lame.lowpass_frequency.to_string(), get("lowpass_frequency"));
    assert_eq!(lame.music_length.to_string(), get("music_length"));
    assert_eq!(xing.samples().unwrap().to_string(), get("samples"));

    let total = xing.frames.unwrap() as u64 * 1152;
    let mut previous = 0;

    for sample in (0..=total).step_by(500) {
        let position = xing.seek(sample).unwrap();
        assert!(position >= previous);
        assert!(position <= xing.bytes.unwrap() as usize);
        previous = position;
    }

    assert_eq!(
        xing.seek(0),
        Some(xing.table.unwrap()[0] as usize * file.len() / 256)
    );

    for length in 0..frame.frame_length() {
        assert!(matches!(
            VbrHeader::parse(&file[..length], 0),
            Err(Error::Needs(_))
        ));
    }
}

#[test]
fn vbri_matches_mutagen() {
    let name = "metadata_vbri.mp3";
    let file = read(name);
    let get = |key| value(name, "mpeg", key);
    let Some(VbrHeader::Vbri(vbri)) = VbrHeader::parse(&file, 0).unwrap() else {
        panic!("no VBRI header");
    };

    assert_eq!(vbri.frame.sample_rate.to_string(), get("sample_rate"));
    assert_eq!(vbri.frame.channels().to_string(), get("channels"));
    assert_eq!(vbri.frames.to_string(), get("frames"));
    assert_eq!(vbri.bytes.to_string(), get("bytes"));
    assert_eq!(vbri.quality.to_string(), get("quality"));
    assert_eq!(vbri.frames_per_entry.to_string(), get("frames_per_entry"));
    assert_eq!(vbri.samples().unwrap().to_string(), get("samples"));

    let table: Vec<_> = (0..vbri.entries())
        .map(|index| vbri.entry(index).unwrap().to_string())
        .collect();
    assert_eq!(table.join(","), get("table"));

    let frame_length = vbri.frame.frame_length();
    assert_eq!(vbri.seek(0), Some(frame_length));
    assert_eq!(
        vbri.seek(4 * 576),
        Some(frame_length + vbri.entry(0).unwrap() as usize)
    );
    assert_eq!(VbrHeader::parse(&read("mono_440.mp3"), 0), Ok(None));
}

fn flac_blocks(file: &[u8]) -> Vec<Block<'_>> {
    let Container::Flac(flac) = detect(file, 0).unwrap() else {
        panic!("not a FLAC stream");
    };

    flac.blocks().items(file, 0).map(Result::unwrap).collect()
}

#[test]
fn flac_matches_mutagen() {
    let name = "metadata.flac";
    let file = read(name);
    let blocks = flac_blocks(&file);
    let get = |key| value(name, "stream_info", key);

    let codes: Vec<_> = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let code = match block.block_type {
                BlockType::StreamInfo => 0,
                BlockType::Padding => 1,
                BlockType::Application => 2,
                BlockType::SeekTable => 3,
                BlockType::VorbisComment => 4,
                BlockType::CueSheet => 5,
                BlockType::Picture => 6,
                BlockType::Reserved(code) => code,
                BlockType::Invalid => 127,
            };

            format!("block\t{index}\t{code}")
        })
        .collect();
    assert_eq!(codes, expected(name, &["block"]));
    assert!(blocks.last().unwrap().last);

    let info = blocks[0].stream_info().unwrap();
    assert_eq!(info.sample_rate.to_string(), get("sample_rate"));
    assert_eq!(info.channels.to_string(), get("channels"));
    assert_eq!(info.bits_per_sample.to_string(), get("bits_per_sample"));
    assert_eq!(info.total_samples.to_string(), get("total_samples"));
    assert_eq!(hex(&info.md5), get("md5"));
    assert_eq!(
        info.minimum_block_size.to_string(),
        get("minimum_block_size")
    );
    assert_eq!(
        info.maximum_block_size.to_string(),
        get("maximum_block_size")
    );
    assert_eq!(
        info.minimum_frame_size.to_string(),
        get("minimum_frame_size")
    );
    assert_eq!(
        info.maximum_frame_size.to_string(),
        get("maximum_frame_size")
    );

    let Ok(BlockContent::VorbisComment(comments)) = blocks[1].content() else {
        panic!("no Vorbis comment block");
    };
    assert_eq!(
        string(comments.vendor(&file, 0).unwrap()),
        value(name, "vorbis", "vendor")
    );

    let mut lines = vec![format!(
        "vorbis\tvendor\t{}",
        value(name, "vorbis", "vendor")
    )];
    let mut fields = Vec::new();

    for (index, comment) in comments.items(&file, 0).enumerate() {
        let comment = comment.unwrap();
        let key = std::str::from_utf8(comment.key().unwrap()).unwrap();
        lines.push(format!(
            "vorbis\t{index}\t{key}={}",
            string(comment.value().unwrap())
        ));
        fields.push(comment.field());
    }

    assert_eq!(lines, expected(name, &["vorbis"]));
    assert_eq!(
        fields,
        [
            Some(Field::Title),
            Some(Field::Artist),
            Some(Field::Artist),
            Some(Field::Album),
            Some(Field::TrackNumber),
            Some(Field::Date),
            None,
        ]
    );

    let Ok(BlockContent::Picture(picture)) = blocks[2].content() else {
        panic!("no picture block");
    };
    let dimensions = picture.dimensions.unwrap();
    let line = [
        latin1(picture.mime_type),
        picture.picture_type.to_string(),
        string(picture.description),
        dimensions.width.to_string(),
        dimensions.height.to_string(),
        dimensions.depth.to_string(),
        dimensions.colors.to_string(),
        hex(picture.data.slice(&file, 0).unwrap()),
    ]
    .join("\x1f");
    assert_eq!(line, value(name, "picture", "PICTURE"));
    assert_eq!(blocks[3].content(), Ok(BlockContent::Padding));
}

#[test]
fn flac_prefixes_report_what_is_missing() {
    let file = read("metadata.flac");
    let blocks = flac_blocks(&file);
    let audio = blocks.last().unwrap().location.end();

    for length in 0..audio {
        let prefix = &file[..length];
        let flac = match detect(prefix, 0) {
            Ok(Container::Flac(flac)) => flac,
            Err(Error::Needs(location)) => {
                assert_eq!(location, Location::new(0, 4));
                continue;
            }
            other => panic!("{other:?}"),
        };

        for block in flac.blocks().items(prefix, 0) {
            match block {
                Ok(block) => {
                    let result = match block.content() {
                        Ok(BlockContent::VorbisComment(comments)) => comments
                            .items(prefix, 0)
                            .try_for_each(|comment| comment?.value().map(drop)),
                        content => content.map(drop),
                    };

                    if let Err(error) = result {
                        let Error::Needs(location) = error else {
                            panic!("{error}");
                        };
                        assert!(location.end() > length);
                    }
                }
                Err(Error::Needs(location)) => assert!(location.end() > length),
                Err(error) => panic!("{error}"),
            }
        }
    }
}

fn info_lines(file: &[u8], chunk: Chunk<'_>) -> Vec<String> {
    match chunk.info().unwrap() {
        Some(entries) => entries
            .items(file, 0)
            .map(|entry| {
                let entry = entry.unwrap();
                assert_eq!(entry.format, ChunkFormat::RiffInfo);

                format!(
                    "info\t{}\t{}",
                    latin1(&entry.identifier),
                    string(entry.text().unwrap())
                )
            })
            .collect(),
        None => Vec::new(),
    }
}

#[test]
fn wav_matches_soundfile_and_mutagen() {
    let name = "metadata.wav";
    let file = read(name);
    let Container::Wav(riff) = detect(&file, 0).unwrap() else {
        panic!("not a WAV file");
    };
    let mut info = Vec::new();
    let mut tag = None;
    let mut samples = None;

    for chunk in riff.chunks().items(&file, 0) {
        let chunk = chunk.unwrap();
        info.extend(info_lines(&file, chunk));

        if let Some(found) = chunk.id3v2().unwrap() {
            tag = Some(found);
        }

        if let Some(length) = chunk.sample_length().unwrap() {
            samples = Some(length);
        }
    }

    assert_eq!(info, expected(name, &["info"]));
    assert_eq!(samples.unwrap().to_string(), value(name, "audio", "frames"));
    assert_eq!(describe_tag(&file, tag.unwrap()), id3_expected(name));

    let fields: Vec<_> = riff
        .chunks()
        .items(&file, 0)
        .filter_map(|chunk| chunk.unwrap().info().unwrap())
        .flat_map(|entries| entries.items(&file, 0).map(|entry| entry.unwrap().field()))
        .collect();
    assert_eq!(
        fields,
        [
            Some(Field::Title),
            Some(Field::Artist),
            Some(Field::Album),
            Some(Field::Date),
            Some(Field::Genre),
            Some(Field::Comment),
            Some(Field::Copyright),
            Some(Field::TrackNumber),
            Some(Field::Encoder),
        ]
    );
}

#[test]
fn aiff_matches_soundfile_and_mutagen() {
    let name = "metadata.aiff";
    let file = read(name);
    let Container::Aiff(aiff) = detect(&file, 0).unwrap() else {
        panic!("not an AIFF file");
    };
    let mut text = Vec::new();
    let mut tag = None;

    assert!(!aiff.compressed);

    for chunk in aiff.chunks().items(&file, 0) {
        let chunk = chunk.unwrap();

        if chunk.field().is_some() {
            text.push(format!(
                "info\t{}\t{}",
                latin1(&chunk.identifier),
                string(chunk.text().unwrap())
            ));
        }

        if let Some(found) = chunk.id3v2().unwrap() {
            tag = Some(found);
        }
    }

    assert_eq!(text, expected(name, &["info"]));
    assert_eq!(describe_tag(&file, tag.unwrap()), id3_expected(name));
}

#[test]
fn chunks_stream_through_a_small_window() {
    let file = read("metadata.wav");
    let riff = Riff::parse(&file[..12], 0).unwrap();
    let mut cursor = riff.chunks();
    let mut start = 0;
    let mut identifiers = Vec::new();

    loop {
        let bytes = &file[start..(start + 8).min(file.len())];

        match cursor.advance(bytes, start) {
            Ok(Some(chunk)) => identifiers.push(latin1(&chunk.identifier)),
            Ok(None) => break,
            Err(Error::Needs(location)) => {
                assert_eq!(location.length, 8);
                start = location.offset;
            }
            Err(error) => panic!("{error}"),
        }
    }

    assert_eq!(identifiers, ["fmt ", "fact", "LIST", "data", "id3 "]);
}

#[test]
fn detection() {
    assert!(matches!(
        detect(&read("metadata.flac"), 0),
        Ok(Container::Flac(_))
    ));
    assert!(matches!(
        detect(&read("metadata.wav"), 0),
        Ok(Container::Wav(_))
    ));
    assert!(matches!(
        detect(&read("metadata.aiff"), 0),
        Ok(Container::Aiff(_))
    ));
    assert!(matches!(
        detect(&read("mono_440.mp3"), 0),
        Ok(Container::Mpeg(_))
    ));
    assert!(matches!(
        detect(&read("metadata_id3v23.mp3"), 0),
        Ok(Container::Id3v2(_))
    ));
    assert!(
        matches!(detect(&read("aiff_sowt_16.aifc"), 0), Ok(Container::Aiff(aiff)) if aiff.compressed)
    );
    assert_eq!(detect(b"OggS", 0), Err(Error::Unsupported));
    assert_eq!(detect(b"RIFF", 0), Err(Error::Needs(Location::new(0, 12))));
    assert_eq!(detect(b"RIFF\0\0\0\0AVI ", 0), Err(Error::Invalid));
    assert_eq!(detect(b"fL", 5), Err(Error::Needs(Location::new(5, 4))));
    assert_eq!(detect(&[0xFF], 0), Err(Error::Needs(Location::new(0, 4))));
    assert_eq!(detect(&[], 0), Err(Error::Needs(Location::new(0, 4))));
    assert_eq!(detect(b"ID", 0), Err(Error::Needs(Location::new(0, 4))));
}

#[test]
fn field_mapping() {
    assert_eq!(Field::from_id3v2(b"TIT2"), Some(Field::Title));
    assert_eq!(Field::from_id3v2(b"TT2"), Some(Field::Title));
    assert_eq!(Field::from_id3v2(b"TYER"), Some(Field::Date));
    assert_eq!(Field::from_id3v2(b"PRIV"), None);
    assert_eq!(Field::from_vorbis(b"albumartist"), Some(Field::AlbumArtist));
    assert_eq!(Field::from_vorbis(b"TrackNumber"), Some(Field::TrackNumber));
    assert_eq!(Field::from_vorbis(b"X-CUSTOM"), None);
    assert_eq!(Field::from_riff_info(b"ICRD"), Some(Field::Date));
    assert_eq!(Field::from_aiff(b"(c) "), Some(Field::Copyright));
    assert_eq!(Field::Date.id3v2_identifier(4), Some(&b"TDRC"[..]));
    assert_eq!(Field::Date.id3v2_identifier(3), Some(&b"TYER"[..]));
    assert_eq!(Field::Date.id3v2_identifier(2), Some(&b"TYE"[..]));
    assert_eq!(Field::AlbumArtist.vorbis_key(), b"ALBUMARTIST");
}

#[test]
fn unicode_text() {
    let clef = "𝄞 é";
    let mut little = vec![0xFF, 0xFE];
    little.extend(clef.encode_utf16().flat_map(u16::to_le_bytes));
    let mut big = vec![0xFE, 0xFF];
    big.extend(clef.encode_utf16().flat_map(u16::to_be_bytes));
    let plain: Vec<u8> = clef.encode_utf16().flat_map(u16::to_be_bytes).collect();

    assert_eq!(string(Text::new(Encoding::Utf16, &little)), clef);
    assert_eq!(string(Text::new(Encoding::Utf16, &big)), clef);
    assert_eq!(string(Text::new(Encoding::Utf16BigEndian, &plain)), clef);
    assert_eq!(string(Text::new(Encoding::Utf8, clef.as_bytes())), clef);
    assert_eq!(
        string(Text::new(Encoding::Latin1, &[0x43, 0x61, 0x66, 0xE9])),
        "Café"
    );

    let mut multiple = little.clone();
    multiple.extend([0, 0, 0xFE, 0xFF, 0, b'x']);
    assert_eq!(values(Text::new(Encoding::Utf16, &multiple)), [clef, "x"]);

    let mut buffer = [0u8; 16];
    let text = Text::new(Encoding::Utf16, &little);
    assert_eq!(text.decode(&mut buffer), Ok(clef));
    assert_eq!(text.decoded_length(), clef.len());
    assert_eq!(text.decode(&mut buffer[..4]), Err(Error::BufferTooSmall));
    assert!(text.eq_str(clef));

    let lone = [0x00, 0xD8, b'a', 0x00, 0x00, 0xDC, 0x41];
    assert_eq!(
        string(Text::new(Encoding::Utf16, &lone)),
        "\u{FFFD}a\u{FFFD}\u{FFFD}"
    );
    assert_eq!(
        string(Text::new(Encoding::Utf8, &[b'a', 0xC3, b'b', 0xF0, 0x9D])),
        "a\u{FFFD}b\u{FFFD}"
    );
}

fn frame24(identifier: &[u8; 4], flags: u16, body: &[u8]) -> Vec<u8> {
    let size = body.len() as u32;
    let mut frame = identifier.to_vec();
    frame.extend([
        (size >> 21) as u8 & 0x7F,
        (size >> 14) as u8 & 0x7F,
        (size >> 7) as u8 & 0x7F,
        size as u8 & 0x7F,
    ]);
    frame.extend(flags.to_be_bytes());
    frame.extend(body);
    frame
}

fn tag24(flags: u8, frames: &[u8], footer: bool) -> Vec<u8> {
    let size = frames.len() as u32;
    let size = [
        (size >> 21) as u8 & 0x7F,
        (size >> 14) as u8 & 0x7F,
        (size >> 7) as u8 & 0x7F,
        size as u8 & 0x7F,
    ];
    let mut tag = b"ID3\x04\x00".to_vec();
    tag.push(flags);
    tag.extend(size);
    tag.extend(frames);

    if footer {
        tag.extend(b"3DI\x04\x00");
        tag.push(flags);
        tag.extend(size);
    }

    tag
}

#[test]
fn id3v24_footer_encryption_and_grouping() {
    let mut frames = frame24(b"TIT2", 0x0004, b"\x80secret");
    frames.extend(frame24(b"TPE1", 0x0040, b"\x07\x03Grouped"));
    frames.extend(frame24(b"WOAR", 0, b"https://example.com\0"));
    frames.extend(frame24(b"PRIV", 0, b"owner\0data"));
    let tag = tag24(0x10, &frames, true);
    let mut file = vec![0xAA; 7];
    file.extend(&tag);

    let parsed = Id3v2Tag::parse(&tag, 7).unwrap();
    assert!(parsed.flags.footer);
    assert_eq!(parsed.location, Location::new(7, tag.len()));
    assert_eq!(
        Id3v2Tag::parse_footer(&file[file.len() - 10..], file.len() - 10),
        Ok(parsed)
    );

    let contents: Vec<_> = parsed
        .frames()
        .items(&file, 0)
        .map(|frame| frame.unwrap().content().unwrap())
        .collect();

    assert_eq!(contents[0], Content::Encrypted);
    assert_eq!(
        contents[1],
        Content::Text(Text::new(Encoding::Utf8, b"Grouped"))
    );
    assert_eq!(
        contents[2],
        Content::Url(Text::new(Encoding::Latin1, b"https://example.com"))
    );
    assert_eq!(contents[3], Content::Other(b"owner\0data"));
}

#[test]
fn id3v24_whole_tag_unsynchronisation_flag() {
    let frames = frame24(b"TIT2", 0, &[0x00, 0xFF, 0x00, 0xE0]);
    let tag = tag24(0x80, &frames, false);
    let parsed = Id3v2Tag::parse(&tag, 0).unwrap();
    let frame = parsed.frames().items(&tag, 0).next().unwrap().unwrap();
    let mut buffer = [0; 8];

    assert_eq!(frame.content(), Ok(Content::Unsynchronised));
    let decoded = frame.decode(&mut buffer).unwrap();
    assert_eq!(decoded, [0x00, 0xFF, 0xE0]);
    assert_eq!(
        frame.parse(decoded, 0),
        Ok(Content::Text(Text::new(Encoding::Latin1, &[0xFF, 0xE0])))
    );
    assert_eq!(frame.decode(&mut buffer[..2]), Err(Error::BufferTooSmall));
}

#[test]
fn rejects_unsupported_tags() {
    assert_eq!(
        Id3v2Tag::parse(b"ID3\x05\x00\x00\x00\x00\x00\x00", 0),
        Err(Error::Unsupported)
    );
    assert_eq!(
        Id3v2Tag::parse(b"ID3\x04\x00\x00\x00\x00\x80\x00", 0),
        Err(Error::Invalid)
    );

    let compressed = b"ID3\x02\x00\x40\x00\x00\x00\x00";
    let tag = Id3v2Tag::parse(compressed, 0).unwrap();
    assert_eq!(tag.frames().advance(compressed, 0), Err(Error::Unsupported));
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit.max(1) as u64) as usize
    }
}

fn walk_tag(bytes: &[u8], offset: usize, tag: Id3v2Tag) {
    let mut buffer = [0u8; 4096];

    for frame in tag.frames().items(bytes, offset).take(64) {
        let Ok(frame) = frame else {
            break;
        };
        let _ = frame.field();

        match frame.content() {
            Ok(Content::Unsynchronised) => {
                if let Ok(decoded) = frame.decode(&mut buffer) {
                    let decoded = decoded.to_vec();
                    visit(frame.parse(&decoded, 0));
                }
            }
            content => visit(content),
        }
    }
}

fn visit(content: Result<Content<'_>, Error>) {
    let mut output = [0u8; 64];
    let mut texts = Vec::new();

    match content {
        Ok(Content::Text(text) | Content::Url(text)) => texts.push(text),
        Ok(Content::UserText { description, value }) => texts.extend([description, value]),
        Ok(Content::UserUrl { description, url }) => texts.extend([description, url]),
        Ok(Content::Comment {
            description, text, ..
        }) => texts.extend([description, text]),
        Ok(Content::Picture(picture)) => texts.push(picture.description),
        _ => {}
    }

    for text in texts {
        let _ = text.chars().count();
        let _ = text.values().count();
        let _ = text.decode(&mut output);
    }
}

fn walk(bytes: &[u8], offset: usize) {
    let _ = Id3v1::parse(bytes);
    let _ = VbrHeader::parse(bytes, offset).map(|header| {
        header.map(|header| {
            let _ = header.samples();
            let _ = header.seek(1_000);
            let _ = header.seek(u64::MAX);
        })
    });

    match detect(bytes, offset) {
        Ok(Container::Id3v2(tag)) => walk_tag(bytes, offset, tag),
        Ok(Container::Flac(flac)) => {
            for block in flac.blocks().items(bytes, offset).take(64) {
                let Ok(block) = block else {
                    break;
                };
                let _ = block.seek_point(0);

                if let Ok(BlockContent::VorbisComment(comments)) = block.content() {
                    let _ = comments
                        .vendor(bytes, offset)
                        .map(|text| text.chars().count());

                    for comment in comments.items(bytes, offset).take(64) {
                        let Ok(comment) = comment else {
                            break;
                        };
                        let _ = comment.field();
                        let _ = comment.value_location();
                        let _ = comment.value().map(|text| text.chars().count());
                    }
                }
            }
        }
        Ok(Container::Wav(riff)) => walk_chunks(bytes, offset, riff.chunks()),
        Ok(Container::Aiff(aiff)) => walk_chunks(bytes, offset, aiff.chunks()),
        Ok(Container::Mpeg(frame)) => {
            let _ = frame.frame_length();
        }
        Err(_) => {}
    }
}

fn walk_chunks(bytes: &[u8], offset: usize, chunks: nanoaudio_metadata::Chunks) {
    for chunk in chunks.items(bytes, offset).take(64) {
        let Ok(chunk) = chunk else {
            break;
        };
        let _ = chunk.text().map(|text| text.chars().count());
        let _ = chunk.sample_length();

        if let Ok(Some(entries)) = chunk.info() {
            for entry in entries.items(bytes, offset).take(64) {
                let Ok(entry) = entry else {
                    break;
                };
                let _ = entry.text().map(|text| text.chars().count());
            }
        }

        if let Ok(Some(tag)) = chunk.id3v2() {
            walk_tag(bytes, offset, tag);
        }
    }
}

#[test]
fn corrupt_and_truncated_input_never_panics() {
    let mut random = Random(0x9E37_79B9_7F4A_7C15);

    for name in FILES {
        let file = read(name);
        walk(&file, 0);

        for _ in 0..600 {
            let mut corrupt = file.clone();
            let flips = 1 + random.below(8);

            for _ in 0..flips {
                let index = random.below(corrupt.len().min(1200));
                corrupt[index] = random.next() as u8;
            }

            if random.below(3) == 0 {
                let index = random.below(48);
                corrupt[index] = [0x00, 0x7F, 0x80, 0xFF][random.below(4)];
            }

            let length = match random.below(3) {
                0 => corrupt.len(),
                _ => random.below(corrupt.len()),
            };

            walk(&corrupt[..length], 0);
            walk(&corrupt[..length], random.below(64));
        }
    }
}
