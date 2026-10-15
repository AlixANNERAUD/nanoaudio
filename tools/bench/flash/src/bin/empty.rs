#![no_std]
#![no_main]

use nanoaudio_bench_flash::halt;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    halt()
}
