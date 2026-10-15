#![no_std]
#![no_main]

use core::hint::black_box;
use nanoaudio_adpcm::ImaAdpcm;
use nanoaudio_bench_flash::{decode, halt};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    if let Ok(mut codec) = ImaAdpcm::new(black_box(16_000), black_box(1), black_box(256)) {
        decode(&mut codec);
    }
    halt()
}
