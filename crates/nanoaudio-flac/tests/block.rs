use nanoaudio_core::Error;
use nanoaudio_core::codec::Decoder;
use nanoaudio_flac::FlacDecoder;

const MAXIMUM_BLOCK: usize = 4608;

type Narrow = FlacDecoder<i32, MAXIMUM_BLOCK, 2, i16>;
type Wide = FlacDecoder<i32, MAXIMUM_BLOCK, 2>;

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.next() % limit
    }

    fn range(&mut self, low: i64, high: i64) -> i64 {
        low + self.below((high - low + 1) as u64) as i64
    }
}

struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    fn write(&mut self, value: u64, count: u32) {
        for bit in (0..count).rev() {
            if self.used == 8 {
                self.bytes.push(0);
                self.used = 0;
            }

            let last = self.bytes.len() - 1;

            self.bytes[last] |= (((value >> bit) & 1) as u8) << (7 - self.used);
            self.used += 1;
        }
    }

    fn write_signed(&mut self, value: i64, count: u32) {
        self.write(value as u64 & ((1 << count) - 1), count);
    }

    fn write_unary(&mut self, zeros: u64) {
        for _ in 0..zeros {
            self.write(0, 1);
        }

        self.write(1, 1);
    }
}

fn crc_8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |mut crc, byte| {
        crc ^= byte;

        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }

        crc
    })
}

fn crc_16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0, |mut crc, byte| {
        crc ^= u16::from(*byte) << 8;

        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }

        crc
    })
}

fn coded_number(number: u64) -> Vec<u8> {
    if number < 0x80 {
        return vec![number as u8];
    }

    let length = (2..=7u32)
        .find(|length| number < 1 << (7 - length + 6 * (length - 1)))
        .unwrap();
    let mut bytes = vec![(0xFF00u16 >> length) as u8 | (number >> (6 * (length - 1))) as u8];

    for index in (0..length - 1).rev() {
        bytes.push(0x80 | ((number >> (6 * index)) & 0x3F) as u8);
    }

    bytes
}

fn bit_length(value: i64) -> u32 {
    65 - (value ^ (value >> 63)).leading_zeros()
}

fn residual(writer: &mut BitWriter, random: &mut Random, residuals: &[i64], order: usize) {
    let block = residuals.len() + order;
    let method = random.below(2) as u32;
    let parameter_bits = 4 + method;
    let escape = (1u64 << parameter_bits) - 1;
    let orders: Vec<u32> = (0..=8)
        .filter(|partition_order| {
            block.is_multiple_of(1 << partition_order) && block >> partition_order >= order
        })
        .collect();
    let partition_order = orders[random.below(orders.len() as u64) as usize];
    let size = block >> partition_order;

    writer.write(method.into(), 2);
    writer.write(partition_order.into(), 4);

    let mut start = 0;

    for partition in 0..1usize << partition_order {
        let length = if partition == 0 { size - order } else { size };
        let values = &residuals[start..start + length];
        let width = values
            .iter()
            .map(|value| bit_length(*value))
            .max()
            .unwrap_or(0);

        start += length;

        if width <= 31 && random.below(4) == 0 {
            let width = if values.iter().all(|value| *value == 0) && random.below(2) == 0 {
                0
            } else {
                width.max(1)
            };

            writer.write(escape, parameter_bits);
            writer.write(width.into(), 5);

            for value in values {
                writer.write_signed(*value, width);
            }

            continue;
        }

        let parameter = width.saturating_sub(3).min(escape as u32 - 1);

        writer.write(parameter.into(), parameter_bits);

        for value in values {
            let folded = ((value << 1) ^ (value >> 63)) as u64;

            writer.write_unary(folded >> parameter);
            writer.write(folded & ((1 << parameter) - 1), parameter);
        }
    }
}

