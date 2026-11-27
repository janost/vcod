//! The voice options page's record level meter (`UI_OwnerDraw` 265,
//! cod4-voice.md 4.1): retail averages the mean absolute sample of the
//! last six capture batches and draws a 1 unit white frame with a bar
//! inside. Retail records all the time; vcod opens the microphone while an
//! item is painted and closes it after [`IDLE`] without one.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use crate::voice::{Mic, MicGain, VoiceInput};

/// The owner draw of the voice page's `voicechat_level_indicator`.
pub const UI_RECORDLEVEL: i32 = 265;
/// Draws averaged (0x57ad60: the level over 6.0, six slots).
const WINDOW: usize = 6;
/// How long the microphone stays open past the last paint.
const IDLE: Duration = Duration::from_millis(500);

/// A capture batch's level: the mean absolute sample
/// (`Record_QueueAudioDataForEncoding`, 0x4ed120).
pub fn batch_level(pcm: &[i16]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    pcm.iter().map(|&s| f32::from(s).abs()).sum::<f32>() / pcm.len() as f32
}

/// The six slots of 0x57ad60.
#[derive(Default)]
struct Ring {
    slots: [f32; WINDOW],
    n: usize,
}

impl Ring {
    /// Stores `level / 32767 / 6` in the next slot and returns the six's
    /// sum, 0..1 for a full-scale signal.
    fn push(&mut self, level: f32) -> f32 {
        self.slots[self.n % WINDOW] = level / 32767.0 / WINDOW as f32;
        self.n += 1;
        self.slots.iter().sum()
    }
}

/// The meter's rects in the item's units for a `level` of 0..1: the bar
/// when it is wider than 0, then the frame's top, bottom, left and right
/// lines (0x5443a0). All draw white.
pub fn rects(level: f32, [x, y, w, h]: [f32; 4]) -> Vec<[f32; 4]> {
    let bar = level * w - 4.0;
    let mut out = Vec::with_capacity(5);
    if level > 0.0 && bar > 0.0 {
        out.push([x + 2.0, y + 2.0, bar, h - 4.0]);
    }
    out.extend([
        [x, y, w, 1.0],
        [x, y + h - 1.0, w, 1.0],
        [x, y + 1.0, 1.0, h - 2.0],
        [x + w - 1.0, y + 1.0, 1.0, h - 2.0],
    ]);
    out
}

struct Meter {
    /// `None` when no input device opened; not retried until the meter closes.
    mic: Option<Mic>,
    /// The last non-empty batch's level, kept across draws with none.
    last: f32,
    ring: Ring,
    painted: Instant,
}

thread_local! {
    static METER: RefCell<Option<Meter>> = const { RefCell::new(None) };
}

/// The meter's level for this draw: the microphone (opened at `rate` on
/// the first call) read through `gain`, folded into the window.
pub fn level(rate: u32, gain: MicGain) -> f32 {
    METER.with_borrow_mut(|m| {
        let m = m.get_or_insert_with(|| Meter {
            mic: Mic::open(rate),
            last: 0.0,
            ring: Ring::default(),
            painted: Instant::now(),
        });
        m.painted = Instant::now();
        if let Some(mic) = m.mic.as_mut() {
            let pcm: Vec<i16> = mic.take().into_iter().map(|s| gain.apply(s)).collect();
            if !pcm.is_empty() {
                m.last = batch_level(&pcm);
            }
        }
        m.ring.push(m.last)
    })
}

/// Closes the microphone once no meter has drawn for [`IDLE`]; once a frame.
pub fn close_idle() {
    METER.with_borrow_mut(|m| {
        if m.as_ref().is_some_and(|m| m.painted.elapsed() > IDLE) {
            *m = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_is_the_mean_of_the_last_six_batches_over_full_scale() {
        let mut r = Ring::default();
        let mut out = 0.0;
        for _ in 0..6 {
            out = r.push(32767.0);
        }
        assert!((out - 1.0).abs() < 1e-6);
        // A seventh, silent batch replaces the oldest.
        assert!((r.push(0.0) - 5.0 / 6.0).abs() < 1e-6);
        assert_eq!(
            batch_level(&[100, -300, i16::MIN, 0]),
            (100.0 + 300.0 + 32768.0) / 4.0
        );
    }

    #[test]
    fn the_bar_starts_past_the_frame_and_stops_at_the_level() {
        let item = [446.0, 106.0, 100.0, 10.0];
        let r = rects(0.5, item);
        assert_eq!(r[0], [448.0, 108.0, 46.0, 6.0]);
        assert_eq!(r.len(), 5);
        assert_eq!(r[2], [446.0, 115.0, 100.0, 1.0]);
        assert_eq!(r[4], [545.0, 107.0, 1.0, 8.0]);
        // Under the frame's own 4 units there is no bar, only the frame.
        assert_eq!(rects(0.03, item).len(), 4);
        assert_eq!(rects(0.0, item).len(), 4);
    }
}
