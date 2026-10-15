#![no_std]
#![no_main]

use nanoaudio_bench_flash::{decode, halt};
use nanoaudio_qoa::QoaDecoder;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(&mut QoaDecoder::<i16>::new());
    halt()
}
