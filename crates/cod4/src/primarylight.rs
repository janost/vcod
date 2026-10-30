//! The non-sun primary light a dynamic model is lit by, as 1.7's
//! `R_GetNonSunPrimaryLightForBox` (0x5fc5c0) picks it for the box the
//! model's scene entity spans (`docs/research/cod4-world.md` 3.5): the
//! first spot or omni light past the sun whose sphere (or a spot's cone)
//! reaches the box and whose region, when it has hulls, has a hull the box
//! is not outside of.

use crate::assets::world::LightRegionHull;
use crate::map::{Map, PrimaryLight};

/// `GFX_LIGHT_TYPE_SPOT`; 3 is an omni light, 1 the sun.
pub const SPOT: u8 = 2;

/// A map's primary lights and their regions.
#[derive(Clone, Debug, Default)]
pub struct PrimaryLights {
    pub lights: Vec<PrimaryLight>,
    pub regions: Vec<Vec<LightRegionHull>>,
    pub sun: u32,
}

impl PrimaryLights {
    pub fn of(map: &Map) -> PrimaryLights {
        PrimaryLights {
            lights: map.primary_lights.clone(),
            regions: map.light_regions.clone(),
            sun: map.sun_light_index,
        }
    }

    /// `R_GetNonSunPrimaryLightForBox` for the box `mid` +- `half`: the
    /// first light past the sun that reaches it, 0 for none.
    pub fn for_box(&self, mid: [f32; 3], half: [f32; 3]) -> u8 {
        let first = self.sun as usize + 1;
        for (i, l) in self.lights.iter().enumerate().skip(first) {
            if cull_box_from_light(l, mid, half) {
                continue;
            }
            let hulls = self.regions.get(i).map_or(&[][..], Vec::as_slice);
            let diff = [0, 1, 2].map(|k| mid[k] - l.origin[k]);
            // 0x5fb780: a region without hulls bounds nothing
            if hulls.is_empty() || hulls.iter().any(|h| !cull_box_from_hull(h, diff, half)) {
                return i.min(255) as u8;
            }
        }
        0
    }
}

/// `Com_CullBoxFromPrimaryLight`: a spot with a non-negative expanded
/// cone takes the cone's test (0x562fd0), the rest the sphere's (0x562ea0).
fn cull_box_from_light(l: &PrimaryLight, mid: [f32; 3], half: [f32; 3]) -> bool {
    if l.kind == SPOT && l.cos_half_fov_expanded >= 0.0 {
        let cone = Cone {
            apex: l.origin,
            back: l.dir,
            cos: l.cos_half_fov_expanded,
            radius: l.radius,
        };
        cone.misses(mid, half)
    } else {
        cull_box_from_sphere(l.origin, l.radius, mid, half)
    }
}

/// The box's distance from `org` per axis, 0 inside its slab.
fn box_gap(org: [f32; 3], mid: [f32; 3], half: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|k| ((mid[k] - org[k]).abs() - half[k]).max(0.0))
}

