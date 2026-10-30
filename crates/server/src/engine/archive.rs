//! The frame archive's ring and its age lookup, what a killcam replays
//! and CoD2's rewind reads (docs/research/cod11-spectator-follow.md 12,
//! docs/research/cod2-killcam.md). Each game keeps its own frame type and
//! time shift: `crate::archive` (CoD1), `crate::game::cod2_archive`.

use std::collections::VecDeque;
use std::rc::Rc;

/// How far back a request reaches, in frames (cod_lnxded 0x808ef09, the
/// `-0x4b0` in cod2's 0x195a06 and 0x195f84).
pub const MAX_FRAMES: i32 = 0x4b0;

/// Every frame archived since the level loaded, the newest last, the last
/// [`MAX_FRAMES`] kept.
pub struct FrameRing<F> {
    frames: VecDeque<Rc<F>>,
    /// Frames archived since the level loaded.
    count: i32,
}

impl<F> Default for FrameRing<F> {
    fn default() -> Self {
        FrameRing {
            frames: VecDeque::new(),
            count: 0,
        }
    }
}

impl<F> FrameRing<F> {
    /// The newest frame, which a push shares unchanged storage with.
    pub fn last(&self) -> Option<&Rc<F>> {
        self.frames.back()
    }

    /// Keeps `frame` as the newest, dropping the oldest past [`MAX_FRAMES`].
    pub fn push(&mut self, frame: F) {
        if self.frames.len() >= MAX_FRAMES as usize {
            self.frames.pop_front();
        }
        self.frames.push_back(Rc::new(frame));
        self.count += 1;
    }

    /// The frame at an absolute index, while it is still kept.
    pub fn frame(&self, index: i32) -> Option<&Rc<F>> {
        let first = self.count - self.frames.len() as i32;
        usize::try_from(index.checked_sub(first)?)
            .ok()
            .and_then(|i| self.frames.get(i))
    }

    /// The lookup both engines make of a positive age with the archive on
    /// (cod_lnxded 0x808eeb8, cod2 0x195a06 and 0x195f84): the frame
    /// `sv_fps * age / 1000` back, clamped to the last [`MAX_FRAMES`] and to
    /// the first frame with the age rewritten to match, then the first
    /// kept frame from there; none sets the age to 0.
    pub fn lookup(&self, age_ms: &mut i32, sv_fps: i32) -> Option<i32> {
        let cur = self.count;
        let back = (i64::from(*age_ms) * i64::from(sv_fps) / 1000).min(i64::from(i32::MAX));
        let mut index = cur.saturating_sub(back as i32);
        let floor = cur - MAX_FRAMES;
        if index < floor {
            *age_ms = (cur - floor) * 1000 / sv_fps;
            index = floor;
        }
        if index < 0 {
            *age_ms = cur * 1000 / sv_fps;
            index = 0;
        }
        let found = (index..cur).find(|&i| self.frame(i).is_some());
        if found.is_none() {
            *age_ms = 0;
        }
        found
    }
}
