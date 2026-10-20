extern crate std;

use std::vec;
use std::vec::Vec;

use super::*;

fn page(
    flags: u8,
    granule: u64,
    serial: u32,
    sequence: u32,
    lacing: &[u8],
    body: &[u8],
) -> Vec<u8> {
    let mut page = Vec::new();
    page.extend_from_slice(CAPTURE_PATTERN);
    page.push(VERSION);
    page.push(flags);
    page.extend_from_slice(&granule.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]);
    page.push(lacing.len() as u8);
    page.extend_from_slice(lacing);
    page.extend_from_slice(body);

    let crc = update_crc(0, &page);
    page[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    page
}

fn packet(index: usize, length: usize) -> Vec<u8> {
    (0..length)
        .map(|byte| (byte * 7 + index * 13) as u8)
        .collect()
}

fn paginate(packets: &[Vec<u8>], segments_per_page: usize, serial: u32) -> Vec<u8> {
    let mut segments = Vec::new();

    for (index, packet) in packets.iter().enumerate() {
        let mut offset = 0;

        loop {
            let length = (packet.len() - offset).min(255);
            segments.push((index, &packet[offset..offset + length]));
            offset += length;

            if length < 255 {
                break;
            }
        }
    }

    let pages: Vec<_> = segments.chunks(segments_per_page).collect();
    let mut stream = Vec::new();
    let mut continued = false;

    for (sequence, segments) in pages.iter().enumerate() {
        let lacing: Vec<u8> = segments
            .iter()
            .map(|(_, bytes)| bytes.len() as u8)
            .collect();
        let body: Vec<u8> = segments
            .iter()
            .flat_map(|(_, bytes)| bytes.iter().copied())
            .collect();
        let last = sequence == pages.len() - 1;
        let granule = segments
            .iter()
            .filter(|(_, bytes)| bytes.len() < 255)
            .map(|(index, _)| *index as u64 * 100)
            .next_back()
            .unwrap_or(NO_GRANULE_POSITION);
        let flags = (u8::from(continued) * CONTINUED)
            | (u8::from(sequence == 0) * BEGINNING_OF_STREAM)
            | (u8::from(last) * END_OF_STREAM);

        stream.extend(page(
            flags,
            granule,
            serial,
            sequence as u32,
            &lacing,
            &body,
        ));
        continued = *lacing.last().unwrap() == 255;
    }

    stream
}

#[derive(Debug, Default, PartialEq)]
struct Collected {
    packets: Vec<(Vec<u8>, Option<u64>, bool)>,
    pages: usize,
    lost: usize,
}

fn collect(stream: &[u8], chunk: usize, storage: &mut [u8]) -> (Collected, Result<()>) {
    let mut reader = OggPacketReader::new();
    let mut collected = Collected::default();
    let mut pending = Vec::new();
    let mut offset = 0;

    loop {
        let read = match reader.read(&pending, storage) {
            Ok(read) => read,
            Err(Error::BufferTooSmall) => continue,
            Err(error) => return (collected, Err(error)),
        };

        let consumed = read.consumed;
        let mut refill = false;

        match read.event {
            Some(OggEvent::Packet(packet)) => collected.packets.push((
                storage[..packet.length].to_vec(),
                packet.granule_position,
                packet.end_of_stream,
            )),
            Some(OggEvent::Page { lost, .. }) => {
                collected.pages += 1;
                collected.lost += usize::from(lost);
            }
            None if offset == stream.len() => return (collected, Ok(())),
            None => refill = true,
        }

        pending.drain(..consumed);

        if refill {
            let end = (offset + chunk).min(stream.len());
            pending.extend_from_slice(&stream[offset..end]);
            offset = end;
        }
    }
}

fn packets() -> Vec<Vec<u8>> {
    [0, 1, 254, 255, 256, 510, 600, 3]
        .iter()
        .enumerate()
        .map(|(index, &length)| packet(index, length))
        .collect()
}

