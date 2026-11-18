//! `mss_khz` for streamed aliases. Retail opens every stream on the digital
//! driver `mss_khz` picked (CoDMP.exe 0x44cf50), so music, ambients and
//! dialog mix at that rate like loaded sounds (research doc, section 10).
//! kira mixes at the device's rate, so a stream above the mixer rate goes
//! through [`Resampled`], which takes it down chunk by chunk with the same
//! linear interpolation `bank` applies to a loaded file at decode.

use kira::Frame;
use kira::sound::FromFileError;
use kira::sound::streaming::Decoder;
use symphonia::core::audio::{
    Audio, AudioBuffer, GenericAudioBufferRef, conv::FromSample, sample::Sample,
};
use symphonia::core::codecs::{CodecParameters, audio::AudioDecoder};
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo, TrackType, probe::Hint};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::units::Timestamp;

/// Linear interpolation from one rate down to another, fed in chunks.
/// Output frame `i` sits at source position `i * step`, between source
/// frames `k` and `k + 1`; past the last source frame it holds that frame.
pub struct Resampler {
    step: f64,
    /// Source frames not yet passed; `pos` indexes into them.
    pending: Vec<Frame>,
    pos: f64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Resampler {
        Resampler {
            step: f64::from(from) / f64::from(to),
            pending: Vec::new(),
            pos: 0.0,
        }
    }

    /// Takes `src` and appends every output frame whose right neighbour
    /// has arrived.
    pub fn push(&mut self, src: &[Frame], out: &mut Vec<Frame>) {
        self.pending.extend_from_slice(src);
        while (self.pos as usize) + 1 < self.pending.len() {
            out.push(self.at(self.pos as usize));
            self.pos += self.step;
        }
        let k = (self.pos as usize).min(self.pending.len());
        self.pending.drain(..k);
        self.pos -= k as f64;
    }

    /// End of input: the frames left that sit before the last source frame.
    pub fn finish(&mut self, out: &mut Vec<Frame>) {
        while (self.pos as usize) < self.pending.len() {
            out.push(self.at(self.pos as usize));
            self.pos += self.step;
        }
        self.reset();
    }

    fn at(&self, k: usize) -> Frame {
        let last = self.pending.len() - 1;
        let t = (self.pos - k as f64) as f32;
        self.pending[k] * (1.0 - t) + self.pending[(k + 1).min(last)] * t
    }

    fn reset(&mut self) {
        self.pending.clear();
        self.pos = 0.0;
    }
}

/// A streaming decoder taken down to `rate`; one at or below it passes
/// through.
pub struct Resampled<D> {
    inner: D,
    rate: u32,
    resampler: Option<Resampler>,
    done: bool,
}

impl<D: Decoder> Resampled<D> {
    /// `rate` 0 keeps the source's rate.
    pub fn new(inner: D, rate: u32) -> Resampled<D> {
        let from = inner.sample_rate();
        let down = rate != 0 && from > rate;
        Resampled {
            resampler: down.then(|| Resampler::new(from, rate)),
            rate: if down { rate } else { from },
            inner,
            done: false,
        }
    }

    fn step(&self) -> f64 {
        self.resampler.as_ref().map_or(1.0, |r| r.step)
    }
}

impl<D: Decoder> Decoder for Resampled<D> {
    type Error = D::Error;

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn num_frames(&self) -> usize {
        (self.inner.num_frames() as f64 / self.step()).ceil() as usize
    }

    /// kira reads an empty chunk as the end, so this pulls until a chunk
    /// yields output or the source runs dry.
    fn decode(&mut self) -> Result<Vec<Frame>, Self::Error> {
        let Some(r) = self.resampler.as_mut() else {
            return self.inner.decode();
        };
        let mut out = Vec::new();
        while out.is_empty() && !self.done {
            let src = self.inner.decode()?;
            if src.is_empty() {
                r.finish(&mut out);
                self.done = true;
            } else {
                r.push(&src, &mut out);
            }
        }
        Ok(out)
    }

    fn seek(&mut self, index: usize) -> Result<usize, Self::Error> {
        let step = self.step();
        let at = self.inner.seek((index as f64 * step) as usize)?;
        if let Some(r) = self.resampler.as_mut() {
            r.reset();
        }
        self.done = false;
        Ok((at as f64 / step) as usize)
    }
}

