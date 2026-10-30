//! CoD4 1.7's voice chat on the client (docs/research/cod4-voice.md):
//! push to talk on `+talk`, the microphone at 8000 Hz through Speex
//! narrowband, the frames batched out of band to the server, and the
//! server's relayed frames decoded and played, one stream a talker.
//!
//! [`VoiceLink`] is the session's half (no audio): what was heard and
//! when, who is talking, and the outgoing batch on `CL_VoiceTransmit`'s
//! clock. [`VoiceChat`] is the app's: the codec, the microphone and the
//! playback. Speex is opened at run time (`libspeex`), so a build without
//! it still joins and relays; it then neither sends nor plays.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void};
use std::sync::{Arc, Mutex};

use vcod_cod4::net::voice::{self as wire, FRAME_SAMPLES, SAMPLE_RATE, Talked};

/// The session's voice state.
pub struct VoiceLink {
    /// `cl_voice` (default 1): off, nothing heard is kept.
    pub enabled: bool,
    /// Frames encoded since the last batch went (`cl_voiceCommunication`).
    outgoing: Vec<Vec<u8>>,
    /// `voicePacketLastTransmit`, session ms.
    last_transmit: f64,
    /// `s_clientTalkTime`: when each client's last frame came.
    talk_at: [Option<f64>; 64],
    /// Frames heard and not yet played.
    heard: Talked,
}

impl Default for VoiceLink {
    fn default() -> Self {
        VoiceLink {
            enabled: true,
            outgoing: Vec::new(),
            last_transmit: f64::NEG_INFINITY,
            talk_at: [None; 64],
            heard: Vec::new(),
        }
    }
}

impl VoiceLink {
    /// `CL_VoicePacket`: each frame of an unmuted talker, with `cl_voice`
    /// on, stamps the talker (`Voice_IncomingVoiceData`) and waits to be
    /// played.
    pub fn hear(&mut self, packets: Talked, now_ms: f64, muted: impl Fn(u32) -> bool) {
        for (talker, f) in packets {
            if muted(u32::from(talker)) || !self.enabled {
                continue;
            }
            self.talk_at[talker as usize] = Some(now_ms);
            self.heard.push((talker, f));
        }
    }

    /// `Client_SendVoiceData`: one encoded frame on the batch.
    pub fn queue(&mut self, frame: Vec<u8>) {
        if (1..wire::MAX_PACKET).contains(&frame.len()) {
            self.outgoing.push(frame);
        }
    }

    /// `CL_VoiceTransmit` (`iw3mp.exe` 0x46c780): 200 ms past the last
    /// transmit, or ten frames waiting, sends what waits (if anything)
    /// and restarts the clock.
    pub fn due(&mut self, now_ms: f64) -> Option<Vec<Vec<u8>>> {
        if now_ms - self.last_transmit < f64::from(wire::TRANSMIT_MS)
            && self.outgoing.len() < wire::TRANSMIT_PACKETS
        {
            return None;
        }
        self.last_transmit = now_ms;
        (!self.outgoing.is_empty()).then(|| std::mem::take(&mut self.outgoing))
    }

    pub fn take_heard(&mut self) -> Talked {
        std::mem::take(&mut self.heard)
    }

    /// `Voice_IsClientTalking`: a frame in the last 300 ms.
    pub fn talking(&self, client: u32, now_ms: f64) -> bool {
        self.talk_at
            .get(client as usize)
            .copied()
            .flatten()
            .is_some_and(|t| now_ms - t < f64::from(wire::TALKING_MS))
    }

    /// `UI_GetTalkerClientNum`'s order: the talking clients but `own`, by
    /// number.
    pub fn talkers(&self, own: u32, now_ms: f64) -> Vec<u32> {
        (0..64)
            .filter(|&c| c != own && self.talking(c, now_ms))
            .collect()
    }
}

/// `CL_IsPlayerTalking` for the local client: the talk key, when the
/// server's `sv_voice` or our `cl_voice` is on.
pub fn own_talking(talk_held: bool, sv_voice: bool, cl_voice: bool) -> bool {
    talk_held && (sv_voice || cl_voice)
}

