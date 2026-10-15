/// Non-reflected CRC-8 with the given polynomial, no final XOR.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Crc8<const POLYNOMIAL: u8>;

impl<const POLYNOMIAL: u8> Crc8<POLYNOMIAL> {
    const TABLE: [u8; 256] = {
        let mut table = [0; 256];
        let mut index = 0;

        while index < 256 {
            table[index] = Self::update_bits(0, index as u8, 8);
            index += 1;
        }

        table
    };

    /// Updates `crc` with `bytes` using a lookup table.
    pub fn update(crc: u8, bytes: &[u8]) -> u8 {
        bytes
            .iter()
            .fold(crc, |crc, byte| Self::TABLE[(crc ^ byte) as usize])
    }

    /// Updates `crc` with the `count` most significant bits of `byte`, at most 8, without a table.
    pub const fn update_bits(crc: u8, byte: u8, count: u32) -> u8 {
        let mut crc = crc ^ (byte & (0xFF00u16 >> count) as u8);
        let mut bit = 0;

        while bit < count {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ POLYNOMIAL
            } else {
                crc << 1
            };
            bit += 1;
        }

        crc
    }
}

/// Non-reflected CRC-16 with the given polynomial, no final XOR.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Crc16<const POLYNOMIAL: u16>;

impl<const POLYNOMIAL: u16> Crc16<POLYNOMIAL> {
    const TABLE: [u16; 256] = {
        let mut table = [0; 256];
        let mut index = 0;

        while index < 256 {
            let mut value = (index as u16) << 8;
            let mut bit = 0;

            while bit < 8 {
                value = if value & 0x8000 != 0 {
                    (value << 1) ^ POLYNOMIAL
                } else {
                    value << 1
                };
                bit += 1;
            }

            table[index] = value;
            index += 1;
        }

        table
    };

    /// Updates `crc` with `bytes` using a lookup table.
    pub fn update(crc: u16, bytes: &[u8]) -> u16 {
        bytes.iter().fold(crc, |crc, byte| {
            (crc << 8) ^ Self::TABLE[((crc >> 8) as u8 ^ byte) as usize]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECK: &[u8] = b"123456789";

    #[test]
    fn crc_8_matches_reference_check_values() {
        assert_eq!(Crc8::<0x07>::update(0, CHECK), 0xF4);
        assert_eq!(Crc8::<0x1D>::update(0xFF, CHECK) ^ 0xFF, 0x4B);
    }

    #[test]
    fn crc_16_matches_reference_check_values() {
        assert_eq!(Crc16::<0x8005>::update(0, CHECK), 0xFEE8);
        assert_eq!(Crc16::<0x1021>::update(0xFFFF, CHECK), 0x29B1);
    }

    #[test]
    fn bitwise_crc_8_matches_table() {
        let crc = CHECK
            .iter()
            .fold(0x0F, |crc, byte| Crc8::<0x1D>::update_bits(crc, *byte, 8));

        assert_eq!(crc, Crc8::<0x1D>::update(0x0F, CHECK));
    }

    #[test]
    fn bitwise_crc_8_handles_partial_bytes() {
        let mut crc = 0x0F;

        for bit in 0..8 {
            crc = Crc8::<0x1D>::update_bits(crc, 0xA5 << bit, 1);
        }

        assert_eq!(crc, Crc8::<0x1D>::update_bits(0x0F, 0xA5, 8));
        assert_eq!(Crc8::<0x1D>::update_bits(0x42, 0xFF, 0), 0x42);
        assert_eq!(
            Crc8::<0x1D>::update_bits(Crc8::<0x1D>::update_bits(0x0F, 0xA5, 4), 0x50, 4),
            Crc8::<0x1D>::update_bits(0x0F, 0xA5, 8)
        );
    }
}