#[test]
fn crc_matches_reference() {
    assert_eq!(update_crc(0, b"123456789"), 0x89A1_897F);
}

#[test]
fn parses_a_page() {
    let bytes = page(BEGINNING_OF_STREAM, 960, 7, 3, &[3, 0], b"abc");
    let page = OggPage::parse(&bytes).unwrap();

    assert_eq!(page.granule_position(), Some(960));
    assert_eq!(page.serial_number(), 7);
    assert_eq!(page.sequence_number(), 3);
    assert!(page.is_beginning_of_stream() && !page.is_continued() && !page.is_end_of_stream());
    assert_eq!(page.size(), bytes.len());
    assert_eq!(page.header_size(), OGG_HEADER_SIZE + 2);
    assert_eq!(page.packets().collect::<Vec<_>>(), [&b"abc"[..], &b""[..]]);
    assert!(!page.has_unfinished_packet());
}

#[test]
fn rejects_bad_pages() {
    let bytes = page(0, NO_GRANULE_POSITION, 7, 0, &[255], &[1; 255]);

    assert_eq!(OggPage::parse(&bytes).unwrap().granule_position(), None);
    assert!(OggPage::parse(&bytes).unwrap().has_unfinished_packet());

    for length in [0, 3, 20, OGG_HEADER_SIZE, bytes.len() - 1] {
        assert_eq!(
            OggPage::parse(&bytes[..length]),
            Err(Error::Incomplete),
            "{length}"
        );
    }

    let mut corrupt = bytes.clone();
    corrupt[100] ^= 1;
    assert_eq!(OggPage::parse(&corrupt), Err(Error::InvalidData));

    let mut version = bytes.clone();
    version[4] = 1;
    assert_eq!(OggPage::parse(&version), Err(Error::UnsupportedFormat));
    assert_eq!(OggPage::parse(b"RIFF"), Err(Error::InvalidData));
}

#[test]
fn finds_capture_patterns() {
    assert_eq!(OggPage::find(b"xxOxOggSyy"), Some(4));
    assert_eq!(OggPage::find(b"xxxOg"), Some(3));
    assert_eq!(OggPage::find(b"xxxOgx"), None);
    assert_eq!(OggPage::find(b""), None);
}

#[test]
fn reads_packets_spanning_pages() {
    let packets = packets();
    let mut storage = [0; 1024];

    for segments in [1, 2, 3, 255] {
        let stream = paginate(&packets, segments, 9);

        for chunk in [1, 7, stream.len()] {
            let (collected, result) = collect(&stream, chunk, &mut storage);

            assert_eq!(result, Ok(()));
            assert_eq!(collected.lost, 0);
            assert_eq!(
                collected
                    .packets
                    .iter()
                    .map(|(bytes, ..)| bytes.clone())
                    .collect::<Vec<_>>(),
                packets,
                "{segments} segments, chunks of {chunk}"
            );

            let last = collected.packets.last().unwrap();
            assert_eq!((last.1, last.2), (Some(700), true));
            assert!(
                collected.packets[..packets.len() - 1]
                    .iter()
                    .all(|packet| !packet.2)
            );
        }
    }
}

#[test]
fn granule_position_goes_to_the_last_packet_of_a_page() {
    let stream = paginate(&packets(), 4, 9);
    let (collected, _) = collect(&stream, stream.len(), &mut [0; 1024]);
    let granules: Vec<_> = collected.packets.iter().map(|packet| packet.1).collect();

    assert_eq!(
        granules,
        [
            None,
            None,
            Some(200),
            None,
            Some(400),
            Some(500),
            None,
            Some(700)
        ]
    );
}

