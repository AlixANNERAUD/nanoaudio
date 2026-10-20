#![no_std]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr;

use nanoaudio_core::codec::{Decoder, Progress, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Result, Sample};
use nanoaudio_ogg::{OGG_MAXIMUM_PAGE_SIZE, OggEvent, OggPacket, OggPacketReader, OggPage};

/// Output sample rate of every Opus stream, in Hz.
pub const OPUS_SAMPLE_RATE: u32 = 48_000;
/// Largest packet the decoder keeps, in bytes. Larger packets are skipped.
///
/// It holds a 120 ms packet at 510 kbit/s, the highest Opus bitrate.
pub const OPUS_MAXIMUM_PACKET_SIZE: usize = 8 * 1024;
/// Default `MAXIMUM_PACKET_FRAMES` of [`OpusDecoder`]: 120 ms, the longest Opus packet.
pub const OPUS_DEFAULT_MAXIMUM_PACKET_FRAMES: usize = 5_760;
/// Frames decoded and dropped before the target of a seek, so the decoder state has settled: 80 ms, as RFC 7845 recommends.
pub const OPUS_PRE_ROLL: u64 = 3_840;

const MAXIMUM_CHANNELS: usize = 2;
const HEAD_SIZE: usize = 19;
const CONCEALMENT_STEP: usize = 120;
const MAXIMUM_CONCEALED: u64 = 48_000;
const LINEAR_FRAMES: u64 = 24_000;
const MAXIMUM_SEEK_ROUNDS: u32 = 64;
const FALLBACK_BYTES_PER_SECOND: u64 = 8_000;
const UNKNOWN: u64 = u64::MAX;

/// The identification header of an Ogg Opus stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpusHead {
    /// Number of output channels, 1 or 2.
    pub channels: u8,
    /// Frames at 48 kHz to drop at the start of the decoded audio.
    pub pre_skip: u16,
    /// Sample rate of the audio before encoding, in Hz, for information only; 0 if unknown.
    pub input_sample_rate: u32,
    /// Gain applied to the output, in dB in Q7.8.
    pub output_gain: i16,
    /// Channel mapping family: 0, or 1 when it describes a plain mono or stereo stream.
    pub mapping_family: u8,
}

impl OpusHead {
    /// Parses an `OpusHead` packet.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidHeader`] if the packet is not an `OpusHead` or is malformed, and [`Error::UnsupportedFormat`] for a later major version, or for more than two channels or a channel mapping other than plain mono or stereo.
    pub fn parse(packet: &[u8]) -> Result<Self> {
        if packet.len() < HEAD_SIZE || &packet[..8] != b"OpusHead" {
            return Err(Error::InvalidHeader);
        }

        if packet[8] >> 4 != 0 {
            return Err(Error::UnsupportedFormat);
        }

        let channels = packet[9];
        let mapping_family = packet[18];

        if channels == 0 {
            return Err(Error::InvalidHeader);
        }

        if channels as usize > MAXIMUM_CHANNELS {
            return Err(Error::UnsupportedFormat);
        }

        match mapping_family {
            0 => {}
            1 => {
                let table = packet
                    .get(HEAD_SIZE..HEAD_SIZE + 2 + channels as usize)
                    .ok_or(Error::InvalidHeader)?;
                let plain = table[0] == 1
                    && table[1] == channels - 1
                    && table[2..]
                        .iter()
                        .enumerate()
                        .all(|(index, &channel)| channel as usize == index);

                if !plain {
                    return Err(Error::UnsupportedFormat);
                }
            }
            _ => return Err(Error::UnsupportedFormat),
        }

        Ok(Self {
            channels,
            pre_skip: u16::from_le_bytes([packet[10], packet[11]]),
            input_sample_rate: u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]),
            output_gain: i16::from_le_bytes([packet[16], packet[17]]),
            mapping_family,
        })
    }
}