fn length_sq(v: [f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The sphere cull (0x562ea0): the box's nearest point farther than
/// `radius`.
fn cull_box_from_sphere(org: [f32; 3], radius: f32, mid: [f32; 3], half: [f32; 3]) -> bool {
    radius * radius < length_sq(box_gap(org, mid, half))
}

/// What a spot light reaches: the sphere of `radius` round `apex` cut to
/// the cone of half angle `acos(cos)` down the beam. `back` is the light's
/// `dir`, a unit vector pointing up the beam at the light.
struct Cone {
    apex: [f32; 3],
    back: [f32; 3],
    cos: f32,
    radius: f32,
}

impl Cone {
    /// Whether the box `mid` +- `half` lies wholly outside (iw3mp.exe
    /// 0x562fd0, cod4-world.md 3.5). It does when it misses the sphere;
    /// when even its corner farthest down the beam is not past the apex;
    /// or, with that corner outside the cone, when the plane through the
    /// apex that touches the cone on the corner's side has the whole box
    /// in front of it.
    fn misses(&self, mid: [f32; 3], half: [f32; 3]) -> bool {
        if cull_box_from_sphere(self.apex, self.radius, mid, half) {
            return true;
        }
        let rel: [f32; 3] = std::array::from_fn(|k| mid[k] - self.apex[k]);
        let deepest: [f32; 3] = std::array::from_fn(|k| {
            if self.back[k] < 0.0 {
                rel[k] + half[k]
            } else {
                rel[k] - half[k]
            }
        });
        let along = dot(deepest, self.back);
        if along >= 0.0 {
            return true;
        }
        let aside: [f32; 3] = std::array::from_fn(|k| deepest[k] - along * self.back[k]);
        let aside_sq = length_sq(aside);
        let cos_sq = self.cos * self.cos;
        let sin_sq = 1.0 - cos_sq;
        // the corner is inside the cone: its tan^2 off the beam within the cone's
        if along * along * sin_sq >= aside_sq * cos_sq {
            return false;
        }
        let tilt = self.cos / (aside_sq * sin_sq).sqrt();
        let normal: [f32; 3] = std::array::from_fn(|k| self.back[k] + tilt * aside[k]);
        let gap = (0..3).fold(dot(normal, rel), |gap, k| gap - (normal[k] * half[k]).abs());
        gap >= 0.0
    }
}

/// `R_CullBoxFromLightRegionHull` (0x5fb490): the box `diff` +- `half`,
/// relative to the light, outside the hull's k-DOP or along one of its
/// extra axes (a gap of 0 counts as outside).
fn cull_box_from_hull(h: &LightRegionHull, diff: [f32; 3], half: [f32; 3]) -> bool {
    let [x, y, z] = diff;
    let [hx, hy, hz] = half;
    let kdop = [
        (x, hx),
        (y, hy),
        (z, hz),
        (x + y, hx + hy),
        (x - y, hx + hy),
        (x + z, hx + hz),
        (x - z, hx + hz),
        (y + z, hy + hz),
        (y - z, hy + hz),
    ];
    for (k, (m, e)) in kdop.into_iter().enumerate() {
        if (m - h.kdop_mid[k]).abs() >= e + h.kdop_half[k] {
            return true;
        }
    }
    h.axes.iter().any(|a| {
        let e = hx * a.dir[0].abs() + hy * a.dir[1].abs() + hz * a.dir[2].abs();
        (dot(diff, a.dir) - a.mid).abs() >= e + a.half
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn omni(origin: [f32; 3], radius: f32) -> PrimaryLight {
        PrimaryLight {
            kind: 3,
            can_shadow: false,
            exponent: 0,
            color: [1.0; 3],
            dir: [0.0, 0.0, 1.0],
            origin,
            radius,
            cos_half_fov_outer: 0.0,
            cos_half_fov_inner: 0.0,
            cos_half_fov_expanded: 0.0,
            def: None,
        }
    }

    /// The first light past the sun that reaches the box, its region's
    /// hulls bounding it.
    #[test]
    fn the_first_reaching_light_past_the_sun_wins() {
        let lights = PrimaryLights {
            lights: vec![
                omni([0.0; 3], 0.0),
                omni([0.0; 3], 1e6),
                omni([100.0, 0.0, 0.0], 50.0),
                omni([100.0, 0.0, 0.0], 200.0),
            ],
            regions: vec![Vec::new(); 4],
            sun: 1,
        };
        // 40 units of gap past light 2's 50: light 3 reaches it
        assert_eq!(lights.for_box([10.0, 0.0, 0.0], [0.0; 3]), 3);
        // a box reaching into light 2's sphere
        assert_eq!(lights.for_box([10.0, 0.0, 0.0], [45.0, 1.0, 1.0]), 2);
        assert_eq!(lights.for_box([1000.0, 0.0, 0.0], [1.0; 3]), 0);
        // a hull far from the box keeps light 2 out of it
        let mut boxed = lights.clone();
        boxed.regions[2] = vec![LightRegionHull {
            kdop_mid: [500.0; 9],
            kdop_half: [1.0; 9],
            axes: Vec::new(),
        }];
        assert_eq!(boxed.for_box([10.0, 0.0, 0.0], [45.0, 1.0, 1.0]), 3);
    }

    /// A spot pointing down -z (its `dir` +z, back at the light) reaches
    /// a box under it, not one above it or far off its axis.
    #[test]
    fn a_spot_reaches_what_its_cone_covers() {
        let spot = PrimaryLight {
            kind: SPOT,
            can_shadow: false,
            cos_half_fov_expanded: 0.9,
            ..omni([0.0, 0.0, 100.0], 300.0)
        };
        let half = [5.0; 3];
        assert!(!cull_box_from_light(&spot, [0.0, 0.0, 0.0], half));
        assert!(cull_box_from_light(&spot, [0.0, 0.0, 150.0], half));
        assert!(cull_box_from_light(&spot, [150.0, 0.0, 0.0], half));
        // the sphere's test alone reaches the off-axis one
        assert!(!cull_box_from_light(
            &omni([0.0, 0.0, 100.0], 300.0),
            [150.0, 0.0, 0.0],
            half
        ));
    }

    /// FNV-1a over a stream of words: the pins below hash every output's bits.
    fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
        words.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, w| {
            (h ^ u64::from(w)).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// A grid of boxes against spots of four cones, bit for bit.
    #[test]
    fn pins_the_cone_cull() {
        let mut out = Vec::new();
        for (cos, dir) in [
            (0.9, [0.0, 0.0, 1.0]),
            (0.5, [0.6, 0.0, 0.8]),
            (0.0, [-0.48, 0.6, -0.64]),
            (0.99, [0.0, -1.0, 0.0]),
        ] {
            let spot = PrimaryLight {
                kind: SPOT,
                dir,
                cos_half_fov_expanded: cos,
                ..omni([3.0, -7.0, 40.0], 260.0)
            };
            for x in (-300..=300).step_by(37) {
                for y in (-300..=300).step_by(41) {
                    for z in (-200..=280).step_by(43) {
                        let mid = [x as f32, y as f32 + 0.5, z as f32];
                        let half = [(x & 15) as f32 + 1.0, 6.0, (z & 7) as f32 * 3.0];
                        out.push(u32::from(cull_box_from_light(&spot, mid, half)));
                    }
                }
            }
        }
        let culled = out.iter().filter(|&&c| c == 1).count();
        assert!(
            culled > 0 && culled < out.len(),
            "{culled} of {}",
            out.len()
        );
        assert_eq!(fnv(out), PIN_CONE);
    }
    const PIN_CONE: u64 = 13960210394312989177;
}
