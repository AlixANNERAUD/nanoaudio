# nanoaudio-buffer

A lock-free single-producer/single-consumer ring buffer that hands out contiguous slices, for DMA, I2S drivers and audio callbacks. It is `no_std`, has no dependency and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-buffer = "0.1"
```

```rust
use nanoaudio_buffer::RingBuffer;

let mut buffer = RingBuffer::<1024>::new();
let (mut producer, mut consumer) = buffer.split();

producer.grant()[..4].copy_from_slice(&[1, 2, 3, 4]);
producer.commit(4);

assert_eq!(consumer.readable(), &[1, 2, 3, 4]);
consumer.release(4);
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