/// A stream's file opened for kira at the mixer rate `rate` (0 keeps it).
pub fn open(bytes: Vec<u8>, rate: u32) -> Result<Resampled<FileDecoder>, FromFileError> {
    let d = FileDecoder::new(Box::new(std::io::Cursor::new(bytes)))?;
    Ok(Resampled::new(d, rate))
}

/// The frames `bytes` decode to, read off the default track's packet
/// durations without decoding; `None` when no probe takes the file. Stops
/// counting at the first bad packet, as a decode would.
pub fn frame_count(bytes: Vec<u8>) -> Option<u64> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut reader = symphonia::default::get_probe()
        .probe(
            &Hint::default(),
            mss,
            Default::default(),
            Default::default(),
        )
        .ok()?;
    let track = reader.default_track(TrackType::Audio)?;
    let id = track.id;
    let Some(CodecParameters::Audio(params)) = track.codec_params.as_ref() else {
        return None;
    };
    let rate = u128::from(params.sample_rate?);
    // Packet durations are in the track's time base; frames are at `rate`.
    let (numer, denom) = track.time_base.map_or((1, rate.max(1)), |tb| {
        (u128::from(tb.numer.get()), u128::from(tb.denom.get()))
    });
    let mut dur = 0u128;
    while let Ok(Some(p)) = reader.next_packet() {
        if p.track_id == id {
            dur += u128::from(p.dur.get());
        }
    }
    u64::try_from(dur * numer * rate / denom).ok()
}

/// kira's own symphonia decoder (`kira::sound::streaming::SymphoniaDecoder`,
/// crate-private), so a stream can be wrapped before kira takes it.
pub struct FileDecoder {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    sample_rate: u32,
    num_frames: usize,
    track_id: u32,
}

impl FileDecoder {
    fn new(source: Box<dyn MediaSource>) -> Result<FileDecoder, FromFileError> {
        let mss = MediaSourceStream::new(source, Default::default());
        let format_reader = symphonia::default::get_probe().probe(
            &Hint::default(),
            mss,
            Default::default(),
            Default::default(),
        )?;
        let track = format_reader
            .default_track(TrackType::Audio)
            .ok_or(FromFileError::NoDefaultTrack)?;
        let Some(CodecParameters::Audio(params)) = track.codec_params.as_ref() else {
            return Err(FromFileError::NoDefaultTrack);
        };
        let sample_rate = params.sample_rate.ok_or(FromFileError::UnknownSampleRate)?;
        let num_frames = track.num_frames.ok_or(FromFileError::UnknownDuration)? as usize;
        let decoder =
            symphonia::default::get_codecs().make_audio_decoder(params, &Default::default())?;
        let track_id = track.id;
        Ok(FileDecoder {
            format_reader,
            decoder,
            sample_rate,
            num_frames,
            track_id,
        })
    }
}

impl Decoder for FileDecoder {
    type Error = FromFileError;

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn num_frames(&self) -> usize {
        self.num_frames
    }

    fn decode(&mut self) -> Result<Vec<Frame>, FromFileError> {
        let packet = loop {
            match self.format_reader.next_packet()? {
                Some(p) if p.track_id == self.track_id => break p,
                Some(_) => continue,
                None => return Ok(vec![]),
            }
        };
        frames(&self.decoder.decode(&packet)?)
    }

    fn seek(&mut self, index: usize) -> Result<usize, FromFileError> {
        let seeked = self.format_reader.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts: Timestamp::new(index as i64),
                track_id: self.track_id,
            },
        )?;
        Ok(usize::try_from(seeked.actual_ts.get()).map_or(0, |a| a.min(self.num_frames)))
    }
}

fn frames(buffer: &GenericAudioBufferRef) -> Result<Vec<Frame>, FromFileError> {
    match buffer {
        GenericAudioBufferRef::U8(b) => planes(b),
        GenericAudioBufferRef::U16(b) => planes(b),
        GenericAudioBufferRef::U24(b) => planes(b),
        GenericAudioBufferRef::U32(b) => planes(b),
        GenericAudioBufferRef::S8(b) => planes(b),
        GenericAudioBufferRef::S16(b) => planes(b),
        GenericAudioBufferRef::S24(b) => planes(b),
        GenericAudioBufferRef::S32(b) => planes(b),
        GenericAudioBufferRef::F32(b) => planes(b),
        GenericAudioBufferRef::F64(b) => planes(b),
    }
}

