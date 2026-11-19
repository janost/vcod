//! The world's `corona` entities as retail adds them each view (CoDMP.exe
//! 0x4e4800): a camera-facing sprite of `flareShader`, pulled 16 units
//! toward the eye, drawn only while the eye sits far enough below it.
//! docs/research/cod11-shader-scripts.md section 8.5, "Coronas".

use glam::Vec3;
use vcod_common::bsp::Corona;

/// The shader every corona draws with (0x4dbd1d).
pub const SHADER: &str = "flareShader";

/// One sprite quad, corners in `RB_AddQuadStamp` order with s,t of
/// (0,0), (1,0), (1,1), (0,1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub corners: [[f32; 3]; 4],
    pub rgba: [u8; 4],
}

pub const UVS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// The box the frustum and cell cull tests: the radius times sqrt 3 about
/// the corona's own origin (0x4e48b8).
pub fn cull_box(c: &Corona) -> (Vec3, Vec3) {
    let o = Vec3::from(c.origin);
    let h = Vec3::splat(c.radius * 1.732_050_8);
    (o - h, o + h)
}

/// The sprite for `c` seen from `eye` with the view's `fwd` and `up`, or
/// `None` past its `zcutoff`.
pub fn sprite(c: &Corona, eye: Vec3, fwd: Vec3, up: Vec3) -> Option<Sprite> {
    let origin = Vec3::from(c.origin);
    let dir = (eye - origin).normalize_or_zero();
    if dir.z >= c.zcutoff {
        return None;
    }
    // the colour's alpha, linear from 0 at zcutoff to full at zfadeout,
    // through `_ftol2`'s truncation (0x4e4acb..0x4e4af4)
    let a = c.rgba[3];
    let alpha = if dir.z <= c.zfadeout {
        a
    } else {
        ((c.zcutoff - dir.z) * f32::from(a) / (c.zcutoff - c.zfadeout)) as u8
    };
    let center = origin + dir * 16.0;
    // viewParms.or.axis[1] (left) and [2] (up), each times the radius (0x5101d0)
    let left_axis = up.cross(fwd).normalize_or_zero();
    let up_axis = fwd.cross(left_axis);
    let left = left_axis * c.radius;
    let up = up_axis * c.radius;
    Some(Sprite {
        corners: [
            (center + left + up).to_array(),
            (center - left + up).to_array(),
            (center - left - up).to_array(),
            (center + left - up).to_array(),
        ],
        rgba: [c.rgba[0], c.rgba[1], c.rgba[2], alpha],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corona() -> Corona {
        Corona {
            origin: [0.0, 0.0, 100.0],
            radius: 10.0,
            zcutoff: -0.15,
            zfadeout: -0.25,
            rgba: [255, 200, 100, 255],
        }
    }

    #[test]
    fn hidden_from_level_or_above() {
        let c = corona();
        // level with it: z 0, past the -0.15 cutoff
        assert!(sprite(&c, Vec3::new(-100.0, 0.0, 100.0), Vec3::X, Vec3::Z).is_none());
        assert!(sprite(&c, Vec3::new(-100.0, 0.0, 200.0), Vec3::X, Vec3::Z).is_none());
    }

    #[test]
    fn fades_between_cutoff_and_fadeout() {
        let c = corona();
        // straight below: full alpha
        let s = sprite(&c, Vec3::ZERO, Vec3::Z, Vec3::NEG_X).unwrap();
        assert_eq!(s.rgba, [255, 200, 100, 255]);
        // z = -0.2, halfway: 0.05 * 255 / 0.1 = 127.5, truncated
        let z = -0.2f32;
        let horiz = (1.0 - z * z).sqrt();
        let eye = Vec3::new(-horiz, 0.0, z) * 500.0 + Vec3::new(0.0, 0.0, 100.0);
        let s = sprite(&c, eye, Vec3::X, Vec3::Z).unwrap();
        assert!((126..=127).contains(&s.rgba[3]), "{:?}", s.rgba);
    }

    #[test]
    fn faces_the_view_and_sits_16_units_nearer() {
        let c = corona();
        let eye = Vec3::new(-300.0, 0.0, 0.0);
        let fwd = (Vec3::from(c.origin) - eye).normalize();
        let s = sprite(&c, eye, fwd, Vec3::Z).unwrap();
        let p = s.corners.map(Vec3::from);
        let center = (p[0] + p[2]) * 0.5;
        let want = Vec3::from(c.origin) - fwd * 16.0;
        assert!(center.distance(want) < 1e-3, "{center} {want}");
        // a square of half-width radius in the view plane
        assert!(((p[0] - p[1]).length() - 20.0).abs() < 1e-3);
        assert!(((p[1] - p[2]).length() - 20.0).abs() < 1e-3);
        for e in [p[0] - p[1], p[1] - p[2]] {
            assert!(e.dot(fwd).abs() < 1e-3);
        }
        // corner 0 is left and up of the centre: s 0, t 0
        let left = Vec3::Z.cross(fwd).normalize();
        assert!((p[0] - center).dot(left) > 0.0 && (p[0] - center).z > 0.0);
    }
}
