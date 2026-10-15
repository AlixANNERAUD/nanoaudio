#![no_std]
#![doc = include_str!("../README.md")]

mod stream;

use core::marker::PhantomData;

use nanomp3_core::MAX_SAMPLES_PER_FRAME;
pub use nanomp3_core::Sample as Mp3Sample;

use nanoaudio_core::codec::{Decoder, Progress, Seek, SeekRequest};
use nanoaudio_core::{Configuration, Error, Result, Sample};

use stream::{Header, Kind, Table, Tag, find_frame};

pub const MP3_DEFAULT_MINIMUM_INPUT: usize = 16 * 1024;

const SYNC_SLACK: u64 = 4;
const SYNC_KEPT: usize = 4096;
const SETTLE_FRAMES: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stream {
    first: u64,
    header: Header,
    tag_size: Option<u64>,
    kind: Kind,
    frames: Option<u32>,
    table: Table,
    variable: bool,
    observed: Option<(u64, u64)>,
    counting: bool,
}

impl Stream {
    fn new(first: u64, frame: &[u8], header: Header) -> Self {
        let tag = Tag::parse(frame, header);
        let tagged = tag.kind != Kind::Plain;

        Self {
            first,
            header,
            tag_size: tagged.then_some(header.size() as u64),
            kind: tag.kind,
            frames: tag.frames,
            table: tag.table,
            variable: false,
            observed: Some((0, 0)),
            counting: true,
        }
    }

    fn is_constant(&self) -> bool {
        self.kind != Kind::Variable && !self.variable
    }

    fn tag_frames(&self) -> u64 {
        u64::from(self.tag_size.is_some())
    }

    fn frame_bytes(&self) -> (u64, u64) {
        let header = self.header;
        let nominal = (
            u64::from(header.samples() * header.bitrate() * 125),
            u64::from(header.sample_rate()),
        );

        if self.is_constant() {
            return nominal;
        }

        let last = self.table.length.checked_sub(1);
        let tagged = last.map(|last| (self.table.bytes[last], self.table.frames[last]));

        match (tagged, self.observed) {
            (Some((bytes, frames)), _) if frames > 0 => (u64::from(bytes), u64::from(frames)),
            (_, Some((bytes, frames))) if frames > 0 => (bytes, frames),
            _ => nominal,
        }
    }

    fn preroll(&self) -> u64 {
        let reservoir = self.header.maximum_reservoir() as u64;

        if reservoir == 0 {
            return SETTLE_FRAMES;
        }

        let (bytes, frames) = self.frame_bytes();
        let main = (bytes / frames)
            .saturating_sub(self.header.side_information_size() as u64)
            .max(1);
        let margin = if self.is_constant() { 1 } else { 2 };

        (margin * reservoir).div_ceil(main) + SETTLE_FRAMES
    }

    fn total(&self) -> Option<u64> {
        self.frames.map(|frames| {
            (u64::from(frames) + self.tag_frames()) * u64::from(self.header.samples())
        })
    }

    fn position(&self, frame: u64) -> u64 {
        let (bytes, frames) = self.frame_bytes();

        if self.is_constant() {
            let distance = u128::from(frame) * u128::from(bytes) / u128::from(frames);

            return self
                .first
                .saturating_add(u64::try_from(distance).unwrap_or(u64::MAX));
        }

        let audio = frame.saturating_sub(self.tag_frames());
        let distance = self.table.position(audio).unwrap_or_else(|| {
            let distance = u128::from(audio) * u128::from(bytes) / u128::from(frames);

            u64::try_from(distance)
                .unwrap_or(u64::MAX)
                .saturating_add(self.tag_size.unwrap_or(0))
        });

        self.first.saturating_add(distance)
    }

    fn index(&self, byte: u64) -> u64 {
        let (bytes, frames) = self.frame_bytes();
        let distance = u128::from(byte.saturating_sub(self.first));

        ((2 * distance * u128::from(frames) + u128::from(bytes)) / (2 * u128::from(bytes))) as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Decode,
    Sync {
        target: u64,
        start: u64,
        within: usize,
    },
    Ended,
}

#[derive(Clone)]
pub struct Mp3Decoder<S: Sample + nanomp3_core::Sample = i16> {
    decoder: nanomp3_core::Decoder,
    configuration: Option<Configuration>,
    minimum_input: usize,
    finishing: bool,
    stream: Option<Stream>,
    phase: Phase,
    offset: u64,
    drop: u64,
    skip: usize,
    sample: PhantomData<S>,
}

impl<S: Sample + nanomp3_core::Sample> Mp3Decoder<S> {
    pub const fn new() -> Self {
        Self::with_minimum_input(MP3_DEFAULT_MINIMUM_INPUT)
    }

    pub const fn with_minimum_input(minimum_input: usize) -> Self {
        Self {
            decoder: nanomp3_core::Decoder::new(),
            configuration: None,
            minimum_input,
            finishing: false,
            stream: None,
            phase: Phase::Decode,
            offset: 0,
            drop: 0,
            skip: 0,
            sample: PhantomData,
        }
    }

    pub fn reset(&mut self) {
        self.decoder = nanomp3_core::Decoder::new();
        self.configuration = None;
        self.finishing = false;
        self.stream = None;
        self.phase = Phase::Decode;
        self.offset = 0;
        self.drop = 0;
        self.skip = 0;
    }