fn planes<S: Sample>(b: &AudioBuffer<S>) -> Result<Vec<Frame>, FromFileError>
where
    f32: FromSample<S>,
{
    use symphonia::core::audio::conv::IntoSample;
    match b.num_planes() {
        1 => Ok(b
            .plane(0)
            .unwrap()
            .iter()
            .map(|s| Frame::from_mono((*s).into_sample()))
            .collect()),
        2 => Ok(b
            .plane(0)
            .unwrap()
            .iter()
            .zip(b.plane(1).unwrap().iter())
            .map(|(l, r)| Frame::new((*l).into_sample(), (*r).into_sample()))
            .collect()),
        _ => Err(FromFileError::UnsupportedChannelConfiguration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `src` in chunks of `chunk` frames.
    struct Chunks {
        src: Vec<Frame>,
        rate: u32,
        at: usize,
        chunk: usize,
    }

    impl Decoder for Chunks {
        type Error = ();
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        fn num_frames(&self) -> usize {
            self.src.len()
        }
        fn decode(&mut self) -> Result<Vec<Frame>, ()> {
            let end = (self.at + self.chunk).min(self.src.len());
            let out = self.src[self.at..end].to_vec();
            self.at = end;
            Ok(out)
        }
        fn seek(&mut self, index: usize) -> Result<usize, ()> {
            self.at = index.min(self.src.len());
            Ok(self.at)
        }
    }

    fn ramp(n: usize) -> Vec<Frame> {
        (0..n).map(|i| Frame::from_mono(i as f32)).collect()
    }

    fn drain<D: Decoder<Error = ()>>(d: &mut D) -> Vec<f32> {
        let mut all = Vec::new();
        loop {
            let c = d.decode().unwrap();
            if c.is_empty() {
                return all;
            }
            all.extend(c.iter().map(|f| f.left));
        }
    }

    #[test]
    fn chunked_stream_matches_the_whole_file() {
        // 44100 -> 11025 on a 1001-frame ramp: every 4th frame, 251 of them
        // (ceil(1001 / 4)), whatever the chunk size.
        let whole: Vec<f32> = (0..251).map(|i| (i * 4) as f32).collect();
        for chunk in [1, 3, 7, 1152, 5000] {
            let src = Chunks {
                src: ramp(1001),
                rate: 44100,
                at: 0,
                chunk,
            };
            let mut d = Resampled::new(src, 11025);
            assert_eq!(d.sample_rate(), 11025);
            assert_eq!(d.num_frames(), 251);
            assert_eq!(drain(&mut d), whole, "chunk {chunk}");
        }
    }

    #[test]
    fn fractional_step_lerps_across_chunks() {
        // 2.5 source frames a step: 0, 2.5, 5, 7.5 on a ramp.
        let src = Chunks {
            src: ramp(9),
            rate: 27563,
            at: 0,
            chunk: 2,
        };
        let mut d = Resampled::new(src, 11025);
        let got = drain(&mut d);
        assert_eq!(got.len(), 4);
        for (g, want) in got.iter().zip([0.0, 2.5, 5.0, 7.5]) {
            assert!((g - want).abs() < 0.01, "{got:?}");
        }
    }

    #[test]
    fn seek_lands_in_output_frames() {
        let src = Chunks {
            src: ramp(400),
            rate: 44100,
            at: 0,
            chunk: 64,
        };
        let mut d = Resampled::new(src, 22050);
        d.decode().unwrap();
        assert_eq!(d.seek(50).unwrap(), 50);
        assert_eq!(d.decode().unwrap()[0].left, 100.0);
    }

    #[test]
    fn retail_mp3_ambient_streams_at_the_mixer_rate() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes =
            crate::audio::bank::SoundBank::read_stream(&fs, "ambient/amb_harbor.mp3").unwrap();
        let native = open(bytes.clone(), 0).unwrap();
        let rate = native.sample_rate();
        let mut low = open(bytes, 11025).unwrap();
        assert_eq!(low.sample_rate(), 11025.min(rate));
        assert!(!low.decode().unwrap().is_empty());
        if rate > 11025 {
            let want = (native.num_frames() as f64 * 11025.0 / f64::from(rate)).ceil();
            assert_eq!(low.num_frames(), want as usize);
        }
    }
}