/// `Voice_SendVoiceData` (0x46c850) with the talk key's test from
/// `Record_QueueAudioDataForEncoding`: frames are encoded only while the
/// key is held, in the world, with `sv_voice`, `cl_voice` and a `rate` of
/// 5000 or more.
pub fn may_send(talk_held: bool, active: bool, sv_voice: bool, cl_voice: bool, rate: i32) -> bool {
    talk_held && active && sv_voice && cl_voice && rate >= 5000
}

// ---- Speex, opened at run time ------------------------------------------

#[repr(C)]
struct SpeexBits {
    chars: *mut c_char,
    nb_bits: c_int,
    char_ptr: c_int,
    bit_ptr: c_int,
    owner: c_int,
    overflow: c_int,
    buf_size: c_int,
    reserved1: c_int,
    reserved2: *mut c_void,
}

const SPEEX_SET_ENH: c_int = 0;
const SPEEX_GET_FRAME_SIZE: c_int = 3;
const SPEEX_SET_QUALITY: c_int = 4;
#[cfg(test)]
const SPEEX_GET_BITRATE: c_int = 19;
const SPEEX_SET_SAMPLING_RATE: c_int = 24;
const SPEEX_SET_VAD: c_int = 30;
const SPEEX_SET_DTX: c_int = 34;
const SPEEX_MODEID_NB: c_int = 0;

type ModeFn = unsafe extern "C" fn(c_int) -> *const c_void;
type InitFn = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type CtlFn = unsafe extern "C" fn(*mut c_void, c_int, *mut c_void) -> c_int;
type FreeFn = unsafe extern "C" fn(*mut c_void);
type BitsFn = unsafe extern "C" fn(*mut SpeexBits);
type EncodeFn = unsafe extern "C" fn(*mut c_void, *mut i16, *mut SpeexBits) -> c_int;
type WriteFn = unsafe extern "C" fn(*mut SpeexBits, *mut c_char, c_int) -> c_int;
type ReadFn = unsafe extern "C" fn(*mut SpeexBits, *const c_char, c_int);
type DecodeFn = unsafe extern "C" fn(*mut c_void, *mut SpeexBits, *mut i16) -> c_int;

/// One encoder and one decoder, as retail keeps one of each
/// (`g_encoder`, `g_decoder`): narrowband at 8000 Hz, the encoder with
/// VAD and DTX, the decoder with the perceptual enhancer
/// (`Encode_SetOptions` 0x4ec9c0, `Decode_Init` 0x4ec880).
pub struct Speex {
    enc: *mut c_void,
    dec: *mut c_void,
    enc_bits: Box<SpeexBits>,
    dec_bits: Box<SpeexBits>,
    quality: i32,
    ctl_enc: CtlFn,
    encode: EncodeFn,
    write: WriteFn,
    reset: BitsFn,
    read: ReadFn,
    decode: DecodeFn,
    free_enc: FreeFn,
    free_dec: FreeFn,
    free_bits: BitsFn,
    // Last, so the functions above outlive nothing they point into.
    _lib: libloading::Library,
}

const SPEEX_LIBS: &[&str] = &[
    "libspeex.so.1",
    "libspeex.so",
    "libspeex-1.dll",
    "libspeex.dll",
    "speex.dll",
    "libspeex.1.dylib",
    "libspeex.dylib",
];

