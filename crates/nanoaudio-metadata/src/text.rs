use super::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    Latin1,
    Utf16,
    Utf16BigEndian,
    Utf8,
}

impl Encoding {
    pub const fn from_id3(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Encoding::Latin1),
            1 => Some(Encoding::Utf16),
            2 => Some(Encoding::Utf16BigEndian),
            3 => Some(Encoding::Utf8),
            _ => None,
        }
    }

    pub const fn terminator_length(self) -> usize {
        match self {
            Encoding::Latin1 | Encoding::Utf8 => 1,
            Encoding::Utf16 | Encoding::Utf16BigEndian => 2,
        }
    }

    pub(crate) fn find_terminator(self, bytes: &[u8]) -> Option<usize> {
        match self.terminator_length() {
            1 => bytes.iter().position(|byte| *byte == 0),
            _ => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .position(|unit| unit == &[0, 0])
                .map(|index| index * 2),
        }
    }

    pub(crate) fn split_terminated(self, bytes: &[u8]) -> Option<(&[u8], &[u8])> {
        let end = self.find_terminator(bytes)?;

        Some((&bytes[..end], &bytes[end + self.terminator_length()..]))
    }

    pub(crate) fn trim_terminators(self, mut bytes: &[u8]) -> &[u8] {
        let length = self.terminator_length();

        while bytes.len() >= length
            && bytes.len().is_multiple_of(length)
            && bytes[bytes.len() - length..].iter().all(|byte| *byte == 0)
        {
            bytes = &bytes[..bytes.len() - length];
        }

        bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Text<'a> {
    pub encoding: Encoding,
    pub bytes: &'a [u8],
}

impl<'a> Text<'a> {
    pub const fn new(encoding: Encoding, bytes: &'a [u8]) -> Self {
        Self { encoding, bytes }
    }

    pub(crate) fn terminated(encoding: Encoding, bytes: &'a [u8]) -> Self {
        Self::new(encoding, encoding.trim_terminators(bytes))
    }

    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn chars(&self) -> Chars<'a> {
        Chars {
            encoding: self.encoding,
            bytes: self.bytes,
            big_endian: matches!(self.encoding, Encoding::Utf16BigEndian),
            value_start: true,
        }
    }

    pub fn values(&self) -> Values<'a> {
        Values {
            encoding: self.encoding,
            bytes: Some(self.bytes),
        }
    }

    pub fn decoded_length(&self) -> usize {
        self.chars().map(char::len_utf8).sum()
    }

    pub fn decode<'b>(&self, output: &'b mut [u8]) -> Result<&'b str> {
        let mut length = 0;

        for character in self.chars() {
            let size = character.len_utf8();
            let target = output
                .get_mut(length..length + size)
                .ok_or(Error::BufferTooSmall)?;
            character.encode_utf8(target);
            length += size;
        }

        core::str::from_utf8(&output[..length]).map_err(|_| Error::Invalid)
    }

    pub fn eq_str(&self, text: &str) -> bool {
        self.chars().eq(text.chars())
    }
}

#[derive(Debug, Clone)]
pub struct Chars<'a> {
    encoding: Encoding,
    bytes: &'a [u8],
    big_endian: bool,
    value_start: bool,
}

impl Chars<'_> {
    fn unit(&mut self) -> Option<u16> {
        let (unit, rest) = self.bytes.split_first_chunk::<2>()?;
        self.bytes = rest;

        Some(if self.big_endian {
            u16::from_be_bytes(*unit)
        } else {
            u16::from_le_bytes(*unit)
        })
    }

    fn utf16(&mut self) -> Option<char> {
        loop {
            if self.bytes.len() == 1 {
                self.bytes = &[];
                return Some(char::REPLACEMENT_CHARACTER);
            }

            let unit = self.unit()?;

            if self.value_start && self.encoding == Encoding::Utf16 {
                match unit {
                    0xFEFF => {
                        self.value_start = false;
                        continue;
                    }
                    0xFFFE => {
                        self.big_endian = !self.big_endian;
                        self.value_start = false;
                        continue;
                    }
                    _ => {}
                }
            }

            self.value_start = unit == 0;

            if !(0xD800..0xE000).contains(&unit) {
                return char::from_u32(unit as u32);
            }

            if unit >= 0xDC00 {
                return Some(char::REPLACEMENT_CHARACTER);
            }

            let saved = self.bytes;

            match self.unit() {
                Some(low @ 0xDC00..0xE000) => {
                    let value = 0x10000 + ((unit as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);

                    return char::from_u32(value).or(Some(char::REPLACEMENT_CHARACTER));
                }
                _ => {
                    self.bytes = saved;
                    return Some(char::REPLACEMENT_CHARACTER);
                }
            }
        }
    }

    fn utf8(&mut self) -> Option<char> {
        if self.bytes.is_empty() {
            return None;
        }

        let window = &self.bytes[..self.bytes.len().min(4)];
        let (character, length) = match core::str::from_utf8(window) {
            Ok(text) => first(text),
            Err(error) if error.valid_up_to() > 0 => {
                first(core::str::from_utf8(&window[..error.valid_up_to()]).unwrap_or_default())
            }
            Err(error) => (
                char::REPLACEMENT_CHARACTER,
                error.error_len().unwrap_or(window.len()),
            ),
        };

        self.bytes = &self.bytes[length.max(1)..];

        Some(character)
    }
}

fn first(text: &str) -> (char, usize) {
    match text.chars().next() {
        Some(character) => (character, character.len_utf8()),
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self.encoding {
            Encoding::Latin1 => {
                let (byte, rest) = self.bytes.split_first()?;
                self.bytes = rest;

                Some(char::from(*byte))
            }
            Encoding::Utf8 => self.utf8(),
            Encoding::Utf16 | Encoding::Utf16BigEndian => self.utf16(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Values<'a> {
    encoding: Encoding,
    bytes: Option<&'a [u8]>,
}

impl<'a> Iterator for Values<'a> {
    type Item = Text<'a>;

    fn next(&mut self) -> Option<Text<'a>> {
        let bytes = self.bytes?;

        match self.encoding.split_terminated(bytes) {
            Some((value, rest)) => {
                self.bytes = Some(rest);

                Some(Text::new(self.encoding, value))
            }
            None => {
                self.bytes = None;

                Some(Text::new(self.encoding, bytes))
            }
        }
    }
}
