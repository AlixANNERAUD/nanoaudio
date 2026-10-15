#![no_std]
#![no_main]

use core::hint::black_box;
use nanoaudio_bench_flash::{decode, halt};
use nanoaudio_g722::{G722BitRate, G722Decoder};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(&mut G722Decoder::new(black_box(G722BitRate::Rate64000)));
    halt()
}
