#![no_std]
#![no_main]

use core::hint::black_box;
use nanoaudio_bench_flash::{decode, encode, halt};
use nanoaudio_sbc::{SbcDecoder, SbcEncoder, SbcParameters};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(&mut SbcDecoder::new());
    if let Ok(mut encoder) = SbcEncoder::new(black_box(SbcParameters::HIGH_QUALITY)) {
        encode(&mut encoder);
    }
    halt()
}
