//! CoD2 1.3's voice chat (docs/research/cod2-voice.md): CoD4's wire
//! ([`vcod_common::net::voice`]) at CoD2's sample rate.

pub use vcod_common::net::voice::*;

/// The codec's, the microphone's and the playback's rate: 8192 Hz, not
/// Speex narrowband's nominal 8000 (`g_encoder_samplerate` and
/// `g_sound_recordFrequency`, Windows 0x5d88cc and 0x5d88e4, Mac 0x34d7cc
/// and 0x34d7ec; cod2-voice.md 4).
pub const SAMPLE_RATE: u32 = 8192;
/// `sv_voiceQuality`'s registered default (`SV_Init`, Mac 0x141d62), 3 on
/// CoD4.
pub const SV_VOICE_QUALITY: i32 = 1;
