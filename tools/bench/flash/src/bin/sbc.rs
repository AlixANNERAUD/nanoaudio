#![no_std]
#![no_main]

use nanoaudio_bench_flash::{decode, halt};
use nanoaudio_sbc::SbcDecoder;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(&mut SbcDecoder::new());
    halt()
}
