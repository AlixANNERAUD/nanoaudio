use crate::{Error, Result};

/// Most significant bit first reader over a byte slice.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    position: usize,
    cache: u64,
    available: u32,
}

impl<'a> BitReader<'a> {
    /// Creates a reader at the first bit of `bytes`.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            cache: 0,
            available: 0,
        }
    }

    fn refill(&mut self) {
        if let Some(word) = self
            .bytes
            .get(self.position..)
            .and_then(<[u8]>::first_chunk::<8>)
        {
            let consumed = (63 - self.available) / 8;

            self.cache |= u64::from_be_bytes(*word) >> self.available;
            self.available += consumed * 8;
            self.position += consumed as usize;

            return;
        }

        while self.available <= 56 {
            let Some(&byte) = self.bytes.get(self.position) else {
                break;
            };

            self.cache |= u64::from(byte) << (56 - self.available);
            self.available += 8;
            self.position += 1;
        }
    }

    /// Reads `count` bits, at most 32, as an unsigned value.
    pub fn read(&mut self, count: u32) -> Result<u32> {
        debug_assert!(count <= 32);

        if count == 0 {
            return Ok(0);
        }

        if self.available < count {
            self.refill();

            if self.available < count {
                return Err(Error::Incomplete);
            }
        }

        let value = (self.cache >> (64 - count)) as u32;
        self.cache <<= count;
        self.available -= count;

        Ok(value)
    }

    /// Reads `count` bits, at most 32, as a two's complement value.
    pub fn read_signed(&mut self, count: u32) -> Result<i32> {
        if count == 0 {
            return Ok(0);
        }

        let shift = 32 - count;

        Ok(((self.read(count)? << shift) as i32) >> shift)
    }

    /// Counts and consumes zero bits up to and including the next one bit.
    pub fn read_unary(&mut self) -> Result<u32> {
        let mut zeros = 0u32;

        loop {
            if self.available == 0 {
                self.refill();

                if self.available == 0 {
                    return Err(Error::Incomplete);
                }
            }

            let leading = self.cache.leading_zeros();

            if leading < self.available {
                let consumed = leading + 1;

                self.cache = self.cache.checked_shl(consumed).unwrap_or(0);
                self.available -= consumed;

                return Ok(zeros.saturating_add(leading));
            }

            zeros = zeros.saturating_add(self.available);
            self.cache = 0;
            self.available = 0;
        }
    }

    /// Skips to the next byte boundary.
    pub fn align(&mut self) {
        let padding = self.available % 8;

        self.cache <<= padding;
        self.available -= padding;
    }

    /// Returns the number of bytes consumed, counting a partially read byte.
    pub const fn byte_position(&self) -> usize {
        self.position - (self.available / 8) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_most_significant_bit_first() {
        let mut reader =
            BitReader::new(&[0b1010_0110, 0xFF, 0x00, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]);

        assert_eq!(reader.read(0), Ok(0));
        assert_eq!(reader.read(1), Ok(1));
        assert_eq!(reader.read(3), Ok(0b010));
        assert_eq!(reader.read(4), Ok(0b0110));
        assert_eq!(reader.read(12), Ok(0xFF0));
        assert_eq!(reader.read(32), Ok(0x0123_4567));
        assert_eq!(reader.read(12), Ok(0x89A));
        assert_eq!(reader.byte_position(), 8);
        assert_eq!(reader.read(9), Err(Error::Incomplete));
        assert_eq!(reader.read(8), Ok(0xBC));
        assert_eq!(reader.read(1), Err(Error::Incomplete));
    }

    #[test]
    fn reads_signed_values() {
        let mut reader = BitReader::new(&[0b1110_0111, 0xFF, 0xFF, 0xFF, 0xFF]);

        assert_eq!(reader.read_signed(0), Ok(0));
        assert_eq!(reader.read_signed(3), Ok(-1));
        assert_eq!(reader.read_signed(3), Ok(1));
        assert_eq!(reader.read_signed(32), Ok(-1));
    }

    #[test]
    fn reads_unary_values() {
        let mut reader = BitReader::new(&[0b1001_0000, 0, 0, 0, 0, 0, 0, 0, 0, 0b0000_0001]);

        assert_eq!(reader.read_unary(), Ok(0));
        assert_eq!(reader.read_unary(), Ok(2));
        assert_eq!(reader.read_unary(), Ok(75));
        assert_eq!(reader.byte_position(), 10);
        assert_eq!(reader.read_unary(), Err(Error::Incomplete));
    }

    #[test]
    fn matches_bit_by_bit_reading() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut bytes = [0u8; 97];

        bytes.iter_mut().for_each(|byte| *byte = next() as u8);

        let mut reader = BitReader::new(&bytes);
        let mut position = 0;

        loop {
            let count = (next() % 33) as u32;
            let expected = (position..position + count as usize).try_fold(0u32, |value, bit| {
                let byte = bytes.get(bit / 8)?;

                Some((value << 1) | u32::from((byte >> (7 - bit % 8)) & 1))
            });

            match expected {
                Some(expected) => {
                    assert_eq!(reader.read(count), Ok(expected));
                    position += count as usize;
                    assert_eq!(reader.byte_position(), position.div_ceil(8));
                }
                None => {
                    assert_eq!(reader.read(count), Err(Error::Incomplete));
                    break;
                }
            }
        }
    }

    #[test]
    fn aligns_to_bytes() {
        let mut reader = BitReader::new(&[0xFF, 0xA5]);

        assert_eq!(reader.read(3), Ok(0b111));
        assert_eq!(reader.byte_position(), 1);

        reader.align();

        assert_eq!(reader.byte_position(), 1);
        assert_eq!(reader.read(8), Ok(0xA5));

        reader.align();

        assert_eq!(reader.byte_position(), 2);
    }
}
