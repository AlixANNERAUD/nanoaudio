# nanoaudio-g711

G.711 A-law and µ-law encoding and decoding, bit-exact with the reference implementation. It is `no_std` and does not allocate.

It is part of [nanoaudio](https://github.com/AlixANNERAUD/nanoaudio#readme), and can be used on its own:

```toml
[dependencies]
nanoaudio-g711 = "0.1"
```

```rust
use nanoaudio_g711::{decode_a_law, encode_a_law};

let code = encode_a_law(1000);

assert_eq!(decode_a_law(code), 1008);
```

## License

Licensed under the [MIT License](https://github.com/AlixANNERAUD/nanoaudio/blob/main/LICENSE).
