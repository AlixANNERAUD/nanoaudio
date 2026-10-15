use super::{Error, Location, Result, Window, read_u16_be, read_u24_be, read_u32_be};

const FRAME_HEADER_LENGTH: usize = 4;
const XING_TABLE_LENGTH: usize = 100;
const LAME_LENGTH: usize = 36;
const VBRI_OFFSET: usize = 36;
const VBRI_HEADER_LENGTH: usize = 26;

const BITRATES: [[u16; 15]; 5] = [
    [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
    ],
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
    ],
    [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ],
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
    ],
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
];

const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 32_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MpegVersion {
    Mpeg1,
    Mpeg2,
    Mpeg25,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    Layer1,
    Layer2,
    Layer3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelMode {
    Stereo,
    JointStereo,
    DualChannel,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameHeader {
    pub version: MpegVersion,
    pub layer: Layer,
    pub protected: bool,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub padding: bool,
    pub channel_mode: ChannelMode,
}

impl FrameHeader {
    pub const LENGTH: usize = FRAME_HEADER_LENGTH;

    pub fn parse(bytes: &[u8], offset: usize) -> Result<Self> {
        let header = Window::new(bytes, offset).array::<FRAME_HEADER_LENGTH>(offset)?;

        if header[0] != 0xFF || header[1] & 0xE0 != 0xE0 {
            return Err(Error::Invalid);
        }

        let version = match (header[1] >> 3) & 0x03 {
            0 => MpegVersion::Mpeg25,
            2 => MpegVersion::Mpeg2,
            3 => MpegVersion::Mpeg1,
            _ => return Err(Error::Invalid),
        };
        let layer = match (header[1] >> 1) & 0x03 {
            1 => Layer::Layer3,
            2 => Layer::Layer2,
            3 => Layer::Layer1,
            _ => return Err(Error::Invalid),
        };
        let bitrate_index = (header[2] >> 4) as usize;
        let sample_rate_index = ((header[2] >> 2) & 0x03) as usize;

        if bitrate_index == 0 {
            return Err(Error::Unsupported);
        }

        let table = match (version, layer) {
            (MpegVersion::Mpeg1, Layer::Layer1) => 0,
            (MpegVersion::Mpeg1, Layer::Layer2) => 1,
            (MpegVersion::Mpeg1, Layer::Layer3) => 2,
            (_, Layer::Layer1) => 3,
            _ => 4,
        };
        let bitrate = *BITRATES[table].get(bitrate_index).ok_or(Error::Invalid)? as u32 * 1000;
        let sample_rate = *SAMPLE_RATES.get(sample_rate_index).ok_or(Error::Invalid)?
            >> match version {
                MpegVersion::Mpeg1 => 0,
                MpegVersion::Mpeg2 => 1,
                MpegVersion::Mpeg25 => 2,
            };

        Ok(Self {
            version,
            layer,
            protected: header[1] & 0x01 == 0,
            bitrate,
            sample_rate,
            padding: header[2] & 0x02 != 0,
            channel_mode: match header[3] >> 6 {
                0 => ChannelMode::Stereo,
                1 => ChannelMode::JointStereo,
                2 => ChannelMode::DualChannel,
                _ => ChannelMode::Mono,
            },
        })
    }

    pub const fn channels(&self) -> u8 {
        match self.channel_mode {
            ChannelMode::Mono => 1,
            _ => 2,
        }
    }

    pub const fn samples_per_frame(&self) -> u32 {
        match (self.layer, self.version) {
            (Layer::Layer1, _) => 384,
            (Layer::Layer2, _) | (Layer::Layer3, MpegVersion::Mpeg1) => 1152,
            (Layer::Layer3, _) => 576,
        }
    }

    pub const fn frame_length(&self) -> usize {
        let padding = self.padding as usize;
        let bitrate = self.bitrate as usize;
        let sample_rate = self.sample_rate as usize;

        if sample_rate == 0 {
            return 0;
        }

        match self.layer {
            Layer::Layer1 => (12 * bitrate / sample_rate + padding) * 4,
            _ => self.samples_per_frame() as usize / 8 * bitrate / sample_rate + padding,
        }
    }

    pub const fn side_information_length(&self) -> usize {
        match (self.layer, self.version, self.channel_mode) {
            (Layer::Layer3, MpegVersion::Mpeg1, ChannelMode::Mono) => 17,
            (Layer::Layer3, MpegVersion::Mpeg1, _) => 32,
            (Layer::Layer3, _, ChannelMode::Mono) => 9,
            (Layer::Layer3, _, _) => 17,
            _ => 0,
        }
    }

    pub const fn xing_offset(&self) -> usize {
        FRAME_HEADER_LENGTH + 2 * self.protected as usize + self.side_information_length()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LameHeader {
    pub encoder: [u8; 9],
    pub revision: u8,
    pub vbr_method: u8,
    pub lowpass_frequency: u32,
    pub peak: u32,
    pub track_gain: u16,
    pub album_gain: u16,
    pub encoding_flags: u8,
    pub bitrate: u8,
    pub encoder_delay: u16,
    pub encoder_padding: u16,
    pub miscellaneous: u8,
    pub mp3_gain: u8,
    pub preset: u16,
    pub music_length: u32,
    pub music_crc: u16,
    pub crc: u16,
}

impl LameHeader {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let bytes = bytes.get(..LAME_LENGTH)?;
        let mut encoder = [0; 9];
        encoder.copy_from_slice(&bytes[..9]);

        if !lame_version_has_extension(&encoder) || bytes[9] >> 4 != 0 {
            return None;
        }

        let delay = read_u24_be(&bytes[21..24]);

        Some(Self {
            encoder,
            revision: bytes[9] >> 4,
            vbr_method: bytes[9] & 0x0F,
            lowpass_frequency: bytes[10] as u32 * 100,
            peak: read_u32_be(&bytes[11..15]),
            track_gain: read_u16_be(&bytes[15..17]),
            album_gain: read_u16_be(&bytes[17..19]),
            encoding_flags: bytes[19],
            bitrate: bytes[20],
            encoder_delay: (delay >> 12) as u16,
            encoder_padding: (delay & 0x0FFF) as u16,
            miscellaneous: bytes[24],
            mp3_gain: bytes[25],
            preset: read_u16_be(&bytes[26..28]),
            music_length: read_u32_be(&bytes[28..32]),
            music_crc: read_u16_be(&bytes[32..34]),
            crc: read_u16_be(&bytes[34..36]),
        })
    }
}

fn lame_version_has_extension(encoder: &[u8; 9]) -> bool {
    if encoder.starts_with(b"Lavc") || encoder.starts_with(b"Lavf") {
        return true;
    }

    if !encoder.starts_with(b"LAME") && !encoder.starts_with(b"L3.99") {
        return false;
    }

    let start = encoder
        .iter()
        .position(|byte| !b"EMAL".contains(byte))
        .unwrap_or(encoder.len());
    let version = &encoder[start..];
    let Some((major, rest)) = version.split_first() else {
        return false;
    };
    let rest = rest.strip_prefix(b".").unwrap_or(rest);
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();

    if !major.is_ascii_digit() || digits == 0 {
        return false;
    }

    let minor = rest[..digits]
        .iter()
        .fold(0u32, |value, digit| value * 10 + (digit - b'0') as u32);

    (major - b'0', minor) >= (3, 90)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XingHeader<'a> {
    pub frame: FrameHeader,
    pub info: bool,
    pub frames: Option<u32>,
    pub bytes: Option<u32>,
    pub table: Option<&'a [u8; XING_TABLE_LENGTH]>,
    pub quality: Option<u32>,
    pub lame: Option<LameHeader>,
}

impl XingHeader<'_> {
    pub fn samples(&self) -> Option<u64> {
        let total = self.frames? as u64 * self.frame.samples_per_frame() as u64;
        let trimmed = self.lame.map_or(0, |lame| {
            lame.encoder_delay as u64 + lame.encoder_padding as u64
        });

        Some(total.saturating_sub(trimmed))
    }

    pub fn seek(&self, sample: u64) -> Option<usize> {
        let table = self.table?;
        let bytes = self.bytes? as u128;
        let total = self.frames? as u128 * self.frame.samples_per_frame() as u128;

        if total == 0 {
            return None;
        }

        let position = (sample as u128).min(total) * 100;
        let index = ((position / total) as usize).min(99);
        let remainder = position - index as u128 * total;
        let start = table[index] as u128;
        let end = table.get(index + 1).map_or(256, |value| *value as u128);
        let scaled = start * total + (end.max(start) - start) * remainder;

        Some((scaled * bytes / (256 * total)) as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VbriHeader<'a> {
    pub frame: FrameHeader,
    pub version: u16,
    pub delay: u16,
    pub quality: u16,
    pub bytes: u32,
    pub frames: u32,
    pub scale: u16,
    pub entry_size: u16,
    pub frames_per_entry: u16,
    pub table: &'a [u8],
}

impl VbriHeader<'_> {
    pub fn samples(&self) -> Option<u64> {
        Some(self.frames as u64 * self.frame.samples_per_frame() as u64)
    }

    pub fn entries(&self) -> usize {
        self.table
            .len()
            .checked_div(self.entry_size as usize)
            .unwrap_or(0)
    }

    pub fn entry(&self, index: usize) -> Option<u64> {
        let size = self.entry_size as usize;
        let start = index.checked_mul(size)?;
        let bytes = self.table.get(start..start.checked_add(size)?)?;
        let value = bytes
            .iter()
            .fold(0u64, |value, byte| (value << 8) | *byte as u64);

        Some(value * self.scale as u64)
    }

    pub fn seek(&self, sample: u64) -> Option<usize> {
        let samples_per_frame = self.frame.samples_per_frame() as u64;
        let frames_per_entry = self.frames_per_entry as u64;

        if frames_per_entry == 0 || self.entry_size == 0 {
            return None;
        }

        let frame = (sample / samples_per_frame).min(self.frames as u64);
        let index = (frame / frames_per_entry) as usize;
        let mut position = self.frame.frame_length() as u64;

        for entry in 0..index.min(self.entries()) {
            position += self.entry(entry)?;
        }

        if let Some(entry) = self.entry(index) {
            let within = frame - index as u64 * frames_per_entry;
            position += entry * within / frames_per_entry;
        }

        Some(position as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VbrHeader<'a> {
    Xing(XingHeader<'a>),
    Vbri(VbriHeader<'a>),
}

impl<'a> VbrHeader<'a> {
    pub fn parse(bytes: &'a [u8], offset: usize) -> Result<Option<Self>> {
        let frame = FrameHeader::parse(bytes, offset)?;

        if frame.layer != Layer::Layer3 {
            return Ok(None);
        }

        let length = frame.frame_length();
        let data = Window::new(bytes, offset).get(Location::new(offset, length))?;

        if let Some(xing) = parse_xing(frame, data) {
            return Ok(Some(VbrHeader::Xing(xing)));
        }

        Ok(parse_vbri(frame, data).map(VbrHeader::Vbri))
    }

    pub const fn frame(&self) -> FrameHeader {
        match self {
            VbrHeader::Xing(xing) => xing.frame,
            VbrHeader::Vbri(vbri) => vbri.frame,
        }
    }

    pub fn frames(&self) -> Option<u32> {
        match self {
            VbrHeader::Xing(xing) => xing.frames,
            VbrHeader::Vbri(vbri) => Some(vbri.frames),
        }
    }

    pub fn bytes(&self) -> Option<u32> {
        match self {
            VbrHeader::Xing(xing) => xing.bytes,
            VbrHeader::Vbri(vbri) => Some(vbri.bytes),
        }
    }

    pub fn samples(&self) -> Option<u64> {
        match self {
            VbrHeader::Xing(xing) => xing.samples(),
            VbrHeader::Vbri(vbri) => vbri.samples(),
        }
    }

    pub fn seek(&self, sample: u64) -> Option<usize> {
        match self {
            VbrHeader::Xing(xing) => xing.seek(sample),
            VbrHeader::Vbri(vbri) => vbri.seek(sample),
        }
    }

    pub fn lame(&self) -> Option<LameHeader> {
        match self {
            VbrHeader::Xing(xing) => xing.lame,
            VbrHeader::Vbri(_) => None,
        }
    }
}

fn parse_xing(frame: FrameHeader, data: &[u8]) -> Option<XingHeader<'_>> {
    let mut rest = data.get(frame.xing_offset()..)?;
    let (tag, after) = rest.split_first_chunk::<4>()?;
    let info = match tag {
        b"Xing" => false,
        b"Info" => true,
        _ => return None,
    };
    let (flags, after) = after.split_first_chunk::<4>()?;
    let flags = u32::from_be_bytes(*flags);
    rest = after;

    let mut field = |present: bool| -> Option<Option<u32>> {
        if !present {
            return Some(None);
        }

        let (value, after) = rest.split_first_chunk::<4>()?;
        rest = after;

        Some(Some(u32::from_be_bytes(*value)))
    };

    let frames = field(flags & 0x01 != 0)?;
    let bytes = field(flags & 0x02 != 0)?;
    let table = if flags & 0x04 != 0 {
        let (table, after) = rest.split_first_chunk::<XING_TABLE_LENGTH>()?;
        rest = after;

        Some(table)
    } else {
        None
    };
    let quality = if flags & 0x08 != 0 {
        let (value, after) = rest.split_first_chunk::<4>()?;
        rest = after;

        Some(u32::from_be_bytes(*value))
    } else {
        None
    };

    Some(XingHeader {
        frame,
        info,
        frames,
        bytes,
        table,
        quality,
        lame: LameHeader::parse(rest),
    })
}

fn parse_vbri(frame: FrameHeader, data: &[u8]) -> Option<VbriHeader<'_>> {
    let header = data.get(VBRI_OFFSET..VBRI_OFFSET + VBRI_HEADER_LENGTH)?;

    if &header[..4] != b"VBRI" || read_u16_be(&header[4..6]) != 1 {
        return None;
    }

    let entries = read_u16_be(&header[18..20]) as usize;
    let entry_size = read_u16_be(&header[22..24]);

    if !(1..=4).contains(&entry_size) {
        return None;
    }

    let start = VBRI_OFFSET + VBRI_HEADER_LENGTH;
    let table = data.get(start..start + entries * entry_size as usize)?;

    Some(VbriHeader {
        frame,
        version: 1,
        delay: read_u16_be(&header[6..8]),
        quality: read_u16_be(&header[8..10]),
        bytes: read_u32_be(&header[10..14]),
        frames: read_u32_be(&header[14..18]),
        scale: read_u16_be(&header[20..22]),
        entry_size,
        frames_per_entry: read_u16_be(&header[24..26]),
        table,
    })
}
