#![no_std]
#![no_main]

use nanoaudio_bench_flash::{decode, halt};
use nanoaudio_opus::OpusDecoder;

static mut DECODER: OpusDecoder<i16, 960> = OpusDecoder::new();

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(unsafe { &mut *(&raw mut DECODER) });
    halt()
}
