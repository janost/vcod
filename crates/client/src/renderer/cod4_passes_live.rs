//! The passes, copies and uploads of a CoD4 frame (cod4-client-perf.md
//! 24): vcod-server's war on mp_crossfire with us joined, the session's
//! renderer at 1920 x 1080 and one sample, at CoD4's defaults (the
//! depth prepass on, the sun shadow on, the map's vision set), the eye
//! held at the census views of `cod4_draws_live`. Per view it prints the
//! frame's [`FrameWrite`] log and a hash of the frame, and holds the
//! frame still across three draws. Needs `COD4_DIR` and an adapter with
//! BC textures (lavapipe will do).

use super::cod4_worldfx_live::session_sized;
use super::*;
use glam::Vec3;
use std::hash::{Hash, Hasher};

/// `cod4_perf.sh`'s held view and the census's median one.
const VIEWS: [[f32; 5]; 2] = [
    [5232.0, -888.0, 240.0, 225.0, 0.0],
    [3990.0, -1110.0, 60.0, 180.0, 0.0],
];

#[test]
fn cod4_live_crossfire_frame_writes() {
    use std::sync::{Arc, Mutex};
    let rows = Arc::new(Mutex::new(Vec::new()));
    let out = rows.clone();
    let mut at = 0;
    let ran = session_sized("mp_crossfire", &[], (1, (1920, 1080)), move |l| {
        if l.t < 2.0 {
            return false;
        }
        let Some(v) = VIEWS.get(at) else {
            return true;
        };
        at += 1;
        l.eye = Vec3::new(v[0], v[1], v[2]);
        l.yaw = v[3].to_radians();
        l.pitch = v[4].to_radians();
        l.r.set_depth_prepass(true);
        let name = format!("crossfire_writes_{at}");
        let shot = |l: &mut super::cod4_worldfx_live::Live| {
            let quads = l.quads();
            l.shot_with(Cod4Options::default(), quads, &name)
        };
        // the caches settle on the first frame
        let first = shot(l);
        for _ in 0..2 {
            assert!(shot(l) == first, "view {at}: the frame holds still");
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        first.hash(&mut h);
        out.lock()
            .unwrap()
            .push((at, h.finish(), l.r.frame_writes()));
        false
    });
    if ran.is_none() {
        return;
    }
    let rows = rows.lock().unwrap();
    for (at, hash, writes) in rows.iter() {
        println!("view {at}: frame hash {hash:016x}");
        let mut total = 0;
        for w in writes {
            println!("  {:>12} {}", w.bytes, w.what);
            total += w.bytes;
        }
        println!("  {total:>12} total");
        // gamma 1 and no overbright: the post pass writes the swapchain and
        // no gamma pass copies onto it (cod4-client-perf.md 30)
        assert!(
            writes
                .iter()
                .any(|w| w.what.starts_with("post pass into the swapchain")),
            "view {at}: the frame logged its writes"
        );
        assert!(
            !writes.iter().any(|w| w.what.starts_with("gamma pass")),
            "view {at}: no gamma pass"
        );
    }
    assert_eq!(rows.len(), VIEWS.len(), "every view drew");
}
