#![no_std]

use core::hint::black_box;
use core::panic::PanicInfo;

use nanoaudio_core::codec::{Decoder, Encoder};

const INPUT_SIZE: usize = 16 * 1024;
const OUTPUT_SIZE: usize = 4 * 1024;

static mut INPUT: [u8; INPUT_SIZE] = [0; INPUT_SIZE];
static mut SAMPLES: [i16; OUTPUT_SIZE] = [0; OUTPUT_SIZE];
static mut BYTES: [u8; OUTPUT_SIZE] = [0; OUTPUT_SIZE];

pub fn input() -> &'static [u8] {
    let length = black_box(INPUT_SIZE);

    unsafe { core::slice::from_raw_parts(black_box(&raw const INPUT).cast::<u8>(), length) }
}

pub fn samples() -> &'static mut [i16] {
    let length = black_box(OUTPUT_SIZE);

    unsafe { core::slice::from_raw_parts_mut(black_box(&raw mut SAMPLES).cast::<i16>(), length) }
}

pub fn bytes() -> &'static mut [u8] {
    let length = black_box(OUTPUT_SIZE);

    unsafe { core::slice::from_raw_parts_mut(black_box(&raw mut BYTES).cast::<u8>(), length) }
}

pub fn decode<D: Decoder<Sample = i16>>(decoder: &mut D) {
    let input = input();
    let output = samples();

    let _ = black_box(decoder.decode(input, output));
    decoder.finish();
    let _ = black_box(decoder.decode(input, output));
}

pub fn encode<E: Encoder<Sample = i16>>(encoder: &mut E) {
    let input = samples();
    let output = bytes();

    let _ = black_box(encoder.encode(input, output));
    encoder.flush();
    let _ = black_box(encoder.encode(input, output));
}

pub fn halt() -> ! {
    loop {
        black_box(());
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    halt()
}
