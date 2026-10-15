#![no_std]
#![no_main]

use nanoaudio_bench_flash::{decode, halt};
use nanoaudio_flac::FlacDecoder;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(&mut FlacDecoder::<i16>::new());
    halt()
}
