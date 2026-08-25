//! One type over kira's static and streaming handles. They share no trait and
//! their builders take different receivers, so the start chain is a macro.

use kira::sound::static_sound::StaticSoundHandle;
use kira::sound::streaming::StreamingSoundHandle;
use kira::sound::{FromFileError, PlaybackState};
use kira::{Decibels, Panning, Tween};

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

    /// A dropped kira handle keeps playing; call this before forgetting one.
    pub(super) fn stop(&mut self) {
        match self {
            Handle::Static(h) => h.stop(Tween::default()),
            Handle::Stream(h) => h.stop(Tween::default()),
        }
    }
}

/// Applies gain, pan, pitch, start time and loop flag to `$data` and starts
/// it as `Handle::$variant`. The error is stringified so both kinds share a
/// type.
macro_rules! start_sound {
    ($manager:expr, $data:expr, $variant:ident, $db:expr, $pan:expr, $pitch:expr, $start:expr, $looping:expr) => {{
        let mut data = $data
            .volume(::kira::Decibels($db))
            .panning(::kira::Panning($pan))
            .playback_rate($pitch)
            .start_time($start);
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
