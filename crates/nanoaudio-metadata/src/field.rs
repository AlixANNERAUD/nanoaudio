#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Field {
    Title,
    Subtitle,
    Artist,
    Album,
    AlbumArtist,
    TrackNumber,
    DiscNumber,
    Date,
    Genre,
    Comment,
    Composer,
    Lyricist,
    Conductor,
    Publisher,
    Copyright,
    Encoder,
    EncodedBy,
    Grouping,
    Bpm,
    Isrc,
    Lyrics,
    Language,
}

const ID3V2: &[(&[u8], Field)] = &[
    (b"TIT2", Field::Title),
    (b"TIT3", Field::Subtitle),
    (b"TPE1", Field::Artist),
    (b"TALB", Field::Album),
    (b"TPE2", Field::AlbumArtist),
    (b"TRCK", Field::TrackNumber),
    (b"TPOS", Field::DiscNumber),
    (b"TDRC", Field::Date),
    (b"TYER", Field::Date),
    (b"TCON", Field::Genre),
    (b"COMM", Field::Comment),
    (b"TCOM", Field::Composer),
    (b"TEXT", Field::Lyricist),
    (b"TPE3", Field::Conductor),
    (b"TPUB", Field::Publisher),
    (b"TCOP", Field::Copyright),
    (b"TSSE", Field::Encoder),
    (b"TENC", Field::EncodedBy),
    (b"TIT1", Field::Grouping),
    (b"TBPM", Field::Bpm),
    (b"TSRC", Field::Isrc),
    (b"USLT", Field::Lyrics),
    (b"TLAN", Field::Language),
    (b"TT2", Field::Title),
    (b"TT3", Field::Subtitle),
    (b"TP1", Field::Artist),
    (b"TAL", Field::Album),
    (b"TP2", Field::AlbumArtist),
    (b"TRK", Field::TrackNumber),
    (b"TPA", Field::DiscNumber),
    (b"TYE", Field::Date),
    (b"TCO", Field::Genre),
    (b"COM", Field::Comment),
    (b"TCM", Field::Composer),
    (b"TXT", Field::Lyricist),
    (b"TP3", Field::Conductor),
    (b"TPB", Field::Publisher),
    (b"TCR", Field::Copyright),
    (b"TSS", Field::Encoder),
    (b"TEN", Field::EncodedBy),
    (b"TT1", Field::Grouping),
    (b"TBP", Field::Bpm),
    (b"TRC", Field::Isrc),
    (b"ULT", Field::Lyrics),
    (b"TLA", Field::Language),
];

const VORBIS: &[(&[u8], Field)] = &[
    (b"TITLE", Field::Title),
    (b"SUBTITLE", Field::Subtitle),
    (b"ARTIST", Field::Artist),
    (b"ALBUM", Field::Album),
    (b"ALBUMARTIST", Field::AlbumArtist),
    (b"ALBUM ARTIST", Field::AlbumArtist),
    (b"TRACKNUMBER", Field::TrackNumber),
    (b"DISCNUMBER", Field::DiscNumber),
    (b"DATE", Field::Date),
    (b"GENRE", Field::Genre),
    (b"COMMENT", Field::Comment),
    (b"DESCRIPTION", Field::Comment),
    (b"COMPOSER", Field::Composer),
    (b"LYRICIST", Field::Lyricist),
    (b"CONDUCTOR", Field::Conductor),
    (b"ORGANIZATION", Field::Publisher),
    (b"LABEL", Field::Publisher),
    (b"PUBLISHER", Field::Publisher),
    (b"COPYRIGHT", Field::Copyright),
    (b"ENCODER", Field::Encoder),
    (b"ENCODED-BY", Field::EncodedBy),
    (b"ENCODEDBY", Field::EncodedBy),
    (b"GROUPING", Field::Grouping),
    (b"BPM", Field::Bpm),
    (b"ISRC", Field::Isrc),
    (b"LYRICS", Field::Lyrics),
    (b"UNSYNCEDLYRICS", Field::Lyrics),
    (b"LANGUAGE", Field::Language),
];

const RIFF_INFO: &[(&[u8], Field)] = &[
    (b"INAM", Field::Title),
    (b"IART", Field::Artist),
    (b"IPRD", Field::Album),
    (b"ITRK", Field::TrackNumber),
    (b"IPRT", Field::TrackNumber),
    (b"ICRD", Field::Date),
    (b"IGNR", Field::Genre),
    (b"ICMT", Field::Comment),
    (b"IMUS", Field::Composer),
    (b"IWRI", Field::Lyricist),
    (b"IPUB", Field::Publisher),
    (b"ICOP", Field::Copyright),
    (b"ISFT", Field::Encoder),
    (b"ITCH", Field::EncodedBy),
    (b"ISRC", Field::Isrc),
    (b"ILNG", Field::Language),
];

const AIFF: &[(&[u8], Field)] = &[
    (b"NAME", Field::Title),
    (b"AUTH", Field::Artist),
    (b"ANNO", Field::Comment),
    (b"(c) ", Field::Copyright),
];

fn find(table: &[(&[u8], Field)], key: &[u8]) -> Option<Field> {
    table
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .map(|(_, field)| *field)
}

impl Field {
    pub fn from_id3v2(identifier: &[u8]) -> Option<Self> {
        find(ID3V2, identifier)
    }

    pub fn from_vorbis(key: &[u8]) -> Option<Self> {
        VORBIS
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, field)| *field)
    }

    pub fn from_riff_info(identifier: &[u8]) -> Option<Self> {
        find(RIFF_INFO, identifier)
    }

    pub fn from_aiff(identifier: &[u8]) -> Option<Self> {
        find(AIFF, identifier)
    }

    pub fn id3v2_identifier(self, major_version: u8) -> Option<&'static [u8]> {
        ID3V2
            .iter()
            .filter(|(identifier, _)| (identifier.len() == 3) == (major_version == 2))
            .filter(|(identifier, _)| {
                !matches!((major_version, *identifier), (4, b"TYER") | (3, b"TDRC"))
            })
            .find(|(_, field)| *field == self)
            .map(|(identifier, _)| *identifier)
    }

    pub fn vorbis_key(self) -> &'static [u8] {
        VORBIS
            .iter()
            .find(|(_, field)| *field == self)
            .map(|(key, _)| *key)
            .unwrap_or_default()
    }
}