impl Speex {
    /// The system's `libspeex`, else `None`.
    pub fn open() -> Option<Speex> {
        // SAFETY: loading libspeex runs no initialisers beyond libc's.
        let lib = SPEEX_LIBS
            .iter()
            .find_map(|n| unsafe { libloading::Library::new(n) }.ok())?;
        // SAFETY: every symbol is looked up by its libspeex 1.2 name and
        // typed after `speex.h`; the library stays loaded in `_lib`.
        unsafe {
            fn sym<T: Copy>(lib: &libloading::Library, n: &[u8]) -> Option<T> {
                // SAFETY: the caller types `T` after the symbol.
                unsafe { lib.get::<T>(n).ok().map(|s| *s) }
            }
            let mode: ModeFn = sym(&lib, b"speex_lib_get_mode")?;
            let enc_init: InitFn = sym(&lib, b"speex_encoder_init")?;
            let dec_init: InitFn = sym(&lib, b"speex_decoder_init")?;
            let ctl_enc: CtlFn = sym(&lib, b"speex_encoder_ctl")?;
            let ctl_dec: CtlFn = sym(&lib, b"speex_decoder_ctl")?;
            let bits_init: BitsFn = sym(&lib, b"speex_bits_init")?;
            let nb = mode(SPEEX_MODEID_NB);
            if nb.is_null() {
                return None;
            }
            let enc = enc_init(nb);
            let dec = dec_init(nb);
            if enc.is_null() || dec.is_null() {
                return None;
            }
            let mut enc_bits: Box<SpeexBits> = Box::new(std::mem::zeroed());
            let mut dec_bits: Box<SpeexBits> = Box::new(std::mem::zeroed());
            bits_init(&mut *enc_bits);
            bits_init(&mut *dec_bits);
            let set = |ctl: CtlFn, st, req, mut v: c_int| ctl(st, req, (&raw mut v).cast());
            let rate = SAMPLE_RATE as c_int;
            set(ctl_enc, enc, SPEEX_SET_SAMPLING_RATE, rate);
            set(ctl_enc, enc, SPEEX_SET_QUALITY, wire::DEFAULT_QUALITY);
            set(ctl_enc, enc, SPEEX_SET_VAD, 1);
            set(ctl_enc, enc, SPEEX_SET_DTX, 1);
            set(ctl_dec, dec, SPEEX_SET_ENH, 1);
            set(ctl_dec, dec, SPEEX_SET_SAMPLING_RATE, rate);
            let mut frame: c_int = 0;
            ctl_enc(enc, SPEEX_GET_FRAME_SIZE, (&raw mut frame).cast());
            if frame as usize != FRAME_SAMPLES {
                return None;
            }
            Some(Speex {
                enc,
                dec,
                enc_bits,
                dec_bits,
                quality: wire::DEFAULT_QUALITY,
                ctl_enc,
                encode: sym(&lib, b"speex_encode_int")?,
                write: sym(&lib, b"speex_bits_write")?,
                reset: sym(&lib, b"speex_bits_reset")?,
                read: sym(&lib, b"speex_bits_read_from")?,
                decode: sym(&lib, b"speex_decode_int")?,
                free_enc: sym(&lib, b"speex_encoder_destroy")?,
                free_dec: sym(&lib, b"speex_decoder_destroy")?,
                free_bits: sym(&lib, b"speex_bits_destroy")?,
                _lib: lib,
            })
        }
    }

    /// `Encode_Sample`: one 160-sample frame at `quality`
    /// (`sv_voiceQuality`, set when it changed), its bytes; empty when
    /// DTX sent nothing.
    pub fn encode(&mut self, pcm: &[i16; FRAME_SAMPLES], quality: i32) -> Vec<u8> {
        let mut buf = [0u8; wire::MAX_PACKET];
        let mut pcm = *pcm;
        // SAFETY: the states and bits live as long as `self`; the buffers
        // are the sizes passed.
        unsafe {
            if quality != self.quality {
                self.quality = quality;
                let mut q = quality;
                (self.ctl_enc)(self.enc, SPEEX_SET_QUALITY, (&raw mut q).cast());
            }
            (self.reset)(&mut *self.enc_bits);
            (self.encode)(self.enc, pcm.as_mut_ptr(), &mut *self.enc_bits);
            let n = (self.write)(
                &mut *self.enc_bits,
                buf.as_mut_ptr().cast(),
                (wire::MAX_PACKET - 1) as c_int,
            );
            buf[..n.clamp(0, wire::MAX_PACKET as c_int - 1) as usize].to_vec()
        }
    }

    /// The encoder's bit rate at its current quality, bits a second.
    #[cfg(test)]
    pub fn bitrate(&mut self) -> i32 {
        let mut b: c_int = 0;
        // SAFETY: as in `encode`.
        unsafe { (self.ctl_enc)(self.enc, SPEEX_GET_BITRATE, (&raw mut b).cast()) };
        b
    }

    /// `Voice_IncomingVoiceData` / `Decode_Sample`: one decode per
    /// packet's first 160 bytes (retail reads the packet in
    /// `g_frame_size` byte chunks, each one frame), 160 samples a frame;
    /// a bad frame decodes to nothing.
    pub fn decode(&mut self, packet: &[u8]) -> Vec<i16> {
        let mut out = Vec::new();
        for chunk in packet.chunks(FRAME_SAMPLES) {
            let mut pcm = [0i16; FRAME_SAMPLES];
            // SAFETY: as in `encode`.
            let bad = unsafe {
                (self.read)(
                    &mut *self.dec_bits,
                    chunk.as_ptr().cast(),
                    chunk.len() as c_int,
                );
                (self.decode)(self.dec, &mut *self.dec_bits, pcm.as_mut_ptr())
            };
            if bad == 0 {
                out.extend_from_slice(&pcm);
            }
        }
        out
    }
}