fn subframe(writer: &mut BitWriter, random: &mut Random, values: &[i64], bits: u32, wasted: u32) {
    let block = values.len();
    let wasted = if random.below(3) == 0 { 0 } else { wasted };
    let bits = bits - wasted;
    let values: Vec<i64> = values.iter().map(|value| value >> wasted).collect();
    let kind = random.below(8);
    let constant = values.iter().all(|value| *value == values[0]);
    let header = |writer: &mut BitWriter, kind: u64| {
        writer.write(kind << 1 | u64::from(wasted > 0), 8);

        if wasted > 0 {
            writer.write_unary((wasted - 1).into());
        }
    };

    if constant && kind < 4 {
        header(writer, 0);
        writer.write_signed(values[0], bits);
    } else if kind == 0 {
        header(writer, 1);

        for value in &values {
            writer.write_signed(*value, bits);
        }
    } else if kind < 4 {
        let order = random.below(5).min(block as u64) as usize;
        let coefficients: &[i64] = [&[][..], &[1], &[2, -1], &[3, -3, 1], &[4, -6, 4, -1]][order];

        header(writer, 8 + order as u64);

        for value in &values[..order] {
            writer.write_signed(*value, bits);
        }

        let residuals: Vec<i64> = (order..block)
            .map(|index| {
                let prediction: i64 = coefficients
                    .iter()
                    .enumerate()
                    .map(|(lag, coefficient)| coefficient * values[index - 1 - lag])
                    .sum();

                values[index] - prediction
            })
            .collect();

        residual(writer, random, &residuals, order);
    } else {
        let order = random.range(1, 32.min(block as i64)) as usize;
        let precision = random.range(1, 15) as u32;
        let shift = (precision - 1 + 32 - (order as u32).leading_zeros()).min(15);
        let limit = (1i64 << (precision - 1)) - 1;
        let coefficients: Vec<i64> = (0..order)
            .map(|_| random.range(-limit - 1, limit))
            .collect();

        header(writer, 31 + order as u64);

        for value in &values[..order] {
            writer.write_signed(*value, bits);
        }

        writer.write((precision - 1).into(), 4);
        writer.write(shift.into(), 5);

        for coefficient in &coefficients {
            writer.write_signed(*coefficient, precision);
        }

        let residuals: Vec<i64> = (order..block)
            .map(|index| {
                let prediction: i64 = coefficients
                    .iter()
                    .enumerate()
                    .map(|(lag, coefficient)| coefficient * values[index - 1 - lag])
                    .sum();

                values[index] - (prediction >> shift)
            })
            .collect();

        residual(writer, random, &residuals, order);
    }
}

fn signal(random: &mut Random, length: usize, bits: u32, wasted: u32) -> Vec<i64> {
    let maximum = (1i64 << (bits - 1)) - 1;
    let minimum = -maximum - 1;
    let step = 1i64 << wasted;
    let mut value = random.range(minimum, maximum);

    (0..length)
        .map(|_| {
            value = match random.below(16) {
                0 => maximum,
                1 => minimum,
                2..=5 => random.range(minimum, maximum),
                _ => (value + random.range(-maximum / 8, maximum / 8)).clamp(minimum, maximum),
            };

            value.div_euclid(step) * step
        })
        .collect()
}

