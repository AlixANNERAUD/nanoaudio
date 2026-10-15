#![no_std]
#![no_main]

use core::hint::black_box;
use nanoaudio_bench_flash::{decode, encode, halt};
use nanoaudio_g722::{G722BitRate, G722Decoder, G722Encoder};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(&mut G722Decoder::new(black_box(G722BitRate::Rate64000)));
    encode(&mut G722Encoder::new());
    halt()
}