impl Drop for Speex {
    fn drop(&mut self) {
        // SAFETY: each was made in `open` and is freed once.
        unsafe {
            (self.free_enc)(self.enc);
            (self.free_dec)(self.dec);
            (self.free_bits)(&mut *self.enc_bits);
            (self.free_bits)(&mut *self.dec_bits);
        }
    }
}

// ---- the microphone ------------------------------------------------------

/// 8000 Hz mono samples from wherever they come.
pub trait VoiceInput {
    /// Everything captured since the last call.
    fn take(&mut self) -> Vec<i16>;
}

/// A 440 Hz tone at the codec's rate, `ms` of it per call's worth of
/// wall time: the synthetic microphone (`VCOD_VOICE_INPUT=tone`) and the
/// live test's.
pub struct Tone {
    phase: f64,
    last: Option<std::time::Instant>,
}

impl Tone {
    pub fn new() -> Tone {
        Tone {
            phase: 0.0,
            last: None,
        }
    }

    /// `n` samples of the tone.
    pub fn samples(&mut self, n: usize) -> Vec<i16> {
        (0..n)
            .map(|_| {
                self.phase += 440.0 / f64::from(SAMPLE_RATE);
                ((self.phase * std::f64::consts::TAU).sin() * 8000.0) as i16
            })
            .collect()
    }
}

impl Default for Tone {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceInput for Tone {
    fn take(&mut self) -> Vec<i16> {
        let now = std::time::Instant::now();
        let ms = self
            .last
            .map_or(0.0, |t| now.duration_since(t).as_secs_f64() * 1000.0);
        self.last = Some(now);
        self.samples((ms * f64::from(SAMPLE_RATE) / 1000.0) as usize)
    }
}

/// The default input device through cpal, mixed to mono and taken down
/// to 8000 Hz by averaging (retail records at 8000 Hz itself,
/// `DSOUNDRecord_Init`).
pub struct Mic {
    _stream: cpal::Stream,
    buf: Arc<Mutex<VecDeque<i16>>>,
}

impl Mic {
    pub fn open() -> Option<Mic> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let dev = cpal::default_host().default_input_device()?;
        let cfg = dev.default_input_config().ok()?;
        let channels = usize::from(cfg.channels()).max(1);
        let step = f64::from(cfg.sample_rate()) / f64::from(SAMPLE_RATE);
        let buf = Arc::new(Mutex::new(VecDeque::new()));
        let mut down = Downsample::new(channels, step);
        let sink = buf.clone();
        let err = |e| log::warn!("cod4 voice: microphone: {e}");
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => dev.build_input_stream(
                cfg.config(),
                move |d: &[f32], _: &_| down.push(d.iter().copied(), &sink),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => dev.build_input_stream(
                cfg.config(),
                move |d: &[i16], _: &_| down.push(d.iter().map(|&s| f32::from(s) / 32768.0), &sink),
                err,
                None,
            ),
            f => {
                log::warn!("cod4 voice: microphone format {f:?} not read");
                return None;
            }
        }
        .ok()?;
        stream.play().ok()?;
        Some(Mic {
            _stream: stream,
            buf,
        })
    }
}

impl VoiceInput for Mic {
    fn take(&mut self) -> Vec<i16> {
        self.buf
            .lock()
            .map(|mut b| b.drain(..).collect())
            .unwrap_or_default()
    }
}

/// Interleaved device samples to 8000 Hz mono.
struct Downsample {
    channels: usize,
    step: f64,
    acc: f64,
    sum: f32,
    n: u32,
    pos: f64,
}

impl Downsample {
    fn new(channels: usize, step: f64) -> Self {
        Downsample {
            channels,
            step,
            acc: 0.0,
            sum: 0.0,
            n: 0,
            pos: 0.0,
        }
    }

