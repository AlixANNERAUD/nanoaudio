use std::mem::size_of;
use std::path::Path;

use nanoaudio_adpcm::{ImaAdpcm, MsAdpcm};
use nanoaudio_core::codec::{Decoder, Encoder};
use nanoaudio_flac::{FlacBlockSample, FlacDecoder};
use nanoaudio_g711::{G711, Law};
use nanoaudio_g722::{G722BitRate, G722Decoder, G722Encoder};
use nanoaudio_mp3::Mp3Decoder;
use nanoaudio_ogg::{OGG_MAXIMUM_PAGE_SIZE, OggPage};
use nanoaudio_opus::OpusDecoder;
use nanoaudio_qoa::{QoaDecoder, QoaEncoder};
use nanoaudio_sbc::{SbcDecoder, SbcEncoder, SbcParameters};

pub struct Entry {
    pub item: &'static str,
    pub direction: &'static str,
    pub configuration: String,
    pub state: usize,
    pub input: Option<usize>,
    pub output: Option<usize>,
}

fn entry(
    item: &'static str,
    direction: &'static str,
    configuration: &str,
    state: usize,
    input: Option<usize>,
    output: Option<usize>,
) -> Entry {
    Entry {
        item,
        direction,
        configuration: configuration.to_string(),
        state,
        input,
        output,
    }
}

fn decoder<D: Decoder<Sample = i16>>(
    item: &'static str,
    configuration: &str,
    decoder: &D,
    input: usize,
) -> Entry {
    entry(
        item,
        "decode",
        configuration,
        size_of::<D>(),
        Some(input),
        Some(decoder.minimum_output() * 2),
    )
}

fn encoder<E: Encoder<Sample = i16>>(
    item: &'static str,
    configuration: &str,
    encoder: &E,
) -> Entry {
    entry(
        item,
        "encode",
        configuration,
        size_of::<E>(),
        Some(encoder.minimum_input() * 2),
        Some(encoder.minimum_output()),
    )
}

fn flac<const BLOCK: usize, const CHANNELS: usize, B: FlacBlockSample>(
    file: Option<&[u8]>,
) -> Entry {
    let mut decoder = Box::new(FlacDecoder::<i16, BLOCK, CHANNELS, B>::new());
    let mut input = None;

    if let Some(file) = file.filter(|_| BLOCK >= 4_096 && CHANNELS >= 2) {
        let mut output = vec![0i16; 8_192];

        if decoder
            .decode(&file[..file.len().min(8_192)], &mut output)
            .is_ok()
        {
            input = Some(decoder.minimum_input());
        }
    }

    entry(
        "FLAC",
        "decode",
        &format!(
            "up to {BLOCK}-sample blocks, {CHANNELS} channel{}{}",
            if CHANNELS == 1 { "" } else { "s" },
            if size_of::<B>() == 2 {
                ", i16 blocks"
            } else {
                ""
            }
        ),
        size_of::<FlacDecoder<i16, BLOCK, CHANNELS, B>>(),
        input,
        Some(CHANNELS * 2),
    )
}

fn largest_page(file: Option<&[u8]>) -> usize {
    let Some(mut file) = file else {
        return OGG_MAXIMUM_PAGE_SIZE;
    };
    let mut largest = 0;

    while let Ok(page) = OggPage::parse(file) {
        largest = largest.max(page.size());
        file = &file[page.size()..];
    }

    largest
}