    fn synchronize(&mut self, input: &[u8], target: u64, start: u64, within: usize) -> usize {
        let Some(stream) = self.stream else {
            return 0;
        };
        let Some((index, _)) = find_frame(input, Some(stream.header)) else {
            return if self.finishing {
                input.len()
            } else {
                input.len().saturating_sub(SYNC_KEPT)
            };
        };
        let reached = if stream.is_constant() {
            stream.index(self.offset + index as u64)
        } else {
            start
        };

        self.phase = Phase::Decode;
        self.drop = target.saturating_sub(reached);
        self.skip = if reached <= target { within } else { 0 };

        index
    }

    fn observe(&mut self, input: &[u8]) {
        if let Some((index, header)) = find_frame(input, None) {
            let frame = &input[index..(index + header.size()).min(input.len())];

            self.stream = Some(Stream::new(self.offset + index as u64, frame, header));
        }
    }

    fn account(
        &mut self,
        information: &nanomp3_core::FrameInfo,
        output: &mut [S],
        produced: usize,
    ) -> usize {
        if let Some(stream) = &mut self.stream {
            if information.bitrate != stream.header.bitrate() && stream.kind != Kind::Variable {
                stream.variable = true;
            }

            if let Some((bytes, frames)) = stream.observed.as_mut().filter(|_| stream.counting) {
                *bytes = self.offset - stream.first;
                *frames += 1;
            }
        }

        if self.drop != 0 {
            self.drop -= 1;

            return 0;
        }

        if self.skip == 0 {
            return produced;
        }

        let skipped = (self.skip * information.channels.num() as usize).min(produced);

        output.copy_within(skipped..produced, 0);
        self.skip = 0;

        produced - skipped
    }
}

impl<S: Sample + nanomp3_core::Sample> Default for Mp3Decoder<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Sample + nanomp3_core::Sample> Decoder for Mp3Decoder<S> {
    type Sample = S;

    fn configuration(&self) -> Option<Configuration> {
        self.configuration
    }

    fn minimum_input(&self) -> usize {
        self.minimum_input
    }

    fn minimum_output(&self) -> usize {
        MAX_SAMPLES_PER_FRAME
    }

    fn finish(&mut self) {
        self.finishing = true;
    }

    fn decode(&mut self, input: &[u8], output: &mut [S]) -> Result<Progress> {
        if output.len() < MAX_SAMPLES_PER_FRAME {
            return Err(Error::BufferTooSmall);
        }

        if input.is_empty() || (!self.finishing && input.len() < self.minimum_input) {
            return Ok(Progress::default());
        }

        match self.phase {
            Phase::Decode => {}
            Phase::Ended => {
                self.offset += input.len() as u64;

                return Ok(Progress::new(input.len(), 0));
            }
            Phase::Sync {
                target,
                start,
                within,
            } => {
                let skipped = self.synchronize(input, target, start, within);

                if skipped != 0 || self.phase != Phase::Decode {
                    self.offset += skipped as u64;

                    return Ok(Progress::new(skipped, 0));
                }
            }
        }

        if self.stream.is_none() {
            self.observe(input);
        }

        let (consumed, result) = self.decoder.decode(input, output);

        self.offset += consumed as u64;

        let produced = match result {
            Ok(information) => {
                self.configuration = Some(Configuration::new(
                    information.sample_rate,
                    information.channels.num(),
                    S::FORMAT,
                ));

                let produced = information.samples_produced * information.channels.num() as usize;

                self.account(&information, output, produced)
            }
            Err(error) => {
                if let Some(information) = error.frame_info() {
                    self.account(information, output, 0);
                }

                0
            }
        };

        Ok(Progress::new(consumed, produced))
    }
}

impl<S: Sample + nanomp3_core::Sample> Seek for Mp3Decoder<S> {
    fn seek(&mut self, frame: u64) -> Result<SeekRequest> {
        let Some(stream) = self.stream else {
            if frame != 0 {
                return Err(Error::Incomplete);
            }

            self.reset();

            return Ok(SeekRequest::default());
        };

        if stream.total().is_some_and(|total| frame > total) {
            return Err(Error::InvalidParameter);
        }

        self.decoder = nanomp3_core::Decoder::new();
        self.finishing = false;
        self.drop = 0;
        self.skip = 0;

        if let Some(stream) = &mut self.stream {
            stream.observed = stream.observed.filter(|(_, frames)| *frames > 0);
            stream.counting = false;
        }

        let samples = u64::from(stream.header.samples());
        let target = frame / samples;
        let within = frame % samples;

        if stream.total() == Some(frame) {
            self.phase = Phase::Ended;
            self.offset = stream.position(target);

            return Ok(SeekRequest {
                byte_offset: self.offset,
                skip_frames: 0,
            });
        }

        let start = target.saturating_sub(stream.preroll());
        let byte_offset = match start {
            0 => stream.first,
            _ if stream.is_constant() => stream
                .position(start)
                .saturating_sub(SYNC_SLACK)
                .max(stream.first),
            _ => stream.position(start),
        };

        self.phase = Phase::Sync {
            target,
            start,
            within: within as usize,
        };
        self.offset = byte_offset;

        Ok(SeekRequest {
            byte_offset,
            skip_frames: u32::try_from((target - start) * samples + within).unwrap_or(u32::MAX),
        })
    }
}
