#![no_std]
#![no_main]

use core::hint::black_box;
use nanoaudio_bench_flash::{decode, encode, halt};
use nanoaudio_g711::{G711, Law};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    if let Ok(mut codec) = G711::new(black_box(Law::Mu), black_box(8_000), black_box(1)) {
        decode(&mut codec);
        encode(&mut codec);
    }
    halt()
}