    fn push(&mut self, data: impl Iterator<Item = f32>, sink: &Mutex<VecDeque<i16>>) {
        let mut out = Vec::new();
        let mut frame = 0.0;
        for (i, s) in data.enumerate() {
            frame += s;
            if (i + 1) % self.channels != 0 {
                continue;
            }
            self.sum += frame / self.channels as f32;
            frame = 0.0;
            self.n += 1;
            self.pos += 1.0;
            if self.pos >= self.step + self.acc {
                self.acc += self.step - self.pos;
                self.pos = 0.0;
                let v = self.sum / self.n as f32;
                out.push((v.clamp(-1.0, 1.0) * 32767.0) as i16);
                self.sum = 0.0;
                self.n = 0;
            }
        }
        if let Ok(mut b) = sink.lock() {
            b.extend(out);
            // A second at most: the rest is stale.
            let over = b.len().saturating_sub(SAMPLE_RATE as usize);
            b.drain(..over);
        }
    }
}

// ---- playback ------------------------------------------------------------

/// A talker's decoded samples, which a [`VoiceStream`] plays.
type Ring = Arc<Mutex<VecDeque<f32>>>;

/// A kira sound that plays a talker's ring at 8000 Hz, silent while it is
/// empty, never finishing.
struct VoiceStream {
    ring: Ring,
    pos: f64,
    last: f32,
    next: f32,
}

impl kira::sound::Sound for VoiceStream {
    fn process(&mut self, out: &mut [kira::Frame], dt: f64, _: &kira::info::Info) {
        let mut ring = match self.ring.lock() {
            Ok(r) => r,
            Err(_) => return,
        };
        for f in out {
            self.pos += dt * f64::from(SAMPLE_RATE);
            while self.pos >= 1.0 {
                self.pos -= 1.0;
                self.last = self.next;
                self.next = ring.pop_front().unwrap_or(0.0);
            }
            let s = self.last + (self.next - self.last) * self.pos as f32;
            *f = kira::Frame { left: s, right: s };
        }
    }

    fn finished(&self) -> bool {
        false
    }
}

struct VoiceStreamData(Ring);

impl kira::sound::SoundData for VoiceStreamData {
    type Error = ();
    type Handle = ();

    fn into_sound(self) -> Result<(Box<dyn kira::sound::Sound>, ()), ()> {
        Ok((
            Box::new(VoiceStream {
                ring: self.0,
                pos: 0.0,
                last: 0.0,
                next: 0.0,
            }),
            (),
        ))
    }
}

/// What one frame of [`VoiceChat`] needs off the session.
pub struct VoiceFrame {
    /// The talk key and whether frames may go ([`may_send`]).
    pub sending: bool,
    /// `sv_voiceQuality` off the systeminfo.
    pub quality: i32,
}

/// The app's voice: the codec, the input and a stream a talker.
pub struct VoiceChat {
    speex: Option<Speex>,
    input: Option<Box<dyn VoiceInput>>,
    /// Captured samples not yet a whole frame.
    pending: Vec<i16>,
    streams: [Option<Ring>; 64],
    /// Frames sent and heard, for the debug overlay.
    pub sent: u64,
    pub played: u64,
}

impl VoiceChat {
    /// Speex if the system has it; the input opens on the first talk.
    pub fn new() -> VoiceChat {
        let speex = Speex::open();
        if speex.is_none() {
            log::info!("cod4 voice: no libspeex; voice chat neither sends nor plays");
        }
        VoiceChat::with(speex, None)
    }

    pub fn with(speex: Option<Speex>, input: Option<Box<dyn VoiceInput>>) -> VoiceChat {
        VoiceChat {
            speex,
            input,
            pending: Vec::new(),
            streams: std::array::from_fn(|_| None),
            sent: 0,
            played: 0,
        }
    }