fn packet_frames(packet: &[u8]) -> Option<u64> {
    let &toc = packet.first()?;
    let configuration = toc >> 3;
    let frame = match configuration {
        0..=11 => [480, 960, 1_920, 2_880][(configuration & 3) as usize],
        12..=15 => [480, 960][(configuration & 1) as usize],
        _ => [120, 240, 480, 960][(configuration & 3) as usize],
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u64::from(*packet.get(1)? & 0x3F),
    };
    let frames = frame * count;

    (frames != 0 && frames <= OPUS_DEFAULT_MAXIMUM_PACKET_FRAMES as u64).then_some(frames)
}

fn own_frames(page: &OggPage) -> u64 {
    let count = page.packets().count();

    page.packets()
        .enumerate()
        .filter(|&(index, _)| !(index == 0 && page.is_continued()))
        .filter(|&(index, _)| !(index + 1 == count && page.has_unfinished_packet()))
        .map(|(_, packet)| packet_frames(packet).unwrap_or(0))
        .sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Head,
    Tags,
    Audio,
    Search,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Point {
    byte: u64,
    end: u64,
    granule: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Search {
    aim: u64,
    low: Point,
    high: Point,
    probe: u64,
    bisect: bool,
    stretch: u32,
    rounds: u32,
    hint: Option<u64>,
    high_page: u64,
}

#[derive(Debug, Clone, Copy)]
struct Status {
    ready: bool,
    state: State,
    head: Option<OpusHead>,
    gain: f32,
    decoded_start: usize,
    decoded_end: usize,
    position: Option<u64>,
    first: Option<u64>,
    skip_to: Option<u64>,
    end: Option<u64>,
    last_granule: Option<u64>,
    conceal: u64,
    finishing: bool,
    offset: u64,
    audio_offset: Option<u64>,
    length: Option<u64>,
    last_page: Option<Point>,
    search: Search,
    error: Option<Error>,
}

impl Status {
    const fn new() -> Self {
        let point = Point {
            byte: 0,
            end: 0,
            granule: 0,
        };

        Self {
            ready: false,
            state: State::Head,
            head: None,
            gain: 1.0,
            decoded_start: 0,
            decoded_end: 0,
            position: None,
            first: None,
            skip_to: None,
            end: None,
            last_granule: None,
            conceal: 0,
            finishing: false,
            offset: 0,
            audio_offset: None,
            length: None,
            last_page: None,
            search: Search {
                aim: 0,
                low: point,
                high: point,
                probe: 0,
                bisect: false,
                stretch: 0,
                rounds: 0,
                hint: None,
                high_page: UNKNOWN,
            },
            error: None,
        }
    }
}

/// An Ogg Opus decoder built on [`opus-rs`](https://crates.io/crates/opus-rs) that does not allocate.
///
/// It decodes mono and stereo streams (channel mapping family 0, or family 1 when it describes the same layout) to 48 kHz interleaved samples. It drops the pre-skip at the start and trims the end to the final granule position, applies the output gain, and conceals lost or corrupt packets.
///
/// The decoder holds the `opus-rs` state (about 175 KB), one packet and one decoded packet of `MAXIMUM_PACKET_FRAMES` frames per channel in `f32`. The default of 120 ms, the longest Opus packet, takes 45 KiB; 960 frames (20 ms, what most encoders write) takes 7.5 KiB, and longer packets are then rejected. To keep it out of the stack, build it in place with [`init`](Self::init), or in a `static` with [`new`](Self::new), which is `const`.
///
/// ```
/// use core::mem::MaybeUninit;
/// use nanoaudio_opus::OpusDecoder;
///
/// let mut slot = Box::new(MaybeUninit::<OpusDecoder>::uninit());
/// let decoder = OpusDecoder::init(&mut slot);
///
/// assert!(decoder.head().is_none());
/// ```
pub struct OpusDecoder<
    S: Sample = i16,
    const MAXIMUM_PACKET_FRAMES: usize = OPUS_DEFAULT_MAXIMUM_PACKET_FRAMES,
> {
    opus: MaybeUninit<opus_rs::OpusDecoder>,
    reader: OggPacketReader,
    packet: [u8; OPUS_MAXIMUM_PACKET_SIZE],
    decoded: [[f32; MAXIMUM_CHANNELS]; MAXIMUM_PACKET_FRAMES],
    status: Status,
    sample: PhantomData<S>,
}

impl<S: Sample, const MAXIMUM_PACKET_FRAMES: usize> OpusDecoder<S, MAXIMUM_PACKET_FRAMES> {
    const CHECK: () = assert!(
        MAXIMUM_PACKET_FRAMES >= 120 && MAXIMUM_PACKET_FRAMES <= OPUS_DEFAULT_MAXIMUM_PACKET_FRAMES,
        "the maximum packet length must be between 120 and 5760 frames"
    );

    /// Creates a decoder waiting for the first page.
    ///
    /// The value is large: use it to initialize a `static`, or use [`init`](Self::init).
    pub const fn new() -> Self {
        let () = Self::CHECK;

        Self {
            opus: MaybeUninit::uninit(),
            reader: OggPacketReader::new(),
            packet: [0; OPUS_MAXIMUM_PACKET_SIZE],
            decoded: [[0.0; MAXIMUM_CHANNELS]; MAXIMUM_PACKET_FRAMES],
            status: Status::new(),
            sample: PhantomData,
        }
    }

    /// Builds a decoder in `slot`, without moving it through the stack, and returns it.
    ///
    /// `slot` can be a `static`, for example from `static_cell`, or a heap allocation.
    pub fn init(slot: &mut MaybeUninit<Self>) -> &mut Self {
        let () = Self::CHECK;
        let this = slot.as_mut_ptr();

        unsafe {
            ptr::write_bytes(&raw mut (*this).packet, 0, 1);
            ptr::write_bytes(&raw mut (*this).decoded, 0, 1);
            (&raw mut (*this).reader).write(OggPacketReader::new());
            (&raw mut (*this).status).write(Status::new());
            (&raw mut (*this).sample).write(PhantomData);

            slot.assume_init_mut()
        }
    }

    /// Returns to the state of [`new`](Self::new), to decode another stream.
    pub fn reset(&mut self) {
        self.release();
        self.reader.reset();
        self.status = Status::new();
    }

    /// Returns the `OpusHead` of the stream, once it is read.
    pub const fn head(&self) -> Option<OpusHead> {
        self.status.head
    }

    /// Returns the length of the stream in frames, once its last page is read.
    pub fn frames(&self) -> Option<u64> {
        let start = self.start()?;

        Some(self.status.last_granule?.saturating_sub(start))
    }

    /// Gives the stream length in bytes, counted from the first byte fed to the decoder, so that seeking can interpolate from the first request.
    pub fn set_stream_length(&mut self, length: u64) {
        self.status.length = Some(length);
    }

    fn start(&self) -> Option<u64> {
        Some(self.status.first.unwrap_or(0) + u64::from(self.status.head?.pre_skip))
    }

    fn channels(&self) -> usize {
        self.status
            .head
            .map_or(MAXIMUM_CHANNELS, |head| head.channels as usize)
    }

    fn release(&mut self) {
        if core::mem::take(&mut self.status.ready) {
            unsafe { self.opus.assume_init_drop() };
        }
    }

    fn prepare(&mut self) -> Result<()> {
        let channels = self.channels();

        self.release();
        opus_rs::OpusDecoder::init(&mut self.opus, OPUS_SAMPLE_RATE as i32, channels)
            .map_err(|_| Error::InvalidParameter)?;
        self.status.ready = true;

        Ok(())
    }

    fn page_frames(&self, page: &OggPage) -> u64 {
        let pending = self.reader.pending();
        let count = page.packets().count();
        let mut frames = 0;

        for (index, part) in page.packets().enumerate() {
            if index + 1 == count && page.has_unfinished_packet() {
                break;
            }

            let length = match (index, page.is_continued()) {
                (0, true) if pending == 0 => continue,
                (0, true) => {
                    let second = match pending {
                        1 => part.first().copied().unwrap_or(0),
                        _ => self.packet[1],
                    };

                    packet_frames(&[self.packet[0], second])
                }
                _ => packet_frames(part),
            };

            frames += length.unwrap_or(0);
        }

        frames
    }

    fn begin_page(&mut self, page: &OggPage, start: u64, lost: bool) {
        let status = &mut self.status;

        status.end = None;

        if status.audio_offset.is_none() {
            status.audio_offset = Some(start);
        }

        let Some(granule) = page.granule_position() else {
            return;
        };

        if page.is_end_of_stream() {
            status.end = Some(granule);
            status.last_granule = Some(granule);
        }

        if status.position.is_some() && !lost {
            if !page.is_end_of_stream() {
                status.last_page = Some(Point {
                    byte: start,
                    end: start + page.size() as u64,
                    granule,
                });
            }

            return;
        }

        let begin = granule.saturating_sub(self.page_frames(page));
        let status = &mut self.status;

        match status.position {
            None => {
                status.position = Some(begin);

                if status.first.is_none() {
                    status.first = Some(begin);
                }

                if status.skip_to.is_none() {
                    let pre_skip = status.head.map_or(0, |head| u64::from(head.pre_skip));

                    status.skip_to = Some(begin + pre_skip);
                }
            }
            Some(_) if page.is_end_of_stream() => {}
            Some(position) => {
                let gap = begin.saturating_sub(position);

                if gap != 0 && gap <= MAXIMUM_CONCEALED {
                    status.conceal = gap - gap % CONCEALMENT_STEP as u64;
                }

                status.position = Some(begin - status.conceal);
            }
        }
    }

    fn run(&mut self, packet: Option<usize>, frames: usize) {
        let channels = self.channels();
        let output = &mut self.decoded.as_flattened_mut()[..MAXIMUM_PACKET_FRAMES * channels];
        let opus = unsafe { self.opus.assume_init_mut() };
        let decoded = match packet {
            Some(length) => opus.decode(&self.packet[..length], MAXIMUM_PACKET_FRAMES, output),
            None => Err(""),
        };
        let produced = match decoded {
            Ok(produced) => produced.min(MAXIMUM_PACKET_FRAMES),
            Err(_) => match opus.decode(&[], frames, output) {
                Ok(produced) => produced.min(frames),
                Err(_) => {
                    output[..frames * channels].fill(0.0);
                    frames
                }
            },
        };

        let status = &mut self.status;
        let Some(start) = status.position else {
            status.decoded_start = 0;
            status.decoded_end = 0;

            return;
        };
        let end = start + produced as u64;
        let first = start.max(status.skip_to.unwrap_or(0)).min(end);
        let last = status.end.map_or(end, |limit| end.min(limit.max(first)));

        status.position = Some(end);
        status.decoded_start = (first - start) as usize * channels;
        status.decoded_end = (last - start) as usize * channels;
    }

    fn decode_packet(&mut self, packet: OggPacket) -> Result<()> {
        let Some(frames) = packet_frames(&self.packet[..packet.length]) else {
            return Ok(());
        };

        if frames as usize > MAXIMUM_PACKET_FRAMES {
            if let Some(position) = &mut self.status.position {
                *position += frames;
            }

            return Err(Error::UnsupportedFormat);
        }

        self.run(Some(packet.length), frames as usize);

        Ok(())
    }

    fn handle_packet(&mut self, packet: OggPacket) -> Result<()> {
        match self.status.state {
            State::Head => {
                let head = OpusHead::parse(&self.packet[..packet.length])?;

                self.status.head = Some(head);
                self.status.gain = match head.output_gain {
                    0 => 1.0,
                    gain => libm::powf(10.0, f32::from(gain) / (20.0 * 256.0)),
                };
                self.prepare()?;
                self.status.state = State::Tags;
            }
            State::Tags => {
                if packet.length < 8 || &self.packet[..8] != b"OpusTags" {
                    return Err(Error::InvalidHeader);
                }

                self.status.state = State::Audio;
                self.status.audio_offset = Some(self.status.offset);
            }
            _ => self.decode_packet(packet)?,
        }

        if packet.end_of_stream {
            self.status.state = State::Ended;
        }

        Ok(())
    }

    fn drain(&mut self, output: &mut [S]) -> usize {
        let channels = self.channels();
        let status = &mut self.status;
        let samples =
            (status.decoded_end - status.decoded_start).min(output.len() / channels * channels);
        let decoded =
            &self.decoded.as_flattened()[status.decoded_start..status.decoded_start + samples];

        for (output, &value) in output.iter_mut().zip(decoded) {
            *output = S::from_f32(value * status.gain);
        }

        status.decoded_start += samples;

        samples
    }

    fn conceal(&mut self) {
        let step = (MAXIMUM_PACKET_FRAMES - MAXIMUM_PACKET_FRAMES % CONCEALMENT_STEP) as u64;
        let frames = self.status.conceal.min(step);

        self.status.conceal -= frames;
        self.run(None, frames as usize);
    }

    fn step(&mut self, input: &[u8], progress: &mut Progress) -> Result<bool> {
        if self.status.conceal != 0 {
            self.conceal();

            return Ok(true);
        }

        let before = self.status.offset;
        let read = match self
            .reader
            .read(&input[progress.consumed..], &mut self.packet)
        {
            Err(Error::BufferTooSmall) => {
                if self.status.state == State::Tags {
                    self.status.state = State::Audio;
                }

                return Ok(true);
            }
            read => read?,
        };

        progress.consumed += read.consumed;
        self.status.offset += read.consumed as u64;

        match read.event {
            Some(OggEvent::Page { page, lost }) => {
                if self.status.state == State::Audio {
                    let start = before + read.consumed as u64 - page.header_size() as u64;

                    self.begin_page(&page, start, lost);
                }

                Ok(true)
            }
            Some(OggEvent::Packet(packet)) => {
                self.handle_packet(packet)?;

                Ok(true)
            }
            None if self.reader.is_ended() => {
                self.status.state = State::Ended;

                Ok(true)
            }
            None => Ok(read.consumed != 0),
        }
    }

    fn close(&self, low: Point, high: Point) -> bool {
        high.byte <= low.end || self.status.search.aim - low.granule <= LINEAR_FRAMES
    }

    fn land(&mut self, at: u64) -> u64 {
        let status = &mut self.status;

        status.state = State::Audio;
        status.position = None;
        status.offset = at;
        self.reader.resync();

        at
    }

    fn narrow(&mut self, low: Point, high: Point) -> Error {
        let search = &mut self.status.search;
        let before = search.high.byte.saturating_sub(search.low.byte);

        search.low = low;
        search.high = high;
        search.bisect = high.byte != UNKNOWN && high.byte - low.byte > before / 2;
        search.rounds += 1;

        if search.rounds > MAXIMUM_SEEK_ROUNDS || self.close(low, high) {
            return Error::SeekTo(self.land(low.byte));
        }

        Error::SeekTo(self.probe())
    }

    fn probe(&mut self) -> u64 {
        let first = self.status.first.unwrap_or(0);
        let audio_offset = self.status.audio_offset.unwrap_or(0);
        let search = &mut self.status.search;
        let low = search.low;
        let high = search.high;
        let base = low.end.max(low.byte);
        let distance = u128::from(search.aim - low.granule);
        let probe = if let Some(hint) = search.hint.take() {
            u128::from(hint)
        } else if high.byte == UNKNOWN {
            let (bytes, granules) = match low.granule > first {
                true => (low.byte - audio_offset, low.granule - first),
                false => (FALLBACK_BYTES_PER_SECOND, u64::from(OPUS_SAMPLE_RATE)),
            };
            let estimate = distance * u128::from(bytes.max(1)) / u128::from(granules);

            search.stretch += 1;

            u128::from(base) + (estimate << search.stretch.min(32))
        } else if search.bisect || high.granule == UNKNOWN || high.granule <= low.granule {
            u128::from(base + high.byte.saturating_sub(base) / 2)
        } else {
            u128::from(base)
                + u128::from(high.byte.saturating_sub(base)) * distance
                    / u128::from(high.granule - low.granule)
        };
        let probe = (probe.min(u128::from(UNKNOWN - 1)) as u64)
            .clamp(base, high.byte.saturating_sub(1).max(base));

        search.probe = probe;
        self.status.state = State::Search;
        self.status.offset = probe;
        self.status.finishing = false;

        probe
    }

    fn search_step(&mut self, input: &[u8], progress: &mut Progress) -> Result<bool> {
        let remaining = &input[progress.consumed..];
        let search = self.status.search;
        let found = OggPage::find(remaining);
        let page = found.map(|index| (index, OggPage::parse(&remaining[index..])));

        let (index, page) = match page {
            Some((index, Ok(page))) => (index, page),
            Some((index, Err(Error::Incomplete))) if !self.status.finishing => {
                progress.consumed += index;
                self.status.offset += index as u64;

                return Ok(false);
            }
            Some((index, Err(error))) if error != Error::Incomplete => {
                progress.consumed += index + 1;
                self.status.offset += index as u64 + 1;

                return Ok(true);
            }
            None if !self.status.finishing => {
                progress.consumed = input.len();
                self.status.offset += remaining.len() as u64;

                return Ok(false);
            }
            _ => {
                let high = Point {
                    byte: search.probe,
                    end: search.probe,
                    granule: UNKNOWN,
                };

                return Err(self.narrow(search.low, high));
            }
        };

        let start = self.status.offset + index as u64;
        let granule = page.granule_position();

        if Some(page.serial_number()) != self.reader.serial_number() || granule.is_none() {
            progress.consumed += index + page.size();
            self.status.offset += (index + page.size()) as u64;

            return Ok(true);
        }

        let granule = granule.unwrap_or(0);
        let begin = granule.saturating_sub(own_frames(&page));

        if begin > search.aim || page.is_end_of_stream() {
            let next =
                start != search.high_page && begin.saturating_sub(search.aim) <= granule - begin;

            self.status.search.high_page = start;
            self.status.search.hint = next.then(|| start.saturating_sub(2 * page.size() as u64));

            let high = Point {
                byte: search.probe,
                end: search.probe,
                granule: begin,
            };

            return Err(self.narrow(search.low, high));
        }

        let low = Point {
            byte: start,
            end: start + page.size() as u64,
            granule,
        };

        if granule >= search.aim || self.close(low, search.high) {
            progress.consumed += index;
            self.land(start);

            return Ok(true);
        }

        Err(self.narrow(low, search.high))
    }
}

impl<S: Sample, const MAXIMUM_PACKET_FRAMES: usize> Default
    for OpusDecoder<S, MAXIMUM_PACKET_FRAMES>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Sample, const MAXIMUM_PACKET_FRAMES: usize> Drop for OpusDecoder<S, MAXIMUM_PACKET_FRAMES> {
    fn drop(&mut self) {
        self.release();
    }
}

impl<S: Sample, const MAXIMUM_PACKET_FRAMES: usize> Decoder
    for OpusDecoder<S, MAXIMUM_PACKET_FRAMES>
{
    type Sample = S;

    fn configuration(&self) -> Option<Configuration> {
        self.status
            .head
            .map(|head| Configuration::new(OPUS_SAMPLE_RATE, head.channels, S::FORMAT))
    }

    /// Returns [`OGG_MAXIMUM_PAGE_SIZE`]: each page must be whole in the input before it is decoded.
    ///
    /// Most files have pages of a few kilobytes, so a smaller input works while each page fits in it.
    fn minimum_input(&self) -> usize {
        OGG_MAXIMUM_PAGE_SIZE
    }

    fn minimum_output(&self) -> usize {
        self.channels()
    }

    fn finish(&mut self) {
        self.status.finishing = true;
    }

    /// Decodes pages from `input` into interleaved samples in `output`.
    ///
    /// Junk and pages with a bad CRC are skipped, and the audio they held is concealed. After a seek, `decode` can return [`Error::SeekTo`] until it reaches the requested frame.
    fn decode(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        if output.len() < self.minimum_output() {
            return Err(Error::BufferTooSmall);
        }

        if let Some(error) = self.status.error.take() {
            return Err(error);
        }

        let mut progress = Progress::default();

        loop {
            if self.status.decoded_start < self.status.decoded_end {
                progress.produced += self.drain(&mut output[progress.produced..]);

                if self.status.decoded_start < self.status.decoded_end {
                    return Ok(progress);
                }
            }

            let result = match self.status.state {
                State::Ended => {
                    self.status.offset += (input.len() - progress.consumed) as u64;
                    progress.consumed = input.len();

                    return Ok(progress);
                }
                State::Search => self.search_step(input, &mut progress),
                _ => self.step(input, &mut progress),
            };

            match result {
                Ok(true) => {}
                Ok(false) => break,
                Err(error @ Error::SeekTo(_)) => return Err(error),
                Err(error) if progress.is_stalled() => return Err(error),
                Err(error) => {
                    self.status.error = Some(error);

                    return Ok(progress);
                }
            }
        }

        if progress.is_stalled()
            && self.status.finishing
            && input.len() > progress.consumed
            && self.status.state != State::Ended
        {
            return Err(Error::Incomplete);
        }

        Ok(progress)
    }
}

impl<S: Sample, const MAXIMUM_PACKET_FRAMES: usize> Seek for OpusDecoder<S, MAXIMUM_PACKET_FRAMES> {
    /// Starts a search for `frame` by granule position, and returns the byte offset to resume from, counted from the first byte fed to the decoder.
    ///
    /// The decoder resumes at least [`OPUS_PRE_ROLL`] frames before the target and drops them, so the output matches a sequential decode within the precision of the codec.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Incomplete`] before the headers are read, unless `frame` is 0, and [`Error::InvalidParameter`] past [`frames`](OpusDecoder::frames) once it is known.
    fn seek(&mut self, frame: u64) -> Result<SeekRequest> {
        let (Some(start), Some(audio_offset), true) =
            (self.start(), self.status.audio_offset, self.status.ready)
        else {
            if frame != 0 {
                return Err(Error::Incomplete);
            }

            let length = self.status.length;

            self.reset();
            self.status.length = length;

            return Ok(SeekRequest::default());
        };

        if self.frames().is_some_and(|frames| frame > frames) {
            return Err(Error::InvalidParameter);
        }

        self.prepare()?;

        let first = self.status.first.unwrap_or(0);
        let target = start + frame;
        let aim = target.saturating_sub(OPUS_PRE_ROLL);
        let status = &mut self.status;

        status.skip_to = Some(target);
        status.decoded_start = 0;
        status.decoded_end = 0;
        status.conceal = 0;
        status.end = None;
        status.finishing = false;
        status.error = None;

        let mut low = Point {
            byte: audio_offset,
            end: audio_offset,
            granule: first,
        };
        let mut high = Point {
            byte: status.length.unwrap_or(UNKNOWN),
            end: UNKNOWN,
            granule: UNKNOWN,
        };

        if let Some(last) = status.last_page {
            if last.granule <= aim && last.granule > low.granule {
                low = last;
            } else if last.granule > aim && last.byte < high.byte {
                high = last;
            }
        }

        status.search = Search {
            aim,
            low,
            high,
            probe: low.byte,
            bisect: false,
            stretch: 0,
            rounds: 0,
            hint: None,
            high_page: UNKNOWN,
        };

        if aim <= low.granule || self.close(low, high) {
            let skip_frames = u32::try_from(target - low.granule.min(target)).unwrap_or(u32::MAX);

            return Ok(SeekRequest {
                byte_offset: self.land(low.byte),
                skip_frames,
            });
        }

        Ok(SeekRequest {
            byte_offset: self.probe(),
            skip_frames: 0,
        })
    }
}
