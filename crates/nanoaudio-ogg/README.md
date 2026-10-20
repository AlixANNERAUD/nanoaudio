# nanoaudio-ogg

An Ogg page and packet reader. It is `no_std` and does not allocate: packets are copied into a buffer the caller passes, so they can span pages. It checks page CRCs, follows one logical stream and skips the others, and reports granule positions, the end of the stream and lost data.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), where it carries Opus, and can be used on its own:

```toml
[dependencies]
nanoaudio-ogg = "0.1"
nanoaudio-core = "0.1"
```

```rust
use nanoaudio_core::Result;
use nanoaudio_ogg::{OggEvent, OggPacketReader};

fn first_packet_length(input: &[u8]) -> Result<Option<usize>> {
    let mut reader = OggPacketReader::new();
    let mut storage = [0; 4096];
    let mut offset = 0;

    loop {
        let read = reader.read(&input[offset..], &mut storage)?;
        offset += read.consumed;

        match read.event {
            Some(OggEvent::Packet(packet)) => return Ok(Some(packet.length)),
            Some(OggEvent::Page { .. }) => {}
            None => return Ok(None),
        }
    }
}
```

Each page must be whole in the input before it is read, because its CRC is checked first: up to 65,307 bytes, a few kilobytes in most files. `OggPage::parse` reads a single page without copying anything.

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