    /// The talk key's frames onto `link` while sending, and the heard
    /// frames decoded onto their talkers' streams, each started on
    /// `audio` when it first speaks.
    pub fn frame(
        &mut self,
        link: &mut VoiceLink,
        f: &VoiceFrame,
        mut audio: Option<&mut crate::audio::AudioSystem>,
    ) {
        if f.sending
            && let Some(speex) = self.speex.as_mut()
        {
            let input = self
                .input
                .get_or_insert_with(|| match std::env::var("VCOD_VOICE_INPUT") {
                    Ok(v) if v == "tone" => Box::new(Tone::new()),
                    _ => match Mic::open() {
                        Some(m) => Box::new(m),
                        None => {
                            log::warn!("cod4 voice: no microphone");
                            Box::new(Silence)
                        }
                    },
                });
            self.pending.extend(input.take());
            while self.pending.len() >= FRAME_SAMPLES {
                let pcm: [i16; FRAME_SAMPLES] = self.pending[..FRAME_SAMPLES].try_into().unwrap();
                self.pending.drain(..FRAME_SAMPLES);
                let bytes = speex.encode(&pcm, f.quality);
                if !bytes.is_empty() {
                    link.queue(bytes);
                    self.sent += 1;
                }
            }
        } else {
            // Not talking: what the input caught meanwhile is dropped.
            if let Some(i) = self.input.as_mut() {
                i.take();
            }
            self.pending.clear();
        }
        for (talker, packet) in link.take_heard() {
            let Some(speex) = self.speex.as_mut() else {
                continue;
            };
            let pcm = speex.decode(&packet);
            self.played += 1;
            let ring = match &self.streams[talker as usize] {
                Some(r) => r.clone(),
                None => {
                    let r: Ring = Arc::new(Mutex::new(VecDeque::new()));
                    if let Some(a) = audio.as_deref_mut() {
                        a.play_custom(VoiceStreamData(r.clone()));
                    }
                    self.streams[talker as usize] = Some(r.clone());
                    r
                }
            };
            if let Ok(mut q) = ring.lock() {
                q.extend(pcm.iter().map(|&s| f32::from(s) / 32768.0));
                let over = q.len().saturating_sub(SAMPLE_RATE as usize / 2);
                q.drain(..over);
            }
        }
    }
}

impl Default for VoiceChat {
    fn default() -> Self {
        Self::new()
    }
}

/// No input device: nothing to send.
struct Silence;

impl VoiceInput for Silence {
    fn take(&mut self) -> Vec<i16> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cod4_voice_transmit_batches_on_200_ms_or_ten_frames() {
        let mut l = VoiceLink::default();
        // The first call sends nothing but starts the clock.
        assert_eq!(l.due(0.0), None);
        l.queue(vec![1]);
        assert_eq!(l.due(100.0), None);
        assert_eq!(l.due(200.0), Some(vec![vec![1]]));
        for _ in 0..10 {
            l.queue(vec![2]);
        }
        assert_eq!(l.due(210.0).map(|b| b.len()), Some(10));
        // Empty and oversized frames never go.
        l.queue(Vec::new());
        l.queue(vec![0; 256]);
        assert_eq!(l.due(1000.0), None);
    }

    #[test]
    fn cod4_voice_talkers_are_the_last_300_ms_unmuted_but_us() {
        let mut l = VoiceLink::default();
        l.hear(
            vec![(5, vec![1]), (2, vec![1]), (7, vec![1])],
            1000.0,
            |c| c == 7,
        );
        assert_eq!(l.talkers(2, 1299.0), vec![5]);
        assert!(l.talkers(2, 1300.0).is_empty());
        assert_eq!(l.take_heard().len(), 2);
        l.enabled = false;
        l.hear(vec![(5, vec![1])], 2000.0, |_| false);
        assert!(!l.talking(5, 2000.0));
    }

    /// With the system's libspeex, a second of the tone at quality 3
    /// (1.7's `sv_voiceQuality`) is 50 frames of 20 bytes, 8000 bit/s, and
    /// decodes back to 160 samples a frame.
    #[test]
    fn cod4_voice_speex_frames_the_tone() {
        let Some(mut s) = Speex::open() else {
            return;
        };
        let mut t = Tone::new();
        let pcm = t.samples(SAMPLE_RATE as usize);
        let frames: Vec<Vec<u8>> = pcm
            .chunks(FRAME_SAMPLES)
            .map(|c| s.encode(c.try_into().unwrap(), 3))
            .collect();
        assert_eq!(frames.len(), 50);
        assert!(
            frames.iter().all(|f| f.len() == 20),
            "{:?}",
            frames[0].len()
        );
        assert_eq!(s.bitrate(), 8000);
        assert_eq!(s.decode(&frames[10]).len(), FRAME_SAMPLES);
    }
}
