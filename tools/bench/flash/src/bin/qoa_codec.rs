#![no_std]
#![no_main]

use core::hint::black_box;
use nanoaudio_bench_flash::{decode, encode, halt};
use nanoaudio_qoa::{QoaDecoder, QoaEncoder};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    decode(&mut QoaDecoder::<i16>::new());
    if let Ok(mut encoder) = QoaEncoder::<i16>::new(black_box(44_100), black_box(2), black_box(0)) {
        encode(&mut encoder);
    }
    halt()
}