fn encode(random: &mut Random, bits: u32, channels: usize, frames: usize) -> (Vec<u8>, Vec<i32>) {
    let mut bytes = b"fLaC".to_vec();
    let mut expected = Vec::new();
    let mut body = Vec::new();
    let mut total = 0u64;

    for _ in 0..frames {
        let block = random.range(16, MAXIMUM_BLOCK as i64) as usize;
        let wasted = if random.below(3) == 0 {
            random.range(1, bits as i64 - 2) as u32
        } else {
            0
        };
        let signal: Vec<Vec<i64>> = (0..channels)
            .map(|_| {
                if random.below(10) == 0 {
                    vec![
                        random.range(-1 << (bits - 1), (1 << (bits - 1)) - 1) >> wasted << wasted;
                        block
                    ]
                } else {
                    signal(random, block, bits, wasted)
                }
            })
            .collect();
        let assignment = if channels == 2 { random.below(4) } else { 0 };
        let channel_code = match assignment {
            0 => channels as u8 - 1,
            mode => 7 + mode as u8,
        };
        let mut header = vec![0xFF, 0xF9, 0x79, channel_code << 4];

        header.extend(coded_number(total));
        header.extend_from_slice(&(block as u16 - 1).to_be_bytes());
        header.push(crc_8(&header));

        let mut writer = BitWriter {
            bytes: header,
            used: 8,
        };

        if channels == 2 {
            let (left, right) = (&signal[0], &signal[1]);
            let side: Vec<i64> = left.iter().zip(right).map(|(l, r)| l - r).collect();
            let mid: Vec<i64> = left.iter().zip(right).map(|(l, r)| (l + r) >> 1).collect();
            let subframes: [(&[i64], u32); 2] = match assignment {
                0 => [(left, bits), (right, bits)],
                1 => [(left, bits), (&side, bits + 1)],
                2 => [(&side, bits + 1), (right, bits)],
                _ => [(&mid, bits), (&side, bits + 1)],
            };
            let mid_wasted = if assignment == 3 { 0 } else { wasted };

            subframe(
                &mut writer,
                random,
                subframes[0].0,
                subframes[0].1,
                mid_wasted,
            );
            subframe(&mut writer, random, subframes[1].0, subframes[1].1, wasted);
        } else {
            subframe(&mut writer, random, &signal[0], bits, wasted);
        }

        let mut frame = writer.bytes;
        let crc = crc_16(&frame);

        frame.extend_from_slice(&crc.to_be_bytes());
        body.extend(frame);

        for index in 0..block {
            for channel in &signal {
                expected.push((channel[index] << (32 - bits)) as i32);
            }
        }

        total += block as u64;
    }

    bytes.extend_from_slice(&[0x80, 0, 0, 34]);
    bytes.extend_from_slice(&16u16.to_be_bytes());
    bytes.extend_from_slice(&(MAXIMUM_BLOCK as u16).to_be_bytes());
    bytes.extend_from_slice(&[0; 6]);

    let packed =
        (44_100u64 << 44) | ((channels as u64 - 1) << 41) | (u64::from(bits - 1) << 36) | total;

    bytes.extend_from_slice(&packed.to_be_bytes());
    bytes.extend_from_slice(&[0; 16]);
    bytes.extend(body);

    (bytes, expected)
}

fn decode<D: Decoder<Sample = i32>>(
    mut decoder: D,
    file: &[u8],
    chunk: usize,
    output: usize,
) -> Result<Vec<i32>, Error> {
    let mut output = vec![0; output.max(decoder.minimum_output())];
    let mut pending = Vec::new();
    let mut decoded = Vec::new();
    let mut offset = 0;

    loop {
        if offset == file.len() {
            decoder.finish();
        }

        let progress = decoder.decode(&pending, &mut output)?;

        decoded.extend_from_slice(&output[..progress.produced]);
        pending.drain(..progress.consumed);

        if progress.is_stalled() {
            if offset == file.len() {
                return Ok(decoded);
            }

            let end = (offset + chunk).min(file.len());

            pending.extend_from_slice(&file[offset..end]);
            offset = end;
        }
    }
}

#[test]
fn narrow_blocks_match_the_signal() {
    let mut random = Random(0x9E37_79B9_7F4A_7C15);

    for round in 0..48 {
        let bits = [16, 16, 12, 8, 5, 15][round % 6];
        let channels = 1 + usize::from(round % 4 != 0);
        let (file, expected) = encode(&mut random, bits, channels, 6);

        assert_eq!(
            decode(Narrow::new(), &file, file.len(), 1 << 16).unwrap(),
            expected,
            "round {round}"
        );
        assert_eq!(
            decode(Wide::new(), &file, file.len(), 1 << 16).unwrap(),
            expected,
            "round {round}"
        );
        assert_eq!(
            decode(Narrow::new(), &file, 1_000, 3).unwrap(),
            expected,
            "round {round}"
        );
    }
}

#[test]
fn narrow_blocks_reject_deep_streams() {
    let mut random = Random(0x2545_F491_4F6C_DD1D);
    let (file, expected) = encode(&mut random, 17, 2, 2);

    assert_eq!(
        decode(Narrow::new(), &file, file.len(), 1 << 16).err(),
        Some(Error::UnsupportedFormat)
    );
    assert_eq!(
        decode(Wide::new(), &file, file.len(), 1 << 16).unwrap(),
        expected
    );
}

#[test]
fn narrow_blocks_halve_the_buffer() {
    let saved = size_of::<Wide>() - size_of::<Narrow>();

    assert_eq!(saved, MAXIMUM_BLOCK * 2 * 2);
}
