//! One type over kira's static and streaming handles. They share no trait and
//! their builders take different receivers, so the start chain is a macro.

use kira::sound::static_sound::StaticSoundHandle;
use kira::sound::streaming::StreamingSoundHandle;
use kira::sound::{FromFileError, PlaybackState};
use kira::{Decibels, Panning, PlaybackRate, Tween};

pub(super) enum Handle {
    Static(StaticSoundHandle),
    Stream(StreamingSoundHandle<FromFileError>),
}

impl Handle {
    pub(super) fn state(&self) -> PlaybackState {
        match self {
            Handle::Static(h) => h.state(),
            Handle::Stream(h) => h.state(),
        }
    }

    pub(super) fn set_volume(&mut self, db: f32) {
        let db = gain_db(db);
        match self {
            Handle::Static(h) => h.set_volume(Decibels(db), Tween::default()),
            Handle::Stream(h) => h.set_volume(Decibels(db), Tween::default()),
        }
    }

    pub(super) fn set_panning(&mut self, p: f32) {
        match self {
            Handle::Static(h) => h.set_panning(Panning(p), Tween::default()),
            Handle::Stream(h) => h.set_panning(Panning(p), Tween::default()),
        }
    }

    pub(super) fn set_playback_rate(&mut self, rate: f64) {
        let rate = playback_rate(rate);
        match self {
            Handle::Static(h) => h.set_playback_rate(PlaybackRate(rate), Tween::default()),
            Handle::Stream(h) => h.set_playback_rate(PlaybackRate(rate), Tween::default()),
        }
    }

    /// A dropped kira handle keeps playing; call this before forgetting one.
    pub(super) fn stop(&mut self) {
        self.fade_out(Tween::default());
    }

    /// Stops over `tween`; the state reads `Stopped` once the fade is done.
    pub(super) fn fade_out(&mut self, tween: Tween) {
        match self {
            Handle::Static(h) => h.stop(tween),
            Handle::Stream(h) => h.stop(tween),
        }
    }

    /// Playback position in seconds; a stream's is in output frames, which
    /// `stream::Resampled` keeps at the file's timeline.
    pub(super) fn position(&self) -> f64 {
        match self {
            Handle::Static(h) => h.position(),
            Handle::Stream(h) => h.position(),
        }
    }
}

/// Loudest a voice plays at: room for `q 2 0` doubling a loud alias
/// (retail leaves the fade level unclamped), none for a hostile `q 1000 0`.
pub(super) const MAX_DB: f32 = 12.0;
/// Fastest playback rate a voice takes, three octaves up; csv pitches and
/// UO's packed blend pitch are otherwise unbounded.
pub(super) const MAX_RATE: f64 = 8.0;

/// `db` as kira takes it: -60 (silence) up to [`MAX_DB`], NaN silent.
pub(crate) fn gain_db(db: f32) -> f32 {
    if db.is_nan() {
        -60.0
    } else {
        db.clamp(-60.0, MAX_DB)
    }
}

/// `rate` held to 0..=[`MAX_RATE`]; a non-finite one plays at 1.
pub(crate) fn playback_rate(rate: f64) -> f64 {
    if rate.is_finite() {
        rate.clamp(0.0, MAX_RATE)
    } else {
        1.0
    }
}

/// Applies gain, pan, pitch, start time, start position (seconds) and loop flag to `$data` and starts
/// it as `Handle::$variant`. The error is stringified so both kinds share a
/// type.
macro_rules! start_sound {
    ($manager:expr, $data:expr, $variant:ident, $db:expr, $pan:expr, $pitch:expr, $start:expr, $from_s:expr, $looping:expr) => {{
        let mut data = $data
            .volume(::kira::Decibels($crate::audio::handle::gain_db($db)))
            .panning(::kira::Panning($pan))
            .playback_rate($crate::audio::handle::playback_rate($pitch))
            .start_time($start)
            .start_position($from_s);
        if $looping {
            data = data.loop_region(..);
        }
        $manager
            .play(data)
            .map($crate::audio::handle::Handle::$variant)
            .map_err(|e| e.to_string())
    }};
}

pub(super) use start_sound;
