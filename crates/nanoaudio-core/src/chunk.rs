use crate::{Error, Result};

const HEADER_SIZE: usize = 8;

/// The byte order of chunk sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ByteOrder {
    /// Little-endian sizes, as in RIFF files such as WAV.
    LittleEndian,
    /// Big-endian sizes, as in IFF files such as AIFF.
    BigEndian,
}

/// A chunk header read by [`Chunks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chunk<'a> {
    /// The four-character identifier, such as `fmt ` or `COMM`.
    pub identifier: &'a [u8; 4],
    /// Size in bytes of the body, as declared by the header, padding excluded.
    pub size: u32,
    /// Offset in bytes of the first byte of the body, from the start of the buffer.
    pub offset: usize,
    rest: &'a [u8],
}

impl<'a> Chunk<'a> {
    /// Returns the first `length` bytes after the header, which can run past the end of the chunk, or [`Error::Incomplete`] when the buffer is shorter.
    #[inline]
    pub fn body(&self, length: usize) -> Result<&'a [u8]> {
        self.rest.get(..length).ok_or(Error::Incomplete)
    }
}

/// Walks the chunks of a RIFF or IFF buffer one header at a time, skipping the padding byte after odd sizes.
///
/// The buffer does not need to hold the whole file: reading a header past its end gives [`Error::Incomplete`].
///
/// # Examples
///
/// ```
/// use nanoaudio_core::Error;
/// use nanoaudio_core::chunk::{ByteOrder, Chunks};
///
/// let bytes = b"abcd\x03\0\0\0xyz\0efgh\0\0\0\0";
/// let mut chunks = Chunks::new(bytes, 0, ByteOrder::LittleEndian);
///
/// let chunk = chunks.next_chunk().unwrap();
/// assert_eq!(chunk.identifier, b"abcd");
/// assert_eq!(chunk.body(3), Ok(&b"xyz"[..]));
/// assert_eq!(chunks.next_chunk().unwrap().identifier, b"efgh");
/// assert_eq!(chunks.next_chunk(), Err(Error::Incomplete));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chunks<'a> {
    bytes: &'a [u8],
    body: usize,
    size: u32,
    order: ByteOrder,
}

impl<'a> Chunks<'a> {
    /// Creates a walker whose first chunk header is at `offset` in `bytes`.
    #[inline]
    pub const fn new(bytes: &'a [u8], offset: usize, order: ByteOrder) -> Self {
        Self {
            bytes,
            body: offset,
            size: 0,
            order,
        }
    }

    /// Returns the offset of the next chunk header, or `None` when the last chunk read ends past `usize::MAX`.
    #[inline]
    pub fn position(&self) -> Option<usize> {
        let size = self.size as usize;

        size.checked_add(size & 1)
            .and_then(|padded| self.body.checked_add(padded))
    }

    /// Reads the next chunk header and moves past the chunk.
    ///
    /// # Errors
    ///
    /// [`Error::Incomplete`] when `bytes` ends before the header, and [`Error::InvalidHeader`] when the previous chunk ends past `usize::MAX`.
    #[inline]
    pub fn next_chunk(&mut self) -> Result<Chunk<'a>> {
        let offset = self.position().ok_or(Error::InvalidHeader)?;
        let (header, rest) = self
            .bytes
            .get(offset..)
            .and_then(<[u8]>::split_first_chunk::<HEADER_SIZE>)
            .ok_or(Error::Incomplete)?;
        let (identifier, size) = header.split_first_chunk::<4>().ok_or(Error::Incomplete)?;
        let size = [size[0], size[1], size[2], size[3]];
        let size = match self.order {
            ByteOrder::LittleEndian => u32::from_le_bytes(size),
            ByteOrder::BigEndian => u32::from_be_bytes(size),
        };

        self.body = offset + HEADER_SIZE;
        self.size = size;

        Ok(Chunk {
            identifier,
            size,
            offset: self.body,
            rest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_both_byte_orders() {
        let little = b"abcd\x01\0\0\0x\0efgh\0\0\0\0";
        let big = b"abcd\0\0\0\x01x\0efgh\0\0\0\0";

        for (bytes, order) in [
            (&little[..], ByteOrder::LittleEndian),
            (&big[..], ByteOrder::BigEndian),
        ] {
            let mut chunks = Chunks::new(bytes, 0, order);
            let first = chunks.next_chunk().unwrap();

            assert_eq!(first.identifier, b"abcd");
            assert_eq!(first.size, 1);
            assert_eq!(first.offset, 8);
            assert_eq!(first.body(1), Ok(&b"x"[..]));

            let second = chunks.next_chunk().unwrap();

            assert_eq!(second.identifier, b"efgh");
            assert_eq!(second.offset, 18);
            assert_eq!(chunks.position(), Some(18));
            assert_eq!(second.body(0), Ok(&[][..]));
            assert_eq!(second.body(1), Err(Error::Incomplete));
            assert_eq!(chunks.next_chunk(), Err(Error::Incomplete));
        }
    }

    #[test]
    fn incomplete_headers() {
        let bytes = b"abcd\0\0\0\0";

        for length in 0..bytes.len() {
            let mut chunks = Chunks::new(&bytes[..length], 0, ByteOrder::LittleEndian);

            assert_eq!(chunks.next_chunk(), Err(Error::Incomplete));
        }

        let mut chunks = Chunks::new(bytes, 9, ByteOrder::LittleEndian);
        assert_eq!(chunks.next_chunk(), Err(Error::Incomplete));
    }

    #[test]
    fn overflowing_sizes() {
        let bytes = b"abcd\xff\xff\xff\xff";
        let mut chunks = Chunks::new(bytes, 0, ByteOrder::LittleEndian);

        assert_eq!(chunks.next_chunk().unwrap().size, u32::MAX);
        assert_eq!(chunks.position().is_none(), usize::BITS <= 32);

        let expected = if usize::BITS > 32 {
            Error::Incomplete
        } else {
            Error::InvalidHeader
        };
        assert_eq!(chunks.next_chunk(), Err(expected));
    }
}
