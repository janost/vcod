//! CoD4 1.7's voice chat (docs/research/cod4-voice.md): the wire CoD2
//! shares ([`vcod_common::net::voice`]) at CoD4's sample rate.

pub use vcod_common::net::voice::*;

/// The codec's rate (`g_encoder_samplerate` 8000 at `iw3mp.exe` 0x724ed4).
pub const SAMPLE_RATE: u32 = 8000;