#[test]
fn skips_other_streams() {
    let packets = packets();
    let first = paginate(&packets, 2, 1);
    let second = paginate(&packets[..3], 2, 2);
    let mut stream = Vec::new();
    let pages = |bytes: &[u8]| {
        let mut pages = Vec::new();
        let mut offset = 0;

        while offset < bytes.len() {
            let size = OggPage::parse(&bytes[offset..]).unwrap().size();
            pages.push(bytes[offset..offset + size].to_vec());
            offset += size;
        }

        pages
    };

    for (index, page) in pages(&first).into_iter().enumerate() {
        stream.extend(page);

        if let Some(other) = pages(&second).get(index) {
            stream.extend(other);
        }
    }

    let (collected, result) = collect(&stream, 100, &mut [0; 1024]);

    assert_eq!(result, Ok(()));
    assert_eq!(collected.lost, 0);
    assert_eq!(collected.packets.len(), packets.len());
}

#[test]
fn reports_junk_and_bad_pages_as_lost() {
    let packets = packets();
    let stream = paginate(&packets, 2, 9);
    let first = OggPage::parse(&stream).unwrap().size();
    let second = first + OggPage::parse(&stream[first..]).unwrap().size();
    let mut junk = stream[..first].to_vec();
    junk.extend_from_slice(b"OgOggSjunk");
    junk.extend_from_slice(&stream[first..]);

    let (collected, result) = collect(&junk, 5, &mut [0; 1024]);

    assert_eq!(result, Ok(()));
    assert_eq!(collected.lost, 1);
    assert_eq!(collected.packets.len(), packets.len());

    let mut corrupt = stream.clone();
    corrupt[second - 1] ^= 0x10;

    let (collected, result) = collect(&corrupt, 64, &mut [0; 1024]);

    assert_eq!(result, Ok(()));
    assert_eq!(collected.lost, 1);
    assert!(collected.packets.len() < packets.len());
    assert_eq!(
        collected.packets.last().unwrap().0,
        packets[packets.len() - 1]
    );
}

#[test]
fn drops_packets_after_a_sequence_gap() {
    let stream = paginate(&packets(), 1, 9);
    let first = OggPage::parse(&stream).unwrap().size();
    let second = OggPage::parse(&stream[first..]).unwrap().size();
    let mut gap = stream[..first].to_vec();
    gap.extend_from_slice(&stream[first + second..]);

    let (collected, result) = collect(&gap, gap.len(), &mut [0; 1024]);

    assert_eq!(result, Ok(()));
    assert_eq!(collected.lost, 1);
    assert_eq!(collected.packets.len(), packets().len() - 1);
}

#[test]
fn rejects_streams_without_a_first_page() {
    let mut reader = OggPacketReader::new();

    assert_eq!(reader.read(b"RIFF....", &mut []), Err(Error::InvalidHeader));
    assert_eq!(reader.read(b"Og", &mut []), Ok(OggRead::default()));

    let mut stream = paginate(&packets(), 2, 9);
    stream[29] ^= 1;

    assert_eq!(reader.read(&stream, &mut []), Err(Error::InvalidData));
}

#[test]
fn skips_packets_larger_than_the_storage() {
    let packets = packets();
    let stream = paginate(&packets, 3, 9);
    let (collected, result) = collect(&stream, 50, &mut [0; 300]);
    let expected: Vec<_> = packets
        .iter()
        .filter(|packet| packet.len() <= 300)
        .cloned()
        .collect();

    assert_eq!(result, Ok(()));
    assert_eq!(
        collected
            .packets
            .iter()
            .map(|(bytes, ..)| bytes.clone())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn resync_skips_to_the_next_page() {
    let packets = packets();
    let stream = paginate(&packets, 2, 9);
    let mut reader = OggPacketReader::new();
    let mut storage = vec![0; 1024];
    let read = reader.read(&stream, &mut storage).unwrap();

    assert!(matches!(
        read.event,
        Some(OggEvent::Page { lost: false, .. })
    ));

    reader.resync();

    let middle = stream.len() / 2;
    let read = reader.read(&stream[middle..], &mut storage).unwrap();

    let Some(OggEvent::Page { page, lost: true }) = read.event else {
        panic!("{read:?}");
    };

    assert_eq!(reader.serial_number(), Some(9));
    assert!(page.sequence_number() > 0);
    assert_eq!(reader.pending(), 0);
}
