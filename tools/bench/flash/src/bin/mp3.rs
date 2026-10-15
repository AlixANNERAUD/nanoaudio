#![no_std]
#![no_main]

use nanoaudio_bench_flash::{decode, halt};
use nanoaudio_mp3::Mp3Decoder;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(&mut Mp3Decoder::<i16>::new());
    halt()
}
