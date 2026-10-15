#![no_std]
#![no_main]

use core::hint::black_box;
use nanoaudio_bench_flash::{bytes, halt, input};
use nanoaudio_core::Format;
use nanoaudio_dsp::{Stream, Volume, source};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let input = input();
    let (music, notification) = input.split_at(black_box(8_192));

    if let (Ok(music), Ok(notification)) = (
        source(music, Format::I16, 2),
        source(notification, Format::U8, 1),
    ) && let Ok(notification) = notification.to_channels(2)
    {
        let frames = music
            .volume(Volume::from_raw(black_box(3_277)))
            .mix(notification)
            .write_bytes(bytes(), Format::I16);
        black_box(frames);
    }

    halt()
}