pub fn entries(data: Option<&Path>) -> Vec<Entry> {
    let flac_file = data.and_then(|data| std::fs::read(data.join("signal.flac")).ok());
    let flac_file = flac_file.as_deref();
    let g711 = G711::new(Law::Mu, 8_000, 1).unwrap();
    let ima_mono = ImaAdpcm::new(16_000, 1, 256).unwrap();
    let ima_stereo = ImaAdpcm::new(44_100, 2, 2_048).unwrap();
    let ima_stereo_limited = ImaAdpcm::<2>::new_limited(44_100, 2, 2_048).unwrap();
    let ms_stereo = MsAdpcm::new(44_100, 2, 2_048).unwrap();
    let qoa_decoder = QoaDecoder::<i16>::new();
    let qoa_stereo_decoder = QoaDecoder::<i16, 2>::new_limited();
    let qoa_mono = QoaEncoder::<i16>::new(16_000, 1, 64_000).unwrap();
    let qoa_stereo = QoaEncoder::<i16>::new(44_100, 2, 176_400).unwrap();
    let qoa_stereo_limited = QoaEncoder::<i16, 2>::new_limited(44_100, 2, 176_400).unwrap();
    let g722_decoder = G722Decoder::new(G722BitRate::Rate64000);
    let g722_encoder = G722Encoder::new();
    let sbc_decoder = SbcDecoder::new();
    let msbc = SbcEncoder::new(SbcParameters::MSBC).unwrap();
    let sbc = SbcEncoder::new(SbcParameters::HIGH_QUALITY).unwrap();
    let mp3 = Mp3Decoder::<i16>::new();
    let opus_file = data.and_then(|data| std::fs::read(data.join("signal.opus")).ok());
    let opus_page = largest_page(opus_file.as_deref());
    let opus = Box::new(OpusDecoder::<i16>::new());
    let opus_short = Box::new(OpusDecoder::<i16, 960>::new());

    vec![
        decoder("G.711", "8 kHz mono", &g711, 1),
        encoder("G.711", "8 kHz mono", &g711),
        decoder("IMA ADPCM", "16 kHz mono, 256-byte blocks", &ima_mono, 256),
        encoder("IMA ADPCM", "16 kHz mono, 256-byte blocks", &ima_mono),
        decoder(
            "IMA ADPCM",
            "44.1 kHz stereo, 2048-byte blocks",
            &ima_stereo,
            2_048,
        ),
        encoder(
            "IMA ADPCM",
            "44.1 kHz stereo, 2048-byte blocks",
            &ima_stereo,
        ),
        decoder(
            "IMA ADPCM",
            "44.1 kHz stereo, 2048-byte blocks, up to 2 channels",
            &ima_stereo_limited,
            2_048,
        ),
        decoder(
            "MS ADPCM",
            "44.1 kHz stereo, 2048-byte blocks",
            &ms_stereo,
            2_048,
        ),
        encoder("MS ADPCM", "44.1 kHz stereo, 2048-byte blocks", &ms_stereo),
        decoder("QOA", "any, up to 8 channels", &qoa_decoder, 8 + 16 * 2),
        decoder(
            "QOA",
            "any, up to 2 channels",
            &qoa_stereo_decoder,
            8 + 16 * 2,
        ),
        encoder("QOA", "16 kHz mono", &qoa_mono),
        encoder("QOA", "44.1 kHz stereo", &qoa_stereo),
        encoder(
            "QOA",
            "44.1 kHz stereo, up to 2 channels",
            &qoa_stereo_limited,
        ),
        flac::<1_152, 2, i32>(flac_file),
        flac::<4_096, 1, i32>(flac_file),
        flac::<4_096, 2, i32>(flac_file),
        flac::<4_096, 2, i16>(flac_file),
        flac::<4_608, 2, i32>(flac_file),
        flac::<4_608, 2, i16>(flac_file),
        flac::<4_608, 8, i32>(flac_file),
        decoder("G.722", "16 kHz mono", &g722_decoder, 1),
        encoder("G.722", "16 kHz mono", &g722_encoder),
        decoder(
            "SBC",
            "mSBC",
            &sbc_decoder,
            SbcParameters::MSBC.frame_length(),
        ),
        decoder(
            "SBC",
            "44.1 kHz joint stereo, bitpool 53",
            &sbc_decoder,
            SbcParameters::HIGH_QUALITY.frame_length(),
        ),
        encoder("SBC", "mSBC", &msbc),
        encoder("SBC", "44.1 kHz joint stereo, bitpool 53", &sbc),
        decoder("MP3", "default input window", &mp3, mp3.minimum_input()),
        decoder("Opus", "packets up to 120 ms", &*opus, opus_page),
        decoder("Opus", "packets up to 20 ms", &*opus_short, opus_page),
    ]
}
