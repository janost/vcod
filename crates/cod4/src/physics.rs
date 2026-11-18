//! A `physicsLaunch`ed part on the client: the rigid body iw3mp.exe's
//! `CG_CreatePhysicsObject` (0x434c70) hands ODE's fx world, stood in by a
//! small impulse solver that sweeps the body's points through the clip
//! map and meets the world's brushes and triangles with its boxes.
//! Retail's model and where this one differs: cod4-gsc.md 3.6.

use crate::assets::Assets;
use crate::assets::model::{PhysBrush, PhysGeomList, XModel};
use crate::ragdoll::{Sweep, Trace};
use glam::{Mat3, Quat, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use vcod_common::turretpose::{angles_quat, quat_angles};

/// `PhysGeomInfo.type`s the solver shapes (cod4-gsc.md 3.6).
const GEOM_BOX: i32 = 1;
const GEOM_CYLINDER: i32 = 4;
/// A cylinder's points round each cap.
const RING: usize = 8;
/// ODE's `bounce_vel` on every contact (0x5a609e, 0.1): slower approaches
/// do not bounce.
const BOUNCE_VEL: f32 = 0.1;
/// Contact solver passes per step (vcod's).
const ITERATIONS: usize = 10;
/// `phys_jitterMaxMass`'s default (registered at 0x5a41f3..0x5a42a0: 200,
/// 0.1..FLT_MAX): a jitter falls off with the body's mass up to it.
pub const JITTER_MAX_MASS: f32 = 200.0;
/// The least contact normal z a jitter lifts at (the float at 0x70b0e8,
/// read at 0x5a23f3).
const JITTER_MIN_NORMAL_Z: f32 = 0.7;
/// Two contacts face each other under this dot of their normals (the
/// double at 0x70b268, -0.99 as a float): 0x5a2280 pinches the body on
/// it and 0x5a25d0 keeps one of the two.
const FACING: f32 = -0.99;
/// A pinched body drops a contact whose point moves along its normal by
/// more than this, the dot of the normal with the point's velocity
/// normalized (the float at 0x6df494, read at 0x5a2894 and 0x5a28f9).
const SEPARATING: f32 = 0.01;

/// A `physicsJitter` region (`EV_PHYS_JITTER`, held by 0x5a7730 in its
/// world's list of five until the world's next step): each contact of a
/// body on ground within `outer` of `origin` (level) takes an upward
/// push of `min..max` (cod4-world.md 13.8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jitter {
    pub origin: Vec3,
    pub inner: f32,
    pub outer: f32,
    pub min: f32,
    pub max: f32,
}

/// What a part's `PhysPreset` gives the body: `mass` (+8), `bounce`
/// (+12), `friction` (+16) and `bulletForceScale` (+20).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub mass: f32,
    pub bounce: f32,
    pub friction: f32,
    pub bullet_force_scale: f32,
}

impl Preset {
    /// From the asset's 0x2C bytes.
    pub fn from_raw(raw: &[u8]) -> Option<Preset> {
        let f = |o: usize| Some(f32::from_le_bytes(raw.get(o..o + 4)?.try_into().ok()?));
        Some(Preset {
            mass: f(8)?,
            bounce: f(12)?,
            friction: f(16)?,
            bullet_force_scale: f(20)?,
        })
    }
}

/// The `phys_*` dvars the fx world reads, at their 1.7 defaults
/// (cod4-gsc.md 3.6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dvars {
    /// `phys_gravity` (-800).
    pub gravity: f32,
    /// `phys_bulletUpBias` (0.5) and `phys_bulletSpinScale` (3).
    pub bullet_up_bias: f32,
    pub bullet_spin_scale: f32,
    /// `phys_frictionScale` (1).
    pub friction_scale: f32,
    /// `phys_dragLinear` (0.03) and `phys_dragAngular` (0.5), per second.
    pub drag_linear: f32,
    pub drag_angular: f32,
    /// `phys_autoDisableLinear` (20), `phys_autoDisableAngular` (1) and
    /// `phys_autoDisableTime` (0.9 s).
    pub sleep_linear: f32,
    pub sleep_angular: f32,
    pub sleep_time: f32,
    /// `phys_contact_erp` (0.8) and `phys_mcv` (20): how fast a corner in
    /// the world is pushed out.
    pub erp: f32,
    pub max_correcting: f32,
    /// `msecStep` (17 ms, the table at 0x71e798): a lone body's step, and
    /// what a jitter's push is spread over. A [`WorldClock`] sets it per
    /// step ([`msec_step`]).
    pub step_ms: i32,
    /// `phys_narrowObjMaxLength` (4): a box with a side shorter, or a
    /// cylinder thinner, takes the narrow pass ([`box_contacts`]).
    pub narrow_max_length: f32,
    /// `phys_csl` (1): the post-step sweep goes as a point for a body
    /// whose sweep size is under twice it ([`Body::sweep`]).
    pub csl: f32,
}

impl Default for Dvars {
    fn default() -> Self {
        Dvars {
            gravity: -800.0,
            bullet_up_bias: 0.5,
            bullet_spin_scale: 3.0,
            friction_scale: 1.0,
            drag_linear: 0.03,
            drag_angular: 0.5,
            sleep_linear: 20.0,
            sleep_angular: 1.0,
            sleep_time: 0.9,
            erp: 0.8,
            max_correcting: 20.0,
            step_ms: 17,
            narrow_max_length: 4.0,
            csl: 1.0,
        }
    }
}

/// A part's collision and mass in its model's frame: the points that meet
/// the world, the boxes and cylinders among its geoms, the centre of mass,
/// the inertia per unit of mass about it, and the half size its post-step
/// sweep reads ([`Body::sweep`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub points: Vec<Vec3>,
    pub boxes: Vec<ShapeBox>,
    pub cylinders: Vec<ShapeCylinder>,
    /// Which of the model's geoms each point, box and cylinder is, in the
    /// order 0x5a5710 adds them: the contact pass runs per geom.
    pub point_geoms: Vec<usize>,
    pub box_geoms: Vec<usize>,
    pub cylinder_geoms: Vec<usize>,
    pub com: Vec3,
    pub inertia: Mat3,
    pub sweep_half: f32,
}

/// A cylinder geom: its centre, its axis (unit), its radius and half
/// height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeCylinder {
    pub centre: Vec3,
    pub axis: Vec3,
    pub radius: f32,
    pub half: f32,
}

/// A box geom: its centre, its axes (the columns) and half sizes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeBox {
    pub centre: Vec3,
    pub axes: Mat3,
    pub half: Vec3,
}

impl Shape {
    /// `Phys_ObjSetCollisionFromXModel` (0x5a5710): the model's `physGeoms`
    /// when it has any, else a box on its bounds.
    pub fn of(model: &XModel) -> Shape {
        model
            .phys_geoms
            .as_ref()
            .and_then(Shape::of_geoms)
            .unwrap_or_else(|| Shape::of_bounds(model.mins.into(), model.maxs.into()))
    }

    /// A dynamic entity's shape (0x490240): its model's `physGeoms` as
    /// [`Self::of`] reads them, else a box on its bounds as they are, no
    /// flat axis widened (0x48fd00, 0x5a5240).
    pub fn of_dynent(model: &XModel) -> Shape {
        if model.phys_geoms.is_some() {
            return Shape::of(model);
        }
        let (mins, maxs) = (Vec3::from(model.mins), Vec3::from(model.maxs));
        let (c, h) = ((mins + maxs) * 0.5, (maxs - mins) * 0.5);
        Shape {
            point_geoms: vec![0; corners(c, Mat3::IDENTITY, h).len()],
            box_geoms: vec![0],
            cylinder_geoms: Vec::new(),
            points: corners(c, Mat3::IDENTITY, h),
            boxes: vec![ShapeBox::new(c, Mat3::IDENTITY, h)],
            cylinders: Vec::new(),
            com: c,
            inertia: box_inertia(h),
            sweep_half: h.min_element(),
        }
    }

    /// A box on `mins`..`maxs`, its centre the centre of mass; any flat
    /// axis takes retail's 100-unit cube (0x5a587a).
    pub fn of_bounds(mins: Vec3, maxs: Vec3) -> Shape {
        let (mins, maxs) = if (0..3).any(|k| mins[k] == maxs[k]) {
            (Vec3::splat(-50.0), Vec3::splat(50.0))
        } else {
            (mins, maxs)
        };
        let c = (mins + maxs) * 0.5;
        let h = (maxs - mins) * 0.5;
        Shape {
            point_geoms: vec![0; corners(c, Mat3::IDENTITY, h).len()],
            box_geoms: vec![0],
            cylinder_geoms: Vec::new(),
            points: corners(c, Mat3::IDENTITY, h),
            boxes: vec![ShapeBox::new(c, Mat3::IDENTITY, h)],
            cylinders: Vec::new(),
            com: c,
            inertia: box_inertia(h),
            sweep_half: h.min_element(),
        }
    }

    /// The brushes' and boxes' corners and the cylinders' rims, with the
    /// list's `PhysMass`; `None` when the list gives no point. The sweep's
    /// half size is the last geom's: ODE puts each geom it gives a body at
    /// the head of the body's list, which 0x5a6930 reads.
    fn of_geoms(list: &PhysGeomList) -> Option<Shape> {
        let mut points = Vec::new();
        let mut boxes = Vec::new();
        let mut cylinders = Vec::new();
        let (mut point_geoms, mut box_geoms, mut cylinder_geoms) =
            (Vec::new(), Vec::new(), Vec::new());
        let mut sweep_half = 0.0;
        for (gi, g) in list.geoms.iter().enumerate() {
            if let Some(b) = &g.brush {
                let (lo, hi) = (Vec3::from(b.mins), Vec3::from(b.maxs));
                let (c, h) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
                sweep_half = brush_sweep_half(lo, hi);
                // A brush no side cuts is its bounds' box, as is one
                // with more sides than corners are worth finding.
                let cut = (!b.planes.is_empty() && b.planes.len() + 6 <= MAX_BRUSH_PLANES)
                    .then(|| brush_corners(b))
                    .filter(|cut| {
                        let at_bounds = |p: &Vec3| ((*p - c).abs() - h).abs().max_element() < 0.01;
                        !(cut.len() == 8 && cut.iter().all(at_bounds))
                    });
                match cut {
                    Some(cut) => points.extend(cut),
                    None => {
                        points.extend(corners(c, Mat3::IDENTITY, h));
                        boxes.push(ShapeBox::new(c, Mat3::IDENTITY, h));
                    }
                }
                point_geoms.resize(points.len(), gi);
                box_geoms.resize(boxes.len(), gi);
                continue;
            }
            let axis = Mat3::from_cols(
                g.orientation[0].into(),
                g.orientation[1].into(),
                g.orientation[2].into(),
            );
            let at = Vec3::from(g.offset);
            let h = Vec3::from(g.half_lengths);
            match g.kind {
                GEOM_BOX => {
                    points.extend(corners(at, axis, h));
                    boxes.push(ShapeBox::new(at, axis, h));
                    sweep_half = h.min_element();
                }
                // The axis is the first row (0x5a55f0 gives the geom axis
                // 1, the rotation's first column), `halfLengths` the half
                // height and the radius.
                GEOM_CYLINDER => {
                    for end in [-h.x, h.x] {
                        for i in 0..RING {
                            let a = i as f32 * std::f32::consts::TAU / RING as f32;
                            let local = Vec3::new(end, h.y * a.cos(), h.y * a.sin());
                            points.push(at + axis * local);
                        }
                    }
                    cylinders.push(ShapeCylinder {
                        centre: at,
                        axis: axis.x_axis.normalize_or(Vec3::X),
                        radius: h.y,
                        half: h.x,
                    });
                    // 0x5a688b: the radius over root 2, or the half
                    // height when less.
                    sweep_half = (h.y * std::f32::consts::FRAC_1_SQRT_2).min(h.x);
                }
                _ => {}
            }
            point_geoms.resize(points.len(), gi);
            box_geoms.resize(boxes.len(), gi);
            cylinder_geoms.resize(cylinders.len(), gi);
        }
        if points.is_empty() {
            return None;
        }
        let m = list.moments.map(|m| if m > 0.0 { m } else { 1.0 });
        let p = list.products;
        Some(Shape {
            points,
            boxes,
            cylinders,
            point_geoms,
            box_geoms,
            cylinder_geoms,
            com: list.center_of_mass.into(),
            inertia: Mat3::from_cols(
                Vec3::new(m[0], p[0], p[1]),
                Vec3::new(p[0], m[1], p[2]),
                Vec3::new(p[1], p[2], m[2]),
            ),
            sweep_half,
        })
    }
}

/// A brush geom's sweep half size (0x5a684f, 0x561910): its bounds times
/// 0.01, x and y cut down to z's size about their middles where longer,
/// and the top x. Every stock brush gives well under twice `phys_csl`, so
/// its sweep goes as a point.
fn brush_sweep_half(mins: Vec3, maxs: Vec3) -> f32 {
    let (lo, hi) = (mins * 0.01, maxs * 0.01);
    let s = hi - lo;
    if s.z < s.x {
        hi.x - (s.x - s.z) * 0.5
    } else {
        hi.x
    }
}

/// What `CG_CreatePhysicsObject` reads off a part's model: its shape and
/// its preset.
#[derive(Clone, Debug, PartialEq)]
pub struct PartModel {
    pub shape: Shape,
    pub preset: Preset,
}

/// Every xmodel of `zones` naming a preset one of them holds, by lower-case
/// name, the first zone naming either winning. A zone that only refers to
/// a preset lists it as `,name` with no body.
pub fn part_models(zones: &[&Assets]) -> HashMap<String, Arc<PartModel>> {
    let mut presets: HashMap<String, Preset> = HashMap::new();
    for z in zones {
        for p in z.phys_presets.iter().filter(|p| !p.name.starts_with(',')) {
            if let Some(v) = Preset::from_raw(&p.raw) {
                presets.entry(p.name.to_ascii_lowercase()).or_insert(v);
            }
        }
    }
    let mut out = HashMap::new();
    for z in zones {
        for m in &z.xmodels {
            let Some(p) = m.phys_preset else { continue };
            let name = z.phys_presets[p.index()].name.trim_start_matches(',');
            let Some(preset) = presets.get(&name.to_ascii_lowercase()) else {
                continue;
            };
            out.entry(m.name.to_ascii_lowercase()).or_insert_with(|| {
                Arc::new(PartModel {
                    shape: Shape::of(m),
                    preset: *preset,
                })
            });
        }
    }
    out
}

impl ShapeBox {
    fn new(centre: Vec3, axes: Mat3, half: Vec3) -> ShapeBox {
        ShapeBox { centre, axes, half }
    }
}

/// A box's points: its eight corners, then its edges' midpoints and its
/// faces' centres, so a world edge poking in between two corners meets
/// one; centre `at`, axes the columns of `axis`.
fn corners(at: Vec3, axis: Mat3, h: Vec3) -> Vec<Vec3> {
    let grid = (0..27).map(|i| Vec3::new((i % 3) as f32, (i / 3 % 3) as f32, (i / 9) as f32) - 1.0);
    let mut out: Vec<Vec3> = grid.filter(|g| *g != Vec3::ZERO).collect();
    // Corners first, then edges, then faces.
    out.sort_by_key(|g| std::cmp::Reverse(g.abs().element_sum() as u32));
    out.into_iter().map(|g| at + axis * (g * h)).collect()
}

/// A brush's corners: every point where three of its planes (the six of
/// its bounds, then its sides) meet that lies inside all of them.
fn brush_corners(b: &PhysBrush) -> Vec<Vec3> {
    let (lo, hi) = (Vec3::from(b.mins), Vec3::from(b.maxs));
    let mut planes: Vec<(Vec3, f32)> = (0..3)
        .flat_map(|k| [(-Vec3::AXES[k], -lo[k]), (Vec3::AXES[k], hi[k])])
        .collect();
    planes.extend(b.planes.iter().map(|p| (Vec3::new(p[0], p[1], p[2]), p[3])));
    plane_corners(&planes)
}

/// The most planes [`plane_corners`] takes: its cost is cubic to quartic
/// in them, and the stock maps' brushes have at most 38 (6 axial and 32
/// sides).
const MAX_BRUSH_PLANES: usize = 64;

/// Where three of `planes` (`n . p <= d` inside) meet inside all of them,
/// each corner once.
fn plane_corners(planes: &[(Vec3, f32)]) -> Vec<Vec3> {
    const EPS: f32 = 0.01;
    let mut out: Vec<Vec3> = Vec::new();
    for i in 0..planes.len() {
        for j in i + 1..planes.len() {
            for k in j + 1..planes.len() {
                let ((a, da), (bn, db), (c, dc)) = (planes[i], planes[j], planes[k]);
                let det = a.dot(bn.cross(c));
                if det.abs() < 1e-6 {
                    continue;
                }
                let p = (bn.cross(c) * da + c.cross(a) * db + a.cross(bn) * dc) / det;
                let inside = planes.iter().all(|(n, d)| n.dot(p) <= d + EPS);
                if inside && !out.iter().any(|q| q.distance(p) < EPS) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// A solid box's inertia per unit of mass, `h` its half sizes.
fn box_inertia(h: Vec3) -> Mat3 {
    let s = h * h / 3.0;
    Mat3::from_diagonal(Vec3::new(s.y + s.z, s.x + s.z, s.x + s.y))
}

/// A convex piece of the world a box geom meets: a brush, or a triangle
/// (`open`, solid only behind its one face).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Solid {
    /// Each face's plane, `n . p <= d` inside, and its corners in order
    /// round `n`.
    pub faces: Vec<(Vec3, f32, Vec<Vec3>)>,
    pub verts: Vec<Vec3>,
    pub open: bool,
}

/// The world's solids round a box, `lo..hi`.
pub type Solids<'a> = dyn FnMut(Vec3, Vec3) -> Vec<Solid> + 'a;

impl Solid {
    /// A brush of `planes`; `None` when they bound nothing or number
    /// past [`MAX_BRUSH_PLANES`].
    pub fn brush(planes: &[(Vec3, f32)]) -> Option<Solid> {
        const EPS: f32 = 0.01;
        if planes.len() > MAX_BRUSH_PLANES {
            return None;
        }
        let verts = plane_corners(planes);
        let mut faces = Vec::new();
        for &(n, d) in planes {
            let mut on: Vec<Vec3> = verts
                .iter()
                .copied()
                .filter(|v| (n.dot(*v) - d).abs() < EPS)
                .collect();
            if on.len() < 3 {
                continue;
            }
            let c = on.iter().copied().sum::<Vec3>() / on.len() as f32;
            let t0 = n.any_orthonormal_vector();
            let t1 = n.cross(t0);
            let angle = |v: &Vec3| (*v - c).dot(t1).atan2((*v - c).dot(t0));
            on.sort_by(|a, b| angle(a).total_cmp(&angle(b)));
            faces.push((n, d, on));
        }
        (faces.len() >= 4).then_some(Solid {
            faces,
            verts,
            open: false,
        })
    }

    /// A triangle, its front `(c - a) x (b - a)` as the point trace's
    /// (`CM_TracePointThroughTriangle`); `None` when degenerate.
    pub fn triangle(t: [Vec3; 3]) -> Option<Solid> {
        let n = (t[2] - t[0]).cross(t[1] - t[0]).try_normalize()?;
        Some(Solid {
            // Corners in order round the front.
            faces: vec![(n, n.dot(t[0]), vec![t[0], t[2], t[1]])],
            verts: t.to_vec(),
            open: true,
        })
    }
}

/// `poly` clipped to the side of each plane `(n, d)` where `n . p >= d`.
fn clip_polygon(mut poly: Vec<Vec3>, planes: impl IntoIterator<Item = (Vec3, f32)>) -> Vec<Vec3> {
    for (n, d) in planes {
        if poly.is_empty() {
            break;
        }
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            let (da, db) = (n.dot(a) - d, n.dot(b) - d);
            if da >= 0.0 {
                out.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                out.push(a + (b - a) * (da / (da - db)));
            }
        }
        poly = out;
    }
    poly
}

/// A box (centre `c`, axes the columns of `a`, half sizes `h`, world)
/// against `s`, as the fx world's box-against-brush collider does it
/// (iw3mp.exe 0x59a610, cod4-gsc.md 3.6.1): the least deep of the box's
/// faces and the solid's is the reference, the face of the other most
/// against it is clipped to it, and each clipped point under `margin`
/// off it is a contact. A `narrow` box then also takes each other face of
/// a brush its centre lies in front of and its box meets as a reference
/// (0x59aabe). Pushes `(point on the box, normal out of the solid,
/// separation)`, the separation below 0 inside. A triangle is only ever
/// the reference, and is met from its front.
fn box_contacts(
    c: Vec3,
    a: Mat3,
    h: Vec3,
    s: &Solid,
    margin: f32,
    narrow: bool,
    out: &mut Vec<(Vec3, Vec3, f32)>,
) {
    let axes = [a.x_axis, a.y_axis, a.z_axis];
    let box_verts: Vec<Vec3> = (0..8)
        .map(|i| {
            let sign = |bit: u32| if i >> bit & 1 == 1 { 1.0 } else { -1.0 };
            c + axes[0] * (h.x * sign(0)) + axes[1] * (h.y * sign(1)) + axes[2] * (h.z * sign(2))
        })
        .collect();
    // The solid's faces: how far the box's deepest corner lies off each.
    let mut face = (f32::NEG_INFINITY, 0);
    for (i, (n, d, poly)) in s.faces.iter().enumerate() {
        // The centre behind the triangle: its back, but for a floor's
        // triangle off whose edge the centre lies, as over a step's
        // corner.
        let floor_edge = n.z >= FLOOR_NORMAL_Z && !inside_edges(c, *n, poly);
        if s.open && n.dot(c) < *d && !floor_edge {
            return;
        }
        let sep = box_verts
            .iter()
            .map(|v| n.dot(*v))
            .fold(f32::INFINITY, f32::min)
            - d;
        if sep > margin {
            return;
        }
        if sep > face.0 {
            face = (sep, i);
        }
    }
    // The box's faces: its intervals against the solid's along its axes.
    let mut side = (f32::NEG_INFINITY, 0, 1.0);
    for (k, ax) in axes.iter().enumerate() {
        let (lo, hi) = s
            .verts
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                let x = ax.dot(*v);
                (lo.min(x), hi.max(x))
            });
        let mid = ax.dot(c);
        let (above, below) = (lo - (mid + h[k]), (mid - h[k]) - hi);
        let (sep, sign) = if above >= below {
            (above, 1.0)
        } else {
            (below, -1.0)
        };
        if sep > margin {
            return;
        }
        if sep > side.0 {
            side = (sep, k, sign);
        }
    }
    let on_face = s.open || face.0 >= side.0;
    // The face the reference pair took from the solid.
    let mut taken = face.1;
    if on_face {
        face_reference(c, &axes, h, &s.faces[face.1], margin, out);
    } else {
        // The box's face is the reference: the solid's face most against
        // it, clipped to the face's rectangle.
        let (k, sign) = (side.1, side.2);
        let nb = axes[k] * sign;
        let plane = nb.dot(c) + h[k];
        let Some((i, (_, _, poly))) = s
            .faces
            .iter()
            .enumerate()
            .min_by(|x, y| x.1.0.dot(nb).total_cmp(&y.1.0.dot(nb)))
        else {
            return;
        };
        taken = i;
        let rect = [(k + 1) % 3, (k + 2) % 3].into_iter().flat_map(|j| {
            let m = axes[j].dot(c);
            [(axes[j], m - h[j]), (-axes[j], -m - h[j])]
        });
        for p in clip_polygon(poly.clone(), rect) {
            let sep = nb.dot(p) - plane;
            if sep <= margin {
                out.push((p - nb * sep, -nb, sep));
            }
        }
    }
    // A triangle's own edges too: its face's clip meets none of them.
    edge_contacts(c, &axes, h, s, if s.open { usize::MAX } else { taken }, out);
    if !narrow || s.open {
        return;
    }
    // The narrow pass: every other face the centre is in front of whose
    // winding the box's six faces leave something of.
    let slabs: Vec<(Vec3, f32)> = (0..3)
        .flat_map(|k| {
            let m = axes[k].dot(c);
            [(axes[k], m - h[k]), (-axes[k], -m - h[k])]
        })
        .collect();
    for (i, f) in s.faces.iter().enumerate() {
        if on_face && i == face.1 || f.0.dot(c) <= f.1 {
            continue;
        }
        if !clip_polygon(f.2.clone(), slabs.iter().copied()).is_empty() {
            face_reference(c, &axes, h, f, margin, out);
        }
    }
}

/// The solid's edges where they run inside the box: each clipped edge's
/// ends and middle deeper than the skin in it push the box out along its
/// face nearest the point, but for face `taken`'s, which the reference
/// clip has already met. Neither face is then the right reference: a
/// kerb's or a ledge's edge across a box's face, or a mesh stair's nosing
/// through a picture frame. vcod's.
fn edge_contacts(
    c: Vec3,
    axes: &[Vec3; 3],
    h: Vec3,
    s: &Solid,
    taken: usize,
    out: &mut Vec<(Vec3, Vec3, f32)>,
) {
    let local = |p: Vec3| Vec3::new(axes[0].dot(p - c), axes[1].dot(p - c), axes[2].dot(p - c));
    for (_, _, poly) in s
        .faces
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != taken)
        .map(|(_, f)| f)
    {
        for i in 0..poly.len() {
            let (a, b) = (local(poly[i]), local(poly[(i + 1) % poly.len()]));
            // The segment clipped to the box, as fractions along it.
            let (mut t0, mut t1) = (0.0f32, 1.0f32);
            let d = b - a;
            for k in 0..3 {
                if d[k].abs() < 1e-6 {
                    if a[k].abs() > h[k] {
                        t1 = -1.0;
                    }
                    continue;
                }
                let (u, v) = ((-h[k] - a[k]) / d[k], (h[k] - a[k]) / d[k]);
                t0 = t0.max(u.min(v));
                t1 = t1.min(u.max(v));
            }
            if t0 >= t1 {
                continue;
            }
            for t in [t0, (t0 + t1) * 0.5, t1] {
                let q = a + d * t;
                let (k, depth) = (0..3)
                    .map(|k| (k, h[k] - q[k].abs()))
                    .min_by(|x, y| x.1.total_cmp(&y.1))
                    .unwrap();
                if depth <= SKIN {
                    continue;
                }
                let n_box = axes[k] * q[k].signum();
                let p = c + axes[0] * q.x + axes[1] * q.y + axes[2] * q.z;
                out.push((p + n_box * depth, -n_box, -depth));
            }
        }
    }
}

/// The least normal z of a triangle [`box_contacts`] meets from behind
/// when the box's centre lies off its edges (vcod's).
const FLOOR_NORMAL_Z: f32 = 0.7;

/// Whether `p` lies within the edges of `poly` (in order round `n`),
/// seen along `n`.
fn inside_edges(p: Vec3, n: Vec3, poly: &[Vec3]) -> bool {
    (0..poly.len()).all(|i| {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        n.cross(b - a).dot(p - a) >= 0.0
    })
}

/// A solid's face `(n, d, poly)` as the reference (0x59a2b0): the box's
/// face most against it, clipped to the inside of the face's edges, each
/// point within `margin` of the face a contact.
fn face_reference(
    c: Vec3,
    axes: &[Vec3; 3],
    h: Vec3,
    (n, d, poly): &(Vec3, f32, Vec<Vec3>),
    margin: f32,
    out: &mut Vec<(Vec3, Vec3, f32)>,
) {
    let (k, sign) = (0..3)
        .flat_map(|k| [(k, 1.0f32), (k, -1.0)])
        .min_by(|x, y| {
            (axes[x.0] * x.1)
                .dot(*n)
                .total_cmp(&(axes[y.0] * y.1).dot(*n))
        })
        .unwrap();
    let (u, v) = (axes[(k + 1) % 3], axes[(k + 2) % 3]);
    let (hu, hv) = (h[(k + 1) % 3], h[(k + 2) % 3]);
    let fc = c + axes[k] * (h[k] * sign);
    let quad = vec![
        fc + u * hu + v * hv,
        fc - u * hu + v * hv,
        fc - u * hu - v * hv,
        fc + u * hu - v * hv,
    ];
    let edges = (0..poly.len()).map(|i| {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let inward = n.cross(q - p);
        (inward, inward.dot(p))
    });
    for p in clip_polygon(quad, edges) {
        let sep = n.dot(p) - d;
        if sep <= margin {
            out.push((p, *n, sep));
        }
    }
}

/// A cylinder's reach along `l` from its centre (0x5a11f0, 0x59f5d0): its
/// half height times `|a . l|` and its radius times the sine.
fn cylinder_reach(cy: &ShapeCylinder, l: Vec3) -> f32 {
    let c = cy.axis.dot(l).abs().min(1.0);
    cy.half * c + cy.radius * (1.0 - c * c).sqrt()
}

/// A cylinder against `s`, as the fx world's cylinder colliders do it
/// (iw3mp.exe 0x5a1330 against a brush, 0x5a17c0 against a partition's
/// triangles, cod4-gsc.md 3.6.1). A brush: each face's depth is the
/// cylinder's centre off its plane less the cylinder's reach along it, or
/// none when the centre and the body's centre `com` both lie behind it;
/// a face the cylinder clears by `margin` keeps it out; the least deep of
/// the rest is the reference, and a `narrow` cylinder also takes every
/// other face its centre lies in front of. A triangle is its own
/// reference. Each reference goes to [`cylinder_polygon`]. Pushes
/// `(point on the cylinder, normal out of the solid, separation)`.
fn cylinder_contacts(
    cy: &ShapeCylinder,
    com: Vec3,
    s: &Solid,
    margin: f32,
    narrow: bool,
    out: &mut Vec<(Vec3, Vec3, f32)>,
) {
    if s.open {
        cylinder_polygon(cy, com, &s.faces[0], margin, out);
        return;
    }
    let mut best: Option<(f32, usize)> = None;
    for (i, (n, d, _)) in s.faces.iter().enumerate() {
        let off = n.dot(cy.centre) - d;
        if off < 0.0 && n.dot(com) - d < 0.0 {
            continue;
        }
        let depth = off - cylinder_reach(cy, *n);
        if depth >= margin {
            return;
        }
        if best.is_none_or(|(b, _)| depth > b) {
            best = Some((depth, i));
        }
    }
    let Some((_, reference)) = best else {
        return;
    };
    cylinder_polygon(cy, com, &s.faces[reference], margin, out);
    // Only for a brush 0x5a0f70's gate lets meet the cylinder: a stock
    // car wheel launched with its centre on its car's clip's floor face
    // took that clip's edges, inside it, as contacts.
    let (n, d, _) = &s.faces[reference];
    if n.dot(cy.centre) > *d || n.dot(com) > *d {
        cylinder_edge_contacts(cy, s, reference, out);
    }
    if !narrow {
        return;
    }
    for (i, f) in s.faces.iter().enumerate() {
        if i != reference && f.0.dot(cy.centre) > f.1 {
            cylinder_polygon(cy, com, f, margin, out);
        }
    }
}

/// [`edge_contacts`] for a cylinder against a brush: each edge of its
/// faces but the reference's where it runs more than the skin inside the cylinder
/// pushes it out at the inside part's ends and middle, along the cap or
/// the side nearest the point. A stock junk tire tipped onto a step's
/// edge slept with the edge a unit into its cap without it. vcod's.
fn cylinder_edge_contacts(
    cy: &ShapeCylinder,
    s: &Solid,
    taken: usize,
    out: &mut Vec<(Vec3, Vec3, f32)>,
) {
    let (c, a, r, hh) = (cy.centre, cy.axis, cy.radius, cy.half);
    // Height along the axis and the part across it.
    let split = |p: Vec3| {
        let w = p - c;
        let h = w.dot(a);
        (h, w - a * h)
    };
    for (_, _, poly) in s
        .faces
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != taken)
        .map(|(_, f)| f)
    {
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            let ((hp, fp), (hq, fq)) = (split(p), split(q));
            // The segment within the caps' slab, as fractions along it.
            let (mut t0, mut t1) = (0.0f32, 1.0f32);
            let dh = hq - hp;
            if dh.abs() < 1e-6 {
                if hp.abs() > hh {
                    continue;
                }
            } else {
                let (u, v) = ((-hh - hp) / dh, (hh - hp) / dh);
                t0 = t0.max(u.min(v));
                t1 = t1.min(u.max(v));
            }
            // And within the radius.
            let fd = fq - fp;
            let (qa, qb, qc) = (fd.dot(fd), 2.0 * fp.dot(fd), fp.dot(fp) - r * r);
            if qa < 1e-12 {
                if qc > 0.0 {
                    continue;
                }
            } else {
                let disc = qb * qb - 4.0 * qa * qc;
                if disc < 0.0 {
                    continue;
                }
                let root = disc.sqrt();
                t0 = t0.max((-qb - root) / (2.0 * qa));
                t1 = t1.min((-qb + root) / (2.0 * qa));
            }
            if t0 >= t1 {
                continue;
            }
            for t in [t0, (t0 + t1) * 0.5, t1] {
                let x = p + (q - p) * t;
                let (h, f) = split(x);
                let (side, cap) = (r - f.length(), hh - h.abs());
                let (n_cyl, depth) = if cap <= side {
                    (a * h.signum(), cap)
                } else {
                    (f.normalize_or(a.any_orthonormal_vector()), side)
                };
                if depth <= SKIN {
                    continue;
                }
                out.push((x + n_cyl * depth, -n_cyl, -depth));
            }
        }
    }
}

/// What [`cylinder_polygon`]'s separating axis test found: the least
/// overlap, its feature (iw3mp.exe 0x59fc90's codes) and its axis, from
/// the cylinder into the solid.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Feature {
    Face,
    Axis,
    Edge(usize),
    Vertex(usize),
    Cap(usize),
}

/// A cylinder against one face `(n, d, poly)` of a solid (iw3mp.exe
/// 0x5a0f70): nothing unless the cylinder's centre or the body's centre
/// lies in front of the plane; then the separating axes of 0x59fc90, each
/// turned to point into the solid (`l . n <= 0`), the overlap along each
/// the cylinder's reach less the polygon's nearest corner: the face's
/// normal, the cylinder's axis, each edge across the axis (unless the axis
/// lies within 0.02 of the plane), the line from the axis to each corner,
/// and the line from each cap's rim to each corner, which must overlap by
/// a unit less to win (0x59f9f0's bias of 1). An axis the polygon clears
/// by `margin` keeps it out. The contacts come off the least overlap's
/// feature. vcod's, past the axes: which edge axes may win, and each
/// feature's points ([`cylinder_feature`]).
fn cylinder_polygon(
    cy: &ShapeCylinder,
    com: Vec3,
    face: &(Vec3, f32, Vec<Vec3>),
    margin: f32,
    out: &mut Vec<(Vec3, Vec3, f32)>,
) {
    let (n, d, poly) = (face.0, face.1, &face.2);
    if poly.len() < 3 || (n.dot(cy.centre) <= d && n.dot(com) <= d) {
        return;
    }
    let (c, a) = (cy.centre, cy.axis);
    let mut best: Option<(f32, Feature, Vec3)> = None;
    // false when the polygon clears the cylinder along `l`.
    let mut test = |l: Vec3, feature: Feature, bias: f32, record: bool| -> bool {
        let Some(l) = l.try_normalize() else {
            return true;
        };
        let l = if l.dot(n) > 0.0 { -l } else { l };
        let reach = cylinder_reach(cy, l);
        let (lo, hi) = poly
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                let x = (*v - c).dot(l);
                (lo.min(x), hi.max(x))
            });
        if lo > reach + margin || hi < -reach - margin {
            return false;
        }
        let overlap = reach - lo;
        if record && best.is_none_or(|(b, _, _)| overlap + bias < b) {
            best = Some((overlap, feature, l));
        }
        true
    };
    if !test(-n, Feature::Face, 0.0, true) || !test(a, Feature::Axis, 0.0, true) {
        return;
    }
    let edges = (0..poly.len()).map(|i| (poly[i], poly[(i + 1) % poly.len()]));
    if a.dot(n).abs() > 0.02 {
        for (i, (p, q)) in edges.clone().enumerate() {
            // An edge's axis wins only with the centre outside its edge
            // (vcod's reading of 0x59fc90's sign test).
            let outside = (q - p).cross(n).dot(c - p) > 0.0;
            if !test(a.cross(q - p), Feature::Edge(i), 0.0, outside) {
                return;
            }
        }
    }
    for (i, v) in poly.iter().enumerate() {
        let w = *v - c;
        if !test(w - a * w.dot(a), Feature::Vertex(i), 0.0, true) {
            return;
        }
    }
    for (i, v) in poly.iter().enumerate() {
        for end in [cy.half, -cy.half] {
            let cap = c + a * end;
            let w = *v - cap;
            let Some(out) = (w - a * w.dot(a)).try_normalize() else {
                continue;
            };
            if !test(*v - (cap + out * cy.radius), Feature::Cap(i), 1.0, true) {
                return;
            }
        }
    }
    if let Some((overlap, feature, l)) = best {
        cylinder_feature(cy, face, feature, overlap, l, margin, out);
    }
}

/// The contacts off [`cylinder_polygon`]'s feature, normal `-l`, each
/// point on the cylinder (iw3mp.exe 0x5a0f70's arms):
/// - the face, the axis more than 0.9 off the normal (0x5a02b0): the
///   cylinder's side line nearest the face, clipped to its edges, each end
///   within `margin` of the plane;
/// - the face, the axis within it (0x5a09a0): the cap facing the face's
///   rim, as [`RING`] points, clipped to its edges, each within `margin`;
/// - the axis (0x59f460): the polygon clipped to the cylinder's circle
///   (0x59f170), each point the facing cap reaches;
/// - an edge (0x5a0ef0): its point nearest the axis;
/// - a corner or a cap's rim against a corner (0x5973e0, 0x5a0740): the
///   corner.
///
/// The arms' choice is 0x5a0f70's; the rim's points and the cap's
/// corner as one contact are vcod's.
fn cylinder_feature(
    cy: &ShapeCylinder,
    (n, d, poly): &(Vec3, f32, Vec<Vec3>),
    feature: Feature,
    overlap: f32,
    l: Vec3,
    margin: f32,
    out: &mut Vec<(Vec3, Vec3, f32)>,
) {
    let (c, a, r, hh) = (cy.centre, cy.axis, cy.radius, cy.half);
    let normal = -l;
    let inward = |p: Vec3, q: Vec3| {
        let i = n.cross(q - p);
        (i, i.dot(p))
    };
    let edges = (0..poly.len()).map(|i| inward(poly[i], poly[(i + 1) % poly.len()]));
    let on_plane = |p: Vec3, out: &mut Vec<(Vec3, Vec3, f32)>| {
        let sep = n.dot(p) - d;
        if sep <= margin {
            out.push((p, *n, sep));
        }
    };
    match feature {
        Feature::Face if a.dot(*n).abs() < 0.9 => {
            let Some(u) = (l - a * l.dot(a)).try_normalize() else {
                return;
            };
            let side = c + u * r;
            let ends = edges.fold(vec![side + a * hh, side - a * hh], |s, e| {
                clip_segment(&s, e)
            });
            for p in ends {
                on_plane(p, out);
            }
        }
        Feature::Face => {
            let cap = if a.dot(*n) > 0.0 {
                c - a * hh
            } else {
                c + a * hh
            };
            let t0 = a.any_orthonormal_vector();
            let t1 = a.cross(t0);
            let ring: Vec<Vec3> = (0..RING)
                .map(|i| {
                    let t = i as f32 * std::f32::consts::TAU / RING as f32;
                    cap + (t0 * t.cos() + t1 * t.sin()) * r
                })
                .collect();
            for p in clip_polygon(ring, edges) {
                on_plane(p, out);
            }
        }
        Feature::Axis => {
            // The cap the polygon lies past.
            let up = if l.dot(a) > 0.0 { 1.0 } else { -1.0 };
            for p in clip_to_circle(poly, c, a, r) {
                let h = (p - c).dot(a) * up;
                let depth = hh - h;
                if -depth <= margin {
                    out.push((p + normal * -depth, normal, -depth));
                }
            }
        }
        Feature::Edge(i) => {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            let at = segment_nearest(p, q, c - a * hh, c + a * hh);
            out.push((at + normal * -overlap, normal, -overlap));
        }
        Feature::Vertex(i) | Feature::Cap(i) => {
            out.push((poly[i] + normal * -overlap, normal, -overlap));
        }
    }
}

/// The segment `ends` (two points, or none) clipped to the side of `(n,
/// d)` where `n . p >= d`.
fn clip_segment(ends: &[Vec3], (n, d): (Vec3, f32)) -> Vec<Vec3> {
    let [a, b] = ends else {
        return Vec::new();
    };
    let (da, db) = (n.dot(*a) - d, n.dot(*b) - d);
    match (da >= 0.0, db >= 0.0) {
        (true, true) => vec![*a, *b],
        (false, false) => Vec::new(),
        (true, false) => vec![*a, *a + (*b - *a) * (da / (da - db))],
        (false, true) => vec![*a + (*b - *a) * (da / (da - db)), *b],
    }
}

/// `poly` cut to the inside of the infinite cylinder round `c` along `a`
/// of radius `r` (0x59f170): its corners inside, and where its edges
/// cross the surface.
fn clip_to_circle(poly: &[Vec3], c: Vec3, a: Vec3, r: f32) -> Vec<Vec3> {
    let flat = |p: Vec3| {
        let w = p - c;
        w - a * w.dot(a)
    };
    let inside = |p: Vec3| flat(p).length_squared() <= r * r;
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        if inside(p) {
            out.push(p);
        }
        // |fp + t (fq - fp)| = r.
        let (fp, fd) = (flat(p), flat(q) - flat(p));
        let (qa, qb, qc) = (fd.dot(fd), 2.0 * fp.dot(fd), fp.dot(fp) - r * r);
        let disc = qb * qb - 4.0 * qa * qc;
        if qa < 1e-12 || disc < 0.0 {
            continue;
        }
        let root = disc.sqrt();
        let mut ts = [(-qb - root) / (2.0 * qa), (-qb + root) / (2.0 * qa)];
        ts.sort_by(f32::total_cmp);
        for t in ts {
            if t > 0.0 && t < 1.0 {
                out.push(p + (q - p) * t);
            }
        }
    }
    out
}

/// The point of segment `p..q` nearest segment `s..e`.
fn segment_nearest(p: Vec3, q: Vec3, s: Vec3, e: Vec3) -> Vec3 {
    let (d1, d2, w) = (q - p, e - s, p - s);
    let (a, b, c) = (d1.dot(d1), d1.dot(d2), d2.dot(d2));
    let (dd, ee) = (d1.dot(w), d2.dot(w));
    let den = a * c - b * b;
    let mut t = if den > 1e-9 {
        ((b * ee - c * dd) / den).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // The nearest point of `s..e` to that, then back onto `p..q`.
    let u = if c > 1e-9 {
        ((b * t + ee) / c).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if a > 1e-9 {
        t = ((u * b - dd) / a).clamp(0.0, 1.0);
    }
    p + d1 * t
}

/// `msecStep`'s least and most per world (the tables at 0x71e7a4 and
/// 0x71e7b0): dynamic entities' (0), launched parts' and pieces' (1),
/// ragdolls' (2).
const MSEC_STEP: [(i32, i32); 3] = [(17, 67), (17, 67), (17, 34)];

/// The `msecStep` world `world` takes after a step that leaves `awake` of
/// its bodies awake (0x5a6640, which counts them through 0x5a62f0 after
/// the step, 0x5a6738..0x5a67bb): the least up to 32, then the span times
/// the count past 32 over 18, at most 1, truncated (`_ftol2`, 0x68b360).
pub fn msec_step(world: usize, awake: usize) -> i32 {
    let (lo, hi) = MSEC_STEP[world];
    let f = ((awake as f32 - 32.0) / 18.0).clamp(0.0, 1.0);
    lo + (f * (hi - lo) as f32) as i32
}

/// One physics world's clock, which steps all its bodies together
/// (`Phys_RunFrame`, 0x5a6e02): up to a frame's time in at most two
/// steps, each the larger of `msecStep` and half what is left, `msecStep`
/// picked after each step off the bodies awake ([`msec_step`]).
#[derive(Clone, Copy, Debug)]
pub struct WorldClock {
    world: usize,
    /// The time simulated to; `None` until the first body ([`Self::start`]).
    sim_ms: Option<i32>,
    pub step_ms: i32,
}

impl WorldClock {
    pub fn new(world: usize) -> WorldClock {
        WorldClock {
            world,
            sim_ms: None,
            step_ms: MSEC_STEP[world].0,
        }
    }

    /// A body is made at `now`: a clock not yet running starts there.
    pub fn start(&mut self, now: i32) {
        self.sim_ms.get_or_insert(now);
    }

    /// The world to `time`. `step(ms, until, first, dvars)` steps each
    /// body `ms` to `until` ([`Body::step_world`]), its frame begun when
    /// `first` ([`Body::begin_frame`]), under `d` with this world's
    /// `msecStep`, and returns how many are awake after it. Whether it
    /// stepped: then each body's frame ends ([`Body::end_frame`]).
    pub fn advance(
        &mut self,
        time: i32,
        d: &Dvars,
        mut step: impl FnMut(i32, i32, bool, &Dvars) -> usize,
    ) -> bool {
        let Some(mut sim) = self.sim_ms else {
            return false;
        };
        let stepped = time.wrapping_sub(sim) > 0;
        let mut left = 2;
        let mut first = true;
        while time.wrapping_sub(sim) > 0 {
            let ms = (time.wrapping_sub(sim) / left).max(self.step_ms);
            left = (left - 1).max(1);
            sim = sim.wrapping_add(ms);
            let dv = Dvars {
                step_ms: self.step_ms,
                ..*d
            };
            let awake = step(ms, sim, first, &dv);
            first = false;
            self.step_ms = msec_step(self.world, awake);
        }
        self.sim_ms = Some(sim);
        stepped
    }
}

/// One corner's contact with the world in a step.
#[derive(Clone)]
struct Contact {
    /// The corner off the centre of mass, world axes.
    r: Vec3,
    n: Vec3,
    /// What the contact's normal speed is pushed to at least.
    target: f32,
    k_n: f32,
    t: [Vec3; 2],
    k_t: [f32; 2],
    /// The accumulated normal and friction impulses.
    lambda: f32,
    lambda_t: [f32; 2],
    /// How far off the surface it starts (below 0 inside).
    sep: f32,
    /// One of the body's points' (`Body::gather`), not a geom collider's.
    point: bool,
    /// The geom it is the body's contact for (`Shape::point_geoms`).
    geom: usize,
}

/// One launched part.
#[derive(Clone, Debug)]
pub struct Body {
    /// Centre of mass and rotation; linear and angular velocity, world.
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub avel: Vec3,
    /// The force and torque the pushes since the last step left (ODE's
    /// accumulators at body +0x98 and +0xa8), spent over the next step.
    force: Vec3,
    torque: Vec3,
    mass: f32,
    /// Inertia and its inverse about the centre of mass, body axes.
    inertia: Mat3,
    inv_inertia: Mat3,
    /// The centre of mass in the model's frame, and the points, boxes and
    /// cylinders off it.
    com: Vec3,
    points: Vec<Vec3>,
    boxes: Vec<ShapeBox>,
    cylinders: Vec<ShapeCylinder>,
    point_geoms: Vec<usize>,
    box_geoms: Vec<usize>,
    cylinder_geoms: Vec<usize>,
    sweep_half: f32,
    /// The state at body +0x4c the post-step sweep reads (cod4-gsc.md
    /// 3.6.1): 0 new, 1 started solid or pinched, 2 neither.
    state: u8,
    bounce: f32,
    friction: f32,
    /// Seconds below both sleep speeds.
    idle: f32,
    pub asleep: bool,
    /// `Phys_RunFrame`'s clocks: the time simulated to, and the frame's
    /// start, whose state ([`Self::from`]) the drawn pose lerps from.
    sim_ms: i32,
    from_ms: i32,
    from: (Vec3, Quat),
    /// Steps taken.
    pub steps: u32,
    /// Of them, those whose contact pass met the body in state 1, pinched
    /// or started solid ([`unwedge`]).
    pub pinched: u32,
    /// The jitter regions its next step's contacts take, and the random
    /// state their pushes draw from (`rand`, 0x5a2536).
    pub jitter: Vec<Jitter>,
    pub jitter_rand: u32,
}

impl Body {
    /// `Phys_ObjCreate` (0x5a4fb0) at the entity's `origin` and `angles`
    /// (its `trBase`s), at rest, at time `now`.
    pub fn new(origin: Vec3, angles: Vec3, shape: &Shape, preset: &Preset, now: i32) -> Body {
        Body::at(origin, angles_quat(angles.to_array()), shape, preset, now)
    }

    /// [`Self::new`] turned by `rot`: a dynamic entity's pose
    /// (`DynEntCl_CreatePhysObj`, iw3mp.exe 0x48dca0).
    pub fn at(origin: Vec3, rot: Quat, shape: &Shape, preset: &Preset, now: i32) -> Body {
        let mass = preset.mass.max(0.001);
        let inertia = shape.inertia * mass;
        let pos = origin + rot * shape.com;
        Body {
            pos,
            rot,
            vel: Vec3::ZERO,
            avel: Vec3::ZERO,
            force: Vec3::ZERO,
            torque: Vec3::ZERO,
            mass,
            inertia,
            inv_inertia: inertia.inverse(),
            com: shape.com,
            points: shape.points.iter().map(|p| *p - shape.com).collect(),
            boxes: shape
                .boxes
                .iter()
                .map(|b| ShapeBox::new(b.centre - shape.com, b.axes, b.half))
                .collect(),
            cylinders: shape
                .cylinders
                .iter()
                .map(|c| ShapeCylinder {
                    centre: c.centre - shape.com,
                    ..*c
                })
                .collect(),
            point_geoms: shape.point_geoms.clone(),
            box_geoms: shape.box_geoms.clone(),
            cylinder_geoms: shape.cylinder_geoms.clone(),
            sweep_half: shape.sweep_half,
            state: 0,
            bounce: preset.bounce.min(1.0),
            friction: vcod_common::fguard::finite(preset.friction).max(0.0),
            idle: 0.0,
            asleep: false,
            sim_ms: now,
            from_ms: now,
            from: (pos, rot),
            steps: 0,
            pinched: 0,
            jitter: Vec::new(),
            jitter_rand: 0,
        }
    }

    /// `Phys_ObjBulletImpact` (0x5a5a20) as `CG_CreatePhysicsObject` calls
    /// it with the launch: `force`'s direction tipped up by
    /// `phys_bulletUpBias`, its length the speed, at `contact` moved out
    /// from the centre `phys_bulletSpinScale` times as far again, scaled by
    /// the preset's `bulletForceScale`, pushed ([`Self::push`]) at the
    /// `msecStep` of the world the caller names.
    pub fn launch(&mut self, contact: Vec3, force: Vec3, scale: f32, msec: i32, d: &Dvars) {
        let speed = force.length();
        let dir = (force.normalize_or_zero() + Vec3::Z * d.bullet_up_bias).normalize_or_zero();
        let at = contact + (contact - self.pos) * d.bullet_spin_scale;
        let r = at - self.pos;
        let c = r.cross(dir);
        let arm = c.length();
        let c = c.normalize_or_zero();
        let closing = speed - (self.vel.dot(dir) + self.avel.dot(c) * arm);
        // A NaN closes nothing either.
        if closing.is_nan() || closing <= 0.0 {
            return;
        }
        // 0x5a5bc3: the axis is turned by the body's rotation, not into
        // its frame, before the inertia reads it.
        let u = self.rot * c;
        let mut denom = self.mass + 0.5;
        if arm != 0.0 {
            denom += 0.5 * self.mass * arm * arm / u.dot(self.inertia * u);
        }
        let j = closing * self.mass * scale / denom;
        self.push(at, dir * j, msec);
    }

    /// `Phys_ObjAddForce` (0x5a5900): `p` times `1000 / msec`, `msec` the
    /// `msecStep` of the world the caller names (0x71e798), onto the
    /// body's force at world point `at` (`dBodyAddForceAtPos` 0x5b7000, its
    /// torque taken off the centre now), and the body woken. The next step
    /// of the body's own world spends it ([`Self::integrate`]), so the
    /// impulse is `p` times that step's length over `msec`.
    pub fn push(&mut self, at: Vec3, p: Vec3, msec: i32) {
        let f = p * (1000.0 / f64::from(msec)) as f32;
        self.force += f;
        self.torque += (at - self.pos).cross(f);
        self.wake();
    }

    /// The pushes' force over a step of `dt` seconds, which goes on with
    /// gravity ahead of the solve (ODE's quickstep, 0x5b9187), then zeroed
    /// (0x5a666b, after the step).
    pub(crate) fn spend_force(&mut self, dt: f32) {
        if self.force != Vec3::ZERO || self.torque != Vec3::ZERO {
            self.vel += self.force * (dt / self.mass);
            self.avel += self.inv_inertia_world() * (self.torque * dt);
            (self.force, self.torque) = (Vec3::ZERO, Vec3::ZERO);
        }
    }

    /// The force the pushes since the last step left, which the next step
    /// spends.
    pub fn force(&self) -> Vec3 {
        self.force
    }

    fn wake(&mut self) {
        self.asleep = false;
        self.idle = 0.0;
    }

    /// The world inverse inertia.
    fn inv_inertia_world(&self) -> Mat3 {
        let r = Mat3::from_quat(self.rot);
        r * self.inv_inertia * r.transpose()
    }

    fn impulse(&mut self, r: Vec3, p: Vec3) {
        self.vel += p / self.mass;
        self.avel += self.inv_inertia_world() * r.cross(p);
    }

    /// `Phys_RunFrame`'s clock (0x5a6e02): up to `time`, in at most two
    /// steps of at least `step_ms`, each at least half what is left, so
    /// the body runs up to a step ahead and [`Self::pose`] lerps.
    pub fn advance(&mut self, time: i32, trace: &mut Trace, d: &Dvars) {
        self.advance_in(time, trace, &mut |_, _| Vec::new(), d);
    }

    /// [`Self::advance`] with the body's boxes meeting `solids` as well.
    pub fn advance_in(&mut self, time: i32, trace: &mut Trace, solids: &mut Solids, d: &Dvars) {
        if time.wrapping_sub(self.sim_ms) <= 0 {
            return;
        }
        self.begin_frame();
        let mut left = 2;
        while time.wrapping_sub(self.sim_ms) > 0 {
            let step = (time.wrapping_sub(self.sim_ms) / left).max(d.step_ms);
            left = (left - 1).max(1);
            self.integrate(step as f32 / 1000.0, trace, solids, d);
            self.sim_ms = self.sim_ms.wrapping_add(step);
        }
        self.end_frame(trace, d);
    }

    /// A world's frame begins ([`WorldClock::advance`]): the drawn pose
    /// lerps from here, and the post-step sweep starts here (the copy at
    /// 0x5a6da0, once a frame before its steps).
    pub fn begin_frame(&mut self) {
        self.from_ms = self.sim_ms;
        self.from = (self.pos, self.rot);
    }

    /// A world's frame ends: a body awake takes the post-step sweep from
    /// where the frame began ([`Self::sweep`]; 0x5a6c10 on each body after
    /// `Phys_RunToTime`'s steps, 0x5a6e77).
    pub fn end_frame(&mut self, trace: &mut Trace, d: &Dvars) {
        if !self.asleep {
            self.sweep(self.from.0, trace, d);
        }
    }

    /// One step of its world, `ms` long, to `until`.
    pub fn step_world(
        &mut self,
        ms: i32,
        until: i32,
        trace: &mut Trace,
        solids: &mut Solids,
        d: &Dvars,
    ) {
        self.integrate(ms as f32 / 1000.0, trace, solids, d);
        self.sim_ms = until;
    }

    /// The drawn origin and angles at `time`: the frame's start state
    /// lerped toward the last step's.
    pub fn pose(&self, time: i32) -> (Vec3, Vec3) {
        let (origin, rot) = self.pose_quat(time);
        (origin, Vec3::from(quat_angles(rot)))
    }

    /// [`Self::pose`] as the model's origin and rotation.
    pub fn pose_quat(&self, time: i32) -> (Vec3, Quat) {
        let span = self.sim_ms.wrapping_sub(self.from_ms);
        let f = if span > 0 {
            (time.wrapping_sub(self.from_ms) as f32 / span as f32).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let pos = self.from.0.lerp(self.pos, f);
        let rot = self.from.1.slerp(self.rot, f).normalize();
        (pos - rot * self.com, rot)
    }

    /// The model's origin now.
    pub fn origin(&self) -> Vec3 {
        self.pos - self.rot * self.com
    }

    /// Every collision point in world space.
    pub fn world_points(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.points.iter().map(|p| self.pos + self.rot * *p)
    }

    /// Every box geom in world space.
    pub fn world_boxes(&self) -> impl Iterator<Item = ShapeBox> + '_ {
        let r = Mat3::from_quat(self.rot);
        self.boxes
            .iter()
            .map(move |b| ShapeBox::new(self.pos + self.rot * b.centre, r * b.axes, b.half))
    }

    /// Every cylinder geom in world space.
    pub fn world_cylinders(&self) -> impl Iterator<Item = ShapeCylinder> + '_ {
        self.cylinders.iter().map(|c| ShapeCylinder {
            centre: self.pos + self.rot * c.centre,
            axis: self.rot * c.axis,
            ..*c
        })
    }

    /// A frame of one step of `dt` seconds: gravity, the corners' contacts
    /// solved as impulses with Coulomb friction and the preset's bounce,
    /// the move, the drag, the sleep test, then the post-step sweep.
    pub fn step(&mut self, dt: f32, trace: &mut Trace, d: &Dvars) {
        self.step_in(dt, trace, &mut |_, _| Vec::new(), d);
    }

    /// [`Self::step`] with the boxes' contacts with `solids` among the
    /// points'.
    pub fn step_in(&mut self, dt: f32, trace: &mut Trace, solids: &mut Solids, d: &Dvars) {
        let old = self.pos;
        self.integrate(dt, trace, solids, d);
        if !self.asleep {
            self.sweep(old, trace, d);
        }
    }

    /// One step of its world, the sweep left for the frame's end.
    fn integrate(&mut self, dt: f32, trace: &mut Trace, solids: &mut Solids, d: &Dvars) {
        if self.asleep {
            self.jitter.clear();
            return;
        }
        self.steps += 1;
        let (v0, w0) = (self.vel, self.avel);
        self.vel.z += d.gravity * dt;
        self.spend_force(dt);
        let mut contacts: Vec<Contact> = Vec::new();
        let mut touched = vec![false; self.points.len()];
        for pass in 0..2 {
            let mut found = self.gather(dt, trace, d, v0, w0, &mut touched);
            if found.is_empty() && pass > 0 {
                break;
            }
            if pass == 0 {
                let mut all = self.box_contacts(dt, solids, d, v0, w0);
                all.append(&mut found);
                let all = self.contact_pass(all, d, v0, w0, dt);
                contacts.extend(all);
            } else {
                if self.state == 1 {
                    drop_separating(&mut found, v0, w0);
                }
                contacts.append(&mut found);
            }
            self.solve(&mut contacts, d);
        }
        self.pos += self.vel * dt;
        let w = self.avel;
        let dq = Quat::from_xyzw(w.x, w.y, w.z, 0.0) * self.rot;
        self.rot = (self.rot + dq * (0.5 * dt)).normalize();
        self.vel *= (1.0 - d.drag_linear * dt).max(0.0);
        self.avel *= (1.0 - d.drag_angular * dt).max(0.0);
        if self.vel.length_squared() < d.sleep_linear * d.sleep_linear
            && self.avel.length_squared() < d.sleep_angular * d.sleep_angular
        {
            self.idle += dt;
            if self.idle > d.sleep_time {
                self.asleep = true;
                self.vel = Vec3::ZERO;
                self.avel = Vec3::ZERO;
            }
        } else {
            self.idle = 0.0;
        }
    }

    /// The near callback's contact pass (0x5a5f20) on each geom against
    /// the world in the order 0x5aa100 walks the world's space: on the
    /// body's first step the last geom first, as each geom made goes in at
    /// the space's head (0x5a9f00), and on every later step the first geom
    /// first, as 0x5a6640 moves each geom of the body's list (the last made
    /// first) to the head after each step (0x5a9f80). A geom with no
    /// contact sets the state 2; five or more
    /// go through [`reduce`] first (0x5a2320); then 0x5a2760: [`pinch`],
    /// on a body in state 1 [`unwedge`], and the jitter on each contact
    /// kept ([`Self::shake`]). Retail's contacts are ODE's overlaps; vcod's
    /// stand-ins are the geom's in the skin or inside ([`touching`]): its
    /// box's or cylinder's, or a brush with sides' points. The rest are
    /// vcod's, a box's or cylinder's points and the contacts met further on
    /// within the step's travel, and pass through after the geom's. `dt` is
    /// the step's length, over which the jitter's force goes on.
    fn contact_pass(
        &mut self,
        all: Vec<Contact>,
        d: &Dvars,
        v0: Vec3,
        w0: Vec3,
        dt: f32,
    ) -> Vec<Contact> {
        let geoms = [&self.point_geoms, &self.box_geoms, &self.cylinder_geoms]
            .iter()
            .flat_map(|g| g.iter().copied())
            .max()
            .map_or(1, |g| g + 1);
        let regions = std::mem::take(&mut self.jitter);
        let mut pinched = false;
        let mut out = Vec::with_capacity(all.len());
        let first = self.steps <= 1;
        let order: Vec<usize> = if first {
            (0..geoms).rev().collect()
        } else {
            (0..geoms).collect()
        };
        for g in order {
            // A geom with a box or cylinder collider: its points are vcod's.
            let collider = self.box_geoms.contains(&g) || self.cylinder_geoms.contains(&g);
            let (mut touch, ahead): (Vec<Contact>, Vec<Contact>) = all
                .iter()
                .filter(|c| c.geom == g)
                .cloned()
                .partition(|c| touching(c) && !(collider && c.point));
            if touch.is_empty() {
                self.state = 2;
            } else {
                if touch.len() >= 5 {
                    let geoms: Vec<_> = touch.iter().map(|c| (c.r, c.n, -c.sep)).collect();
                    let keep = reduce(&geoms);
                    touch = keep.into_iter().map(|i| touch[i].clone()).collect();
                }
                if pinch(&touch) {
                    self.state = 1;
                }
                if self.state == 1 {
                    pinched = true;
                    unwedge(&mut touch, v0, w0);
                }
                self.shake(&touch, &regions, d, dt);
            }
            out.extend(touch);
            out.extend(ahead);
        }
        if pinched {
            self.pinched += 1;
        }
        out
    }

    /// The post-step sweep (0x5a6930, from 0x5a6c10 after a frame's steps): when
    /// the centre moved, or the state is 0 or 1, the centre is traced from
    /// where the step started to where it ended, as a point (corners at
    /// 0.001) when the sweep half size `m` is under twice `phys_csl`, else
    /// as a box of 0.2 `m`, or of `m - 2 phys_csl` (at least 0.001) when
    /// 0.8 `m` is under it. The state becomes 1 for a start in solid, else
    /// 2; a hit not started solid puts the centre where the trace stopped,
    /// a point's fraction first backed off by `m / 2` over the move's
    /// length, and a fraction under 0.001 zeroes both velocities. Nothing
    /// else touches the speed (cod4-gsc.md 3.6.1).
    fn sweep(&mut self, old: Vec3, trace: &mut Trace, d: &Dvars) {
        if self.pos == old && self.state == 2 {
            return;
        }
        let (m, twice) = (self.sweep_half, 2.0 * d.csl);
        let point = m < twice;
        let ext = if point {
            0.001
        } else if m - 0.2 * m < twice {
            (m - twice).max(0.001)
        } else {
            0.2 * m
        };
        let s = trace(old, self.pos, ext);
        self.state = if s.startsolid { 1 } else { 2 };
        if s.startsolid || s.fraction >= 1.0 {
            return;
        }
        let mut f = s.fraction;
        if point {
            f = (f - 0.5 * m / (self.pos - old).length()).max(0.0);
        }
        self.pos = old + (self.pos - old) * f;
        if f < 0.001 {
            self.vel = Vec3::ZERO;
            self.avel = Vec3::ZERO;
        }
    }

    /// The jitter's pushes in the contact callback (0x5a23e0, from
    /// 0x5a2918), then the regions dropped (0x5a66ff): each contact whose
    /// normal z is at least 0.7, at a level distance `r` under a region's
    /// outer radius, takes `(min + u (max - min)) f m' (1 - (m' / 200)^3)`
    /// straight up through `Phys_ObjAddForce` (0x5a25a1), `u` a `rand()`
    /// over 32768, `f` 1 within the inner radius and
    /// `(outer - r) / (outer - inner)` past it, `m'` the mass capped at
    /// `phys_jitterMaxMass`.
    /// The value is times `1000 / msecStep` before `Phys_ObjAddForce`
    /// scales it again, and the force goes on over the step being collided
    /// (its length `dt`), so the impulse is the value times `dt` over
    /// `msecStep`'s seconds. It runs on each geom's contacts the contact
    /// pass keeps.
    fn shake(&mut self, contacts: &[Contact], regions: &[Jitter], d: &Dvars, dt: f32) {
        if regions.is_empty() {
            return;
        }
        let m = self.mass.min(JITTER_MAX_MASS);
        let q = m / JITTER_MAX_MASS;
        let weight = m * (1.0 - q * q * q);
        let per_step = (1000.0 / f64::from(d.step_ms)) as f32;
        for c in contacts.iter().filter(|c| c.n.z >= JITTER_MIN_NORMAL_Z) {
            let at = self.pos + c.r;
            for j in regions {
                let dist_sq = (at.x - j.origin.x).powi(2) + (at.y - j.origin.y).powi(2);
                if dist_sq >= j.outer * j.outer {
                    continue;
                }
                let dist = dist_sq.sqrt();
                let falloff = if j.inner < dist {
                    (j.outer - dist) / (j.outer - j.inner)
                } else {
                    1.0
                };
                self.jitter_rand = self
                    .jitter_rand
                    .wrapping_mul(0x343fd)
                    .wrapping_add(0x269ec3);
                let u = (self.jitter_rand >> 16 & 0x7fff) as f32 / 32768.0;
                let lift = (j.min + u * (j.max - j.min)) * falloff * weight * per_step;
                self.impulse(c.r, Vec3::Z * (lift * per_step * dt));
            }
        }
    }

    /// The contacts of the points not yet touching: each swept by its
    /// velocity over the step; one starting in the world is traced out
    /// from the centre.
    fn gather(
        &self,
        dt: f32,
        trace: &mut Trace,
        d: &Dvars,
        v0: Vec3,
        w0: Vec3,
        touched: &mut [bool],
    ) -> Vec<Contact> {
        let inv_iw = self.inv_inertia_world();
        let mut out = Vec::new();
        for (i, l) in self.points.iter().enumerate() {
            if touched[i] {
                continue;
            }
            let r = self.rot * *l;
            let p = self.pos + r;
            let travel = (self.vel + self.avel.cross(r)) * dt;
            let s = trace(p, p + travel, 0.0);
            let (n, sep) = if s.startsolid {
                let out = trace(self.pos, p, 0.0);
                if out.startsolid || out.fraction >= 1.0 {
                    continue;
                }
                let hit = self.pos + (p - self.pos) * out.fraction;
                (out.normal, (p - hit).dot(out.normal))
            } else if s.fraction < 1.0 {
                let hit = p + travel * s.fraction;
                (s.normal, (p - hit).dot(s.normal).max(0.0))
            } else {
                continue;
            };
            touched[i] = true;
            out.push(Contact {
                point: true,
                geom: self.point_geoms[i],
                ..self.contact(r, n, sep, dt, d, v0, w0, &inv_iw)
            });
        }
        out
    }

    /// The contacts of the boxes and cylinders with the solids round them:
    /// every point [`box_contacts`] and [`cylinder_contacts`] find within
    /// the step's reach, the reach the most any of the geom's points
    /// travels in the step and the trace's skin.
    fn box_contacts(
        &self,
        dt: f32,
        solids: &mut Solids,
        d: &Dvars,
        v0: Vec3,
        w0: Vec3,
    ) -> Vec<Contact> {
        let inv_iw = self.inv_inertia_world();
        let mut out = Vec::new();
        let mut found = Vec::new();
        for (b, &geom) in self.boxes.iter().zip(&self.box_geoms) {
            let axes = Mat3::from_quat(self.rot) * b.axes;
            let c = self.pos + self.rot * b.centre;
            let ext = axes.x_axis.abs() * b.half.x
                + axes.y_axis.abs() * b.half.y
                + axes.z_axis.abs() * b.half.z;
            let reach =
                (v0.length() + w0.length() * (b.centre.length() + b.half.length())) * dt + SKIN;
            found.clear();
            for s in solids(c - ext - Vec3::splat(reach), c + ext + Vec3::splat(reach)) {
                let narrow = b.half.min_element() * 2.0 < d.narrow_max_length;
                box_contacts(c, axes, b.half, &s, reach, narrow, &mut found);
            }
            for &(p, n, sep) in &found {
                out.push(Contact {
                    geom,
                    ..self.contact(p - self.pos, n, sep, dt, d, v0, w0, &inv_iw)
                });
            }
        }
        for (cy, &geom) in self.world_cylinders().zip(&self.cylinder_geoms) {
            let (a, r, hh) = (cy.axis, cy.radius, cy.half);
            let ext = a.abs() * hh + (Vec3::ONE - a * a).max(Vec3::ZERO).powf(0.5) * r;
            let lever = (cy.centre - self.pos).length() + hh.hypot(r);
            let reach = (v0.length() + w0.length() * lever) * dt + SKIN;
            // 0x5a8110's cylinder arm: narrow when its diameter is under
            // `phys_narrowObjMaxLength`.
            let narrow = 2.0 * r < d.narrow_max_length;
            found.clear();
            let pad = ext + Vec3::splat(reach);
            for s in solids(cy.centre - pad, cy.centre + pad) {
                cylinder_contacts(&cy, self.pos, &s, reach, narrow, &mut found);
            }
            for &(p, n, sep) in &found {
                out.push(Contact {
                    geom,
                    ..self.contact(p - self.pos, n, sep, dt, d, v0, w0, &inv_iw)
                });
            }
        }
        out
    }

    /// A contact at `r` off the centre, normal `n`, `sep` off the surface
    /// (below 0 inside): its normal speed held at a bounce by the preset
    /// when it closes fast enough to meet the surface this step, at what
    /// closes the gap otherwise, and inside pushed out at
    /// `phys_contact_erp` up to `phys_mcv`.
    #[allow(clippy::too_many_arguments)]
    fn contact(
        &self,
        r: Vec3,
        n: Vec3,
        sep: f32,
        dt: f32,
        d: &Dvars,
        v0: Vec3,
        w0: Vec3,
        inv_iw: &Mat3,
    ) -> Contact {
        let approach = (v0 + w0.cross(r)).dot(n);
        let target = if sep < 0.0 {
            (d.erp * -sep / dt).min(d.max_correcting)
        } else if approach < -BOUNCE_VEL && -approach * dt >= sep {
            -self.bounce * approach
        } else {
            -sep / dt
        };
        let k = |dir: Vec3| {
            let rn = r.cross(dir);
            1.0 / self.mass + rn.dot(*inv_iw * rn)
        };
        let t0 = n.any_orthonormal_vector();
        let t = [t0, n.cross(t0)];
        Contact {
            r,
            n,
            target,
            k_n: k(n),
            t,
            k_t: [k(t[0]), k(t[1])],
            lambda: 0.0,
            lambda_t: [0.0; 2],
            sep,
            point: false,
            geom: 0,
        }
    }

    /// Sequential impulses: each contact's normal speed held at or over its
    /// target, its friction within `friction * phys_frictionScale` of the
    /// normal impulse.
    fn solve(&mut self, contacts: &mut [Contact], d: &Dvars) {
        let mu = self.friction * d.friction_scale;
        for _ in 0..ITERATIONS {
            for c in contacts.iter_mut() {
                let r = c.r;
                let vrel = |b: &Body| b.vel + b.avel.cross(r);
                let vn = vrel(self).dot(c.n);
                let next = (c.lambda + (c.target - vn) / c.k_n).max(0.0);
                let dl = next - c.lambda;
                c.lambda = next;
                self.impulse(r, c.n * dl);
                for k in 0..2 {
                    let vt = vrel(self).dot(c.t[k]);
                    let cap = mu * c.lambda;
                    let next = vcod_common::fguard::clamp(c.lambda_t[k] - vt / c.k_t[k], -cap, cap);
                    let dl = next - c.lambda_t[k];
                    c.lambda_t[k] = next;
                    self.impulse(r, c.t[k] * dl);
                }
            }
        }
    }
}

/// The skin the clip map's kernels keep off a surface (`SURFACE_CLIP_EPSILON`).
const SKIN: f32 = 0.125;

/// Whether a contact stands for one of ODE's: in the skin the kernels
/// keep, or inside. The rest are vcod's, met within the step's travel.
fn touching(c: &Contact) -> bool {
    c.sep <= SKIN
}

/// 0x5a2280: whether two of a geom's contacts face each other
/// ([`FACING`]), which sets the body's state 1.
fn pinch(c: &[Contact]) -> bool {
    (0..c.len()).any(|i| (i + 1..c.len()).any(|j| c[i].n.dot(c[j].n) < FACING))
}

/// A pinched body's contacts with one geom (0x5a2760, the state 1): of two
/// that face each other ([`FACING`]) the first stays when the centre lies
/// on its plane or in front of it, else the second takes its slot, and the
/// second's slot takes the last contact, which the walk then passes over
/// (0x5a25d0, its point the body's position, as the world's byte at
/// 0xcc54498 is never set); then [`drop_separating`].
fn unwedge(c: &mut Vec<Contact>, v0: Vec3, w0: Vec3) {
    let mut i = 0;
    while i + 1 < c.len() {
        let mut j = i + 1;
        while j < c.len() {
            if c[i].n.dot(c[j].n) < FACING {
                // The centre is `-r` off the contact.
                if c[i].n.dot(-c[i].r) < 0.0 {
                    c.swap(i, j);
                }
                c.swap_remove(j);
            }
            j += 1;
        }
        i += 1;
    }
    drop_separating(c, v0, w0);
}

/// The contacts 0x5a2320 keeps of a geom's five or more (0x5a5f20 at
/// 0x5a6078), by index into `c` (each its position, normal and depth), in
/// the order it copies them out (cod4-gsc.md 3.6.1): three seeds by normal
/// (0x5a18e0), five k-means rounds over the normals (0x5a1aa0), clusters
/// whose means dot over 0.999 merged (0x5a1cc0), and of each cluster the
/// least and most along two tangents and the deepest, a contact within
/// 0.1 of an earlier one passed over (0x5a1e40).
fn reduce(c: &[(Vec3, Vec3, f32)]) -> Vec<usize> {
    let n = c.len();
    let mut tags = vec![usize::MAX; n];
    if n == 0 {
        return Vec::new();
    }
    // 0x5a18e0: the seed 104729 mod the count, the contact whose normal is
    // most against it, then the one most against both.
    let k0 = 104_729 % n;
    let least = |f: &dyn Fn(Vec3) -> f32| {
        let (mut best, mut at) = (f32::MAX, None);
        for (i, &(_, nm, _)) in c.iter().enumerate() {
            let d = f(nm);
            if best > d {
                (best, at) = (d, Some(i));
            }
        }
        at
    };
    let n0 = c[k0].1;
    let k1 = least(&|nm| nm.dot(n0));
    tags[k0] = 0;
    if let Some(k1) = k1 {
        tags[k1] = 1;
        let n1 = c[k1].1;
        if let Some(k2) = least(&|nm| nm.dot(n0).max(nm.dot(n1))) {
            tags[k2] = 2;
        }
    }
    // 0x5a1aa0: each cluster's mean normal (zero stays zero, 0x4076d0),
    // then each contact to the cluster it dots most with, five times over.
    let means = |tags: &[usize]| {
        let mut m = [Vec3::ZERO; 3];
        for (i, &t) in tags.iter().enumerate() {
            if t < 3 {
                m[t] += c[i].1;
            }
        }
        m.map(|v| v.normalize_or_zero())
    };
    let mut m = means(&tags);
    for _ in 0..5 {
        for (i, t) in tags.iter_mut().enumerate() {
            let (mut best, mut at) = (-f32::MAX, usize::MAX);
            for (k, mk) in m.iter().enumerate() {
                let d = mk.dot(c[i].1);
                if best - d < 0.0 {
                    (best, at) = (d, k);
                }
            }
            *t = at;
        }
        m = means(&tags);
    }
    // 0x5a1cc0: a later cluster whose mean dots over 0.999 (the double at
    // 0x70b270) with an earlier one's joins it, off the means as they were.
    for i in 0..2 {
        for j in i + 1..3 {
            if m[i].dot(m[j]) > 0.999 {
                tags.iter_mut().filter(|t| **t == j).for_each(|t| *t = i);
            }
        }
    }
    // 0x5a1e40: the clusters' normal sums, each normalized and given two
    // tangents off z, or off y when |z| is 0.5 or more (0x5a1d80).
    let mut sums = [Vec3::ZERO; 3];
    for (i, &t) in tags.iter().enumerate() {
        if t < 3 {
            sums[t] += c[i].1;
        }
    }
    let mut out = Vec::new();
    for (k, sum) in sums.iter().enumerate() {
        let nm = sum.normalize_or_zero();
        let u = if nm.z.abs() < 0.5 { Vec3::Z } else { Vec3::Y };
        let t1 = nm.cross(u).normalize_or_zero();
        let t = [t1, nm.cross(t1).normalize_or_zero()];
        let (mut lo, mut hi) = ([f32::MAX; 2], [-f32::MAX; 2]);
        let (mut lo_at, mut hi_at) = ([usize::MAX; 2], [usize::MAX; 2]);
        let (mut deepest, mut deep_at) = (-f32::MAX, usize::MAX);
        let mut members = 0;
        for i in 0..n {
            if tags[i] != k {
                continue;
            }
            // The float at 0x70b87c, 0.01, on the squared distance.
            for j in i + 1..n {
                if tags[j] == k && (c[j].0 - c[i].0).length_squared() < 0.01 {
                    tags[j] = usize::MAX;
                }
            }
            members += 1;
            for a in 0..2 {
                let p = t[a].dot(c[i].0);
                if lo[a] > p {
                    (lo[a], lo_at[a]) = (p, i);
                }
                if hi[a] < p {
                    (hi[a], hi_at[a]) = (p, i);
                }
            }
            if deepest < c[i].2 {
                (deepest, deep_at) = (c[i].2, i);
            }
        }
        if members == 0 {
            continue;
        }
        let start = out.len();
        for i in [lo_at[0], lo_at[1], hi_at[0], hi_at[1], deep_at] {
            if i < n && !out[start..].contains(&i) {
                out.push(i);
            }
        }
    }
    out
}

/// 0x5a2760's second filter on a pinched body: each [`touching`] contact
/// whose point's velocity at the collide (`dBodyGetPointVel`, 0x5b70d0),
/// normalized, dots with its normal over [`SEPARATING`] goes. ODE's
/// normalize (0x5b7460) makes a still point's velocity (1, 0, 0).
fn drop_separating(contacts: &mut Vec<Contact>, v0: Vec3, w0: Vec3) {
    contacts.retain(|c| {
        let v = v0 + w0.cross(c.r);
        let dir = if v == Vec3::ZERO {
            Vec3::X
        } else {
            v.normalize()
        };
        // A NaN keeps it, as the compare's unordered flags do.
        let along = c.n.dot(dir);
        !touching(c) || along <= SEPARATING || along.is_nan()
    });
}

/// The solids of `clip` round a box under the fx world's mask
/// (`0x2806c91`), as its world collider (0x5a8110) gathers them.
pub fn clip_solids(clip: &crate::collision::ClipMap) -> impl FnMut(Vec3, Vec3) -> Vec<Solid> + '_ {
    let mut t = clip_solids_masked(clip);
    move |lo: Vec3, hi: Vec3| t(lo, hi, crate::ragdoll::CLIP_MASK)
}

/// [`clip_solids`] under a given mask.
pub fn clip_solids_masked(
    clip: &crate::collision::ClipMap,
) -> impl FnMut(Vec3, Vec3, u32) -> Vec<Solid> + '_ {
    move |lo: Vec3, hi: Vec3, mask: u32| {
        let w = clip.world_solids(lo, hi, mask);
        w.brushes
            .iter()
            .filter_map(|p| Solid::brush(p))
            .chain(w.tris.into_iter().filter_map(Solid::triangle))
            .collect()
    }
}

/// A point trace through `clip` under the world mask the fx world's
/// bodies meet (`0x2806c91`, iw3mp.exe 0x5a6acf).
pub fn clip_trace(clip: &crate::collision::ClipMap) -> impl FnMut(Vec3, Vec3, f32) -> Sweep + '_ {
    let mut t = clip_trace_masked(clip);
    move |a: Vec3, b: Vec3, r: f32| t(a, b, r, crate::ragdoll::CLIP_MASK)
}

/// [`clip_trace`] under a given mask. `CM_Trace` (0x4f4d30, called at
/// 0x5a6aea) clips brushes and terrain, not static models.
pub fn clip_trace_masked(
    clip: &crate::collision::ClipMap,
) -> impl FnMut(Vec3, Vec3, f32, u32) -> Sweep + '_ {
    move |a: Vec3, b: Vec3, r: f32, mask: u32| {
        let e = Vec3::splat(r);
        let t = clip.trace(a, b, -e, e, mask);
        Sweep {
            fraction: t.fraction,
            normal: t.normal,
            startsolid: t.startsolid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world of half-spaces `n . p <= dist` that are solid, traced as
    /// points with the kernels' skin.
    fn planes(solid: &[(Vec3, f32)]) -> impl FnMut(Vec3, Vec3, f32) -> Sweep + '_ {
        move |a: Vec3, b: Vec3, _r: f32| {
            let mut best = Sweep {
                fraction: 1.0,
                normal: Vec3::ZERO,
                startsolid: false,
            };
            for &(n, dist) in solid {
                let da = n.dot(a) - dist;
                let db = n.dot(b) - dist;
                if da <= 0.0 {
                    return Sweep {
                        fraction: 0.0,
                        normal: n,
                        startsolid: true,
                    };
                }
                if db < SKIN && db < da {
                    let f = ((da - SKIN) / (da - db)).clamp(0.0, 1.0);
                    if f < best.fraction {
                        best = Sweep {
                            fraction: f,
                            normal: n,
                            startsolid: false,
                        };
                    }
                }
            }
            best
        }
    }

    fn door() -> (Shape, Preset) {
        // vehicle_80s_sedan1's door: bounds off the zone, `car_door`.
        let shape = Shape::of_bounds(
            Vec3::new(-41.97, -10.69, -16.0),
            Vec3::new(0.02, 0.01, 28.75),
        );
        let preset = Preset {
            mass: 25.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 0.6,
        };
        (shape, preset)
    }

    fn run(b: &mut Body, ms: i32, trace: &mut Trace, d: &Dvars) {
        let mut t = b.sim_ms;
        while t < ms {
            t += 16;
            b.advance(t, trace, d);
        }
    }

    #[test]
    fn a_free_body_falls_at_phys_gravity() {
        let (shape, preset) = door();
        let d = Dvars {
            drag_linear: 0.0,
            ..Dvars::default()
        };
        let mut b = Body::new(Vec3::Z * 10000.0, Vec3::ZERO, &shape, &preset, 0);
        let mut none = |_a: Vec3, _b: Vec3, _r: f32| Sweep {
            fraction: 1.0,
            normal: Vec3::ZERO,
            startsolid: false,
        };
        let z0 = b.origin().z;
        // 17 ms steps for a second.
        for _ in 0..60 {
            b.step(0.017, &mut none, &d);
        }
        let t = 60.0 * 0.017;
        assert!((b.vel.z + 800.0 * t).abs() < 1e-2, "{}", b.vel.z);
        // Semi-implicit Euler falls a step's worth further than the curve.
        let fell = z0 - b.origin().z;
        assert!((fell - 400.0 * t * t).abs() < 800.0 * t * 0.017, "{fell}");
    }

    #[test]
    fn the_launch_throws_by_the_impulse_formula() {
        let shape = Shape::of_bounds(Vec3::splat(-5.0), Vec3::splat(5.0));
        let preset = Preset {
            mass: 15.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 0.6,
        };
        let mut b = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
        // At the centre no arm: J = speed * m * scale / (m + 0.5), along
        // the force tipped up by 0.5.
        // Each pushed at world 1's 17 and spent over a 17 ms step, whole.
        b.launch(Vec3::ZERO, Vec3::X * 1000.0, 0.6, 17, &Dvars::default());
        b.spend_force(0.017);
        let dir = Vec3::new(1.0, 0.0, 0.5).normalize();
        let want = dir * (1000.0 * 0.6 / 15.5);
        assert!(b.vel.distance(want) < 1e-3, "{} {want}", b.vel);
        assert_eq!(b.avel, Vec3::ZERO);
        // Off the centre it spins and flies slower.
        let mut s = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
        s.launch(Vec3::Y * 4.0, Vec3::X * 1000.0, 0.6, 17, &Dvars::default());
        s.spend_force(0.017);
        assert!(s.vel.length() < b.vel.length());
        assert!(s.avel.z < -1.0, "{}", s.avel);
        // A body already flying faster along the push takes nothing.
        let mut f = b.clone();
        f.vel = dir * 2000.0;
        f.launch(Vec3::ZERO, Vec3::X * 1000.0, 0.6, 17, &Dvars::default());
        f.spend_force(0.017);
        assert_eq!(f.vel, dir * 2000.0);
    }

    #[test]
    fn a_push_is_a_force_over_the_next_step_of_its_body_world() {
        // Out of the world, the free body steps as gravity and the drag
        // leave it, x and the spin untouched by gravity.
        let mut none = planes(&[]);
        let d = Dvars::default();
        let (shape, preset) = door();
        let m = preset.mass;
        let step = |b: &mut Body, ms: i32, none: &mut Trace| {
            let until = b.sim_ms + ms;
            b.step_world(ms, until, none, &mut |_, _| Vec::new(), &d);
        };
        // Pushed at the centre at the 17 of the world named, then two 25 ms
        // steps (a 50 ms frame): the first spends 25 / 17 of the push, the
        // second nothing more.
        let mut b = Body::new(Vec3::Z * 100.0, Vec3::ZERO, &shape, &preset, 0);
        b.push(b.pos, Vec3::X * 100.0, 17);
        assert_eq!(b.vel, Vec3::ZERO);
        step(&mut b, 25, &mut none);
        let drag = 1.0 - d.drag_linear * 0.025;
        let want = 100.0 / m * 25.0 / 17.0 * drag;
        assert!((b.vel.x - want).abs() < want * 1e-5, "{} {want}", b.vel.x);
        step(&mut b, 25, &mut none);
        assert!((b.vel.x - want * drag).abs() < want * 1e-5, "{}", b.vel.x);
        // Off the centre at a world's 28, spent over a 17 ms step: 17 / 28
        // of the spin.
        let mut s = Body::new(Vec3::Z * 100.0, Vec3::ZERO, &shape, &preset, 0);
        let p = Vec3::X * 100.0;
        let r = Vec3::Y * 4.0;
        s.push(s.pos + r, p, 28);
        let whole = s.inv_inertia_world() * r.cross(p);
        step(&mut s, 17, &mut none);
        let want = whole * (17.0 / 28.0) * (1.0 - d.drag_angular * 0.017);
        assert!(
            s.avel.distance(want) < want.length() * 1e-4,
            "{} {want}",
            s.avel
        );
    }

    #[test]
    fn a_dropped_box_bounces_by_its_preset() {
        let shape = Shape::of_bounds(Vec3::splat(-8.0), Vec3::splat(8.0));
        let preset = Preset {
            mass: 18.0,
            bounce: 0.8,
            friction: 0.8,
            bullet_force_scale: 0.6,
        };
        let floor = [(Vec3::Z, 0.0)];
        let mut trace = planes(&floor);
        let d = Dvars::default();
        let mut b = Body::new(Vec3::Z * 200.0, Vec3::ZERO, &shape, &preset, 0);
        let mut peak_after = 0.0f32;
        let mut down = false;
        for _ in 0..120 {
            b.step(0.017, &mut trace, &d);
            if b.vel.z > 0.0 {
                down = true;
            }
            if down {
                peak_after = peak_after.max(b.origin().z);
            }
        }
        // Dropped from 192 over the floor it comes back up most of the way
        // (0.8 bounce: 0.64 of the height, less a step's loss).
        assert!(down);
        assert!(peak_after > 8.0 + 192.0 * 0.5, "{peak_after}");
        assert!(peak_after < 8.0 + 192.0 * 0.7, "{peak_after}");
    }

    #[test]
    fn a_launched_door_lands_slides_and_sleeps_on_the_floor() {
        let (shape, preset) = door();
        let floor = [(Vec3::Z, 0.0), (-Vec3::X, -600.0)];
        let mut trace = planes(&floor);
        let d = Dvars::default();
        let mut b = Body::new(
            Vec3::Z * 60.0,
            Vec3::new(0.0, 30.0, 0.0),
            &shape,
            &preset,
            0,
        );
        // `_destructible.gsc`'s blast: 4500 x 1.0 off the explosion, at the
        // door's own origin (its hinge), which the spin scale throws out to
        // four times its arm: the door spins more than it flies.
        b.launch(b.origin(), Vec3::new(1.0, 0.3, 0.2) * 4500.0, 0.6, 17, &d);
        let mut spent = b.clone();
        spent.spend_force(0.017);
        assert!((30.0..200.0).contains(&spent.vel.length()), "{}", spent.vel);
        assert!(spent.avel.length() > 10.0, "{}", spent.avel);
        run(&mut b, 10_000, &mut trace, &d);
        assert!(b.asleep, "{b:?}");
        for p in b.world_points() {
            assert!(p.z > -0.01 && p.x < 600.01, "{p}");
        }
        // Resting: its lowest points on the floor.
        let low = b.world_points().map(|p| p.z).fold(f32::MAX, f32::min);
        assert!(low < 1.0, "{low}");
        let (o, _) = b.pose(b.sim_ms);
        assert!(o.distance(b.origin()) < 1e-3);
    }

    /// A preset's negative, NaN or infinite friction lands a body on the
    /// floor without a panic in the friction clamp (SEC-029).
    #[test]
    fn a_hostile_friction_lands_on_the_floor() {
        let (shape, preset) = door();
        let floor = [(Vec3::Z, 0.0)];
        let d = Dvars::default();
        for friction in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let preset = Preset { friction, ..preset };
            let mut trace = planes(&floor);
            let mut b = Body::new(Vec3::Z * 30.0, Vec3::ZERO, &shape, &preset, 0);
            b.vel = Vec3::new(100.0, 0.0, 0.0);
            run(&mut b, 2000, &mut trace, &d);
        }
    }

    #[test]
    fn a_fast_part_into_a_corner_stays_out_of_the_world() {
        let (shape, preset) = door();
        // A floor, a wall 100 units ahead and one 60 to the side.
        let solid = [(Vec3::Z, 0.0), (-Vec3::X, -100.0), (Vec3::Y, -60.0)];
        let mut trace = planes(&solid);
        let d = Dvars::default();
        let mut b = Body::new(
            Vec3::Z * 50.0,
            Vec3::new(0.0, 0.0, 20.0),
            &shape,
            &preset,
            0,
        );
        b.vel = Vec3::new(900.0, -400.0, 300.0);
        b.avel = Vec3::new(4.0, -9.0, 12.0);
        let mut t = 0;
        while t < 10_000 && !b.asleep {
            t += 16;
            b.advance(t, &mut trace, &d);
            for p in b.world_points() {
                assert!(p.z > -0.01 && p.x < 100.01 && p.y > -60.01, "{p} at {t}");
            }
        }
        assert!(b.asleep, "{b:?}");
    }

    #[test]
    fn a_tilted_box_settles_flat() {
        let shape = Shape::of_bounds(Vec3::new(-20.0, -10.0, -3.0), Vec3::new(20.0, 10.0, 3.0));
        let preset = Preset {
            mass: 15.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 0.6,
        };
        let floor = [(Vec3::Z, 0.0)];
        let mut trace = planes(&floor);
        let d = Dvars::default();
        let mut b = Body::new(
            Vec3::Z * 40.0,
            Vec3::new(35.0, 20.0, 50.0),
            &shape,
            &preset,
            0,
        );
        run(&mut b, 8000, &mut trace, &d);
        assert!(b.asleep);
        // On a face: the box's thin axis upright.
        let up = b.rot * Vec3::Z;
        assert!(up.z.abs() > 0.99, "{up}");
        let low = b.world_points().map(|p| p.z).fold(f32::MAX, f32::min);
        assert!((0.0..1.0).contains(&low), "{low}");
    }

    #[test]
    fn the_frame_clock_takes_at_most_two_steps_and_lerps() {
        let (shape, preset) = door();
        let mut none = |_a: Vec3, _b: Vec3, _r: f32| Sweep {
            fraction: 1.0,
            normal: Vec3::ZERO,
            startsolid: false,
        };
        let d = Dvars::default();
        let mut b = Body::new(Vec3::Z * 1000.0, Vec3::ZERO, &shape, &preset, 100);
        b.advance(105, &mut none, &d);
        assert_eq!((b.steps, b.sim_ms), (1, 117));
        // 12 ms short of the step: the pose sits 5/17 of the way.
        let (o, _) = b.pose(105);
        let f = (o.z - b.from.0.z + b.com.z) / (b.pos.z - b.from.0.z);
        assert!((f - 5.0 / 17.0).abs() < 1e-3, "{f}");
        b.advance(110, &mut none, &d);
        assert_eq!(b.steps, 1);
        // A 300 ms frame: two steps of 150.
        b.advance(417, &mut none, &d);
        assert_eq!((b.steps, b.sim_ms), (3, 417));
    }

    #[test]
    fn a_brush_of_5000_sides_falls_back_at_once() {
        // Planes in general position: every triple meets.
        let planes: Vec<[f32; 4]> = (0..5000)
            .map(|i| {
                let (a, b) = (i as f32 * 0.37, i as f32 * 0.11);
                [a.cos() * b.sin(), a.sin() * b.sin(), b.cos(), 10.0]
            })
            .collect();
        let list = PhysGeomList {
            geoms: vec![crate::assets::model::PhysGeom {
                brush: Some(PhysBrush {
                    mins: [-10.0; 3],
                    maxs: [10.0; 3],
                    planes: planes.clone(),
                }),
                kind: 0,
                orientation: [[0.0; 3]; 3],
                offset: [0.0; 3],
                half_lengths: [0.0; 3],
            }],
            ..Default::default()
        };
        let s = Shape::of_geoms(&list).unwrap();
        // The bounds' box: 8 corners, 12 edges, 6 faces.
        assert_eq!(s.points.len(), 26);
        let world: Vec<(Vec3, f32)> = planes
            .iter()
            .map(|p| (Vec3::new(p[0], p[1], p[2]), p[3]))
            .collect();
        assert!(Solid::brush(&world).is_none());
    }

    #[test]
    fn a_models_geoms_give_its_points_and_mass() {
        let list = PhysGeomList {
            geoms: vec![crate::assets::model::PhysGeom {
                brush: None,
                kind: GEOM_BOX,
                orientation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                offset: [0.0, -1.0, 12.75],
                half_lengths: [18.0, 18.5, 11.75],
            }],
            center_of_mass: [0.0, -1.0, 12.75],
            moments: [159.9, 153.9, 221.9],
            products: [0.0; 3],
        };
        let s = Shape::of_geoms(&list).unwrap();
        assert_eq!(s.points.len(), 26);
        assert_eq!(s.com, Vec3::new(0.0, -1.0, 12.75));
        assert!(s.points.contains(&Vec3::new(18.0, 17.5, 24.5)));
        assert_eq!(s.inertia.x_axis.x, 159.9);
        // A brush gives its corners: a sedan hood's box, and a cube with
        // one edge cut off by a side.
        let hood = PhysBrush {
            mins: [-3.5, -24.0, -8.0],
            maxs: [41.5, 24.0, 0.0],
            planes: vec![],
        };
        let mut c = brush_corners(&hood);
        assert_eq!(c.len(), 8);
        assert!(c.contains(&Vec3::new(41.5, -24.0, -8.0)));
        let wedge = PhysBrush {
            mins: [0.0; 3],
            maxs: [10.0; 3],
            planes: vec![[1.0, 0.0, 1.0, 10.0]],
        };
        c = brush_corners(&wedge);
        assert_eq!(c.len(), 6, "{c:?}");
        assert!(c.iter().all(|p| p.x + p.z <= 10.01));
        let one = PhysGeomList {
            geoms: vec![crate::assets::model::PhysGeom {
                brush: Some(hood),
                kind: 0,
                orientation: [[0.0; 3]; 3],
                offset: [0.0; 3],
                half_lengths: [0.0; 3],
            }],
            center_of_mass: [19.04, 0.0, -3.56],
            moments: [195.1, 130.4, 318.9],
            products: [0.0, 16.9, 0.0],
        };
        let s = Shape::of_geoms(&one).unwrap();
        assert_eq!((s.points.len(), s.com), (26, Vec3::new(19.04, 0.0, -3.56)));
        assert!(s.points.contains(&Vec3::new(41.5, 0.0, -4.0)));
        // `me_corrugated_metal8x8`'s moments are a box's per unit mass.
        let b = box_inertia(Vec3::new(48.0, 1.0, 48.0));
        assert!((b.x_axis.x - 768.33).abs() < 0.01);
    }

    /// A world of convex brushes (their planes `n . p <= dist`), traced
    /// as points the way the clip map's kernels do, with their skin: a
    /// start within it that closes on a face at all stops at once.
    fn brushes(solid: &[Vec<(Vec3, f32)>]) -> impl FnMut(Vec3, Vec3, f32) -> Sweep + '_ {
        move |a: Vec3, b: Vec3, _r: f32| {
            let mut best = Sweep {
                fraction: 1.0,
                normal: Vec3::ZERO,
                startsolid: false,
            };
            'brush: for brush in solid {
                let (mut enter, mut leave, mut out) = (-1.0f32, 1.0f32, false);
                let mut normal = Vec3::ZERO;
                for &(n, dist) in brush {
                    let (d1, d2) = (n.dot(a) - dist, n.dot(b) - dist);
                    out |= d1 > 0.0;
                    if d1 > 0.0 && (d2 >= SKIN || d2 >= d1) {
                        continue 'brush;
                    }
                    if d1 <= 0.0 && d2 <= 0.0 {
                        continue;
                    }
                    if d1 > d2 {
                        let f = ((d1 - SKIN) / (d1 - d2)).max(0.0);
                        if f > enter {
                            (enter, normal) = (f, n);
                        }
                    } else {
                        leave = leave.min(((d1 + SKIN) / (d1 - d2)).min(1.0));
                    }
                }
                if !out {
                    return Sweep {
                        fraction: 0.0,
                        normal: Vec3::Z,
                        startsolid: true,
                    };
                }
                if enter > -1.0 && enter < leave && enter < best.fraction {
                    best = Sweep {
                        fraction: enter,
                        normal,
                        startsolid: false,
                    };
                }
            }
            best
        }
    }

    #[test]
    fn a_box_held_on_its_centre_by_a_ridge_comes_to_rest() {
        // A floor, and a ridge 20 high whose top pokes between a sedan
        // hood's eight corners (no other point) and holds the box on its
        // centre: the post-step sweep stopping it short of the ridge, under
        // a fraction of 0.001, leaves it no speed to keep.
        let side = |s: f32| {
            (
                Vec3::new(0.0, s * 1.5, 1.0).normalize(),
                20.0 / 1.5f32.hypot(1.0),
            )
        };
        let world = [
            vec![(Vec3::Z, 0.0), (-Vec3::Z, 100.0)],
            vec![side(1.0), side(-1.0), (-Vec3::Z, 0.0)],
        ];
        let mut trace = brushes(&world);
        let mut shape = Shape::of_bounds(Vec3::new(-22.5, -24.0, -4.0), Vec3::new(22.5, 24.0, 4.0));
        shape.points.truncate(8);
        let preset = Preset {
            mass: 15.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 0.6,
        };
        let d = Dvars::default();
        let mut b = Body::new(Vec3::new(0.0, 3.0, 26.0), Vec3::ZERO, &shape, &preset, 0);
        run(&mut b, 10000, &mut trace, &d);
        assert!(b.asleep, "pos {} vel {} avel {}", b.pos, b.vel, b.avel);
    }

    #[test]
    fn a_box_across_a_kerbs_edge_rests_on_the_edge() {
        // A floor and a kerb 10 high from x = 0, as brushes, and a sedan
        // hood's box on its 8 corners alone, dropped flat across the
        // kerb's edge: the edge between the corners meets its face.
        let slab = |lo: Vec3, hi: Vec3| {
            (0..3)
                .flat_map(|k| [(Vec3::AXES[k], hi[k]), (-Vec3::AXES[k], -lo[k])])
                .collect::<Vec<_>>()
        };
        let world = [
            slab(
                Vec3::new(-200.0, -200.0, -50.0),
                Vec3::new(200.0, 200.0, 0.0),
            ),
            slab(Vec3::new(0.0, -200.0, -50.0), Vec3::new(200.0, 200.0, 10.0)),
        ];
        let solid: Vec<Solid> = world.iter().filter_map(|p| Solid::brush(p)).collect();
        assert_eq!(solid[1].faces.len(), 6);
        let mut trace = brushes(&world);
        let mut solids = |_: Vec3, _: Vec3| solid.clone();
        let mut shape = Shape::of_bounds(Vec3::new(-22.5, -24.0, -4.0), Vec3::new(22.5, 24.0, 4.0));
        shape.points.truncate(8);
        let preset = Preset {
            mass: 15.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 0.6,
        };
        let d = Dvars::default();
        let mut b = Body::new(Vec3::new(-6.0, 0.0, 30.0), Vec3::ZERO, &shape, &preset, 0);
        let mut t = 0;
        while t < 10_000 && !b.asleep {
            t += 16;
            b.advance_in(t, &mut trace, &mut solids, &d);
        }
        assert!(b.asleep, "{b:?}");
        // No point of the kerb's edge under the box lies in it.
        let w = b.world_boxes().next().unwrap();
        let tipped = w.axes.z_axis.z < 0.999;
        for i in -20..=20 {
            let local = w.axes.transpose() * (Vec3::new(0.0, i as f32, 10.0) - w.centre);
            let inside = (w.half - local.abs()).min_element();
            assert!(inside < 0.01, "{} inside at y {i}: {inside}", local);
        }
        assert!(tipped, "{:?}", w.axes);
    }

    #[test]
    fn a_cylinder_meets_a_floor_on_its_side_line_and_a_kerb_by_its_edge() {
        let slab = |lo: Vec3, hi: Vec3| {
            (0..3)
                .flat_map(|k| [(Vec3::AXES[k], hi[k]), (-Vec3::AXES[k], -lo[k])])
                .collect::<Vec<_>>()
        };
        let floor = Solid::brush(&slab(Vec3::splat(-100.0), Vec3::new(100.0, 100.0, 0.0))).unwrap();
        // A can of radius 3 and half height 5 on its side along x, 0.1
        // into the floor: the side line's two ends, normal up.
        let cy = ShapeCylinder {
            centre: Vec3::new(0.0, 0.0, 2.9),
            axis: Vec3::X,
            radius: 3.0,
            half: 5.0,
        };
        let mut out = Vec::new();
        cylinder_contacts(&cy, cy.centre, &floor, 0.5, false, &mut out);
        assert_eq!(out.len(), 2, "{out:?}");
        for (p, n, sep) in &out {
            assert!(
                n.distance(Vec3::Z) < 1e-5 && (sep + 0.1).abs() < 1e-4,
                "{p} {n} {sep}"
            );
            assert!((p.x.abs() - 5.0).abs() < 1e-4 && p.z.abs() < 0.11);
        }
        // Upright, the bottom cap's rim, all of it on the floor.
        let up = ShapeCylinder {
            axis: Vec3::Z,
            centre: Vec3::new(0.0, 0.0, 4.9),
            ..cy
        };
        out.clear();
        cylinder_contacts(&up, up.centre, &floor, 0.5, false, &mut out);
        assert_eq!(out.len(), RING, "{out:?}");
        assert!(
            out.iter()
                .all(|(p, _, sep)| (p.z + 0.1).abs() < 1e-4 && (sep + 0.1).abs() < 1e-4)
        );
        // A kerb 10 high from x = 0 and the can upright beside it, its rim
        // a unit into the kerb's side: the kerb's top edge pokes into the
        // rim, and the contacts push it back off the kerb.
        let kerb = Solid::brush(&slab(
            Vec3::new(0.0, -100.0, -100.0),
            Vec3::new(100.0, 100.0, 10.0),
        ))
        .unwrap();
        let by = ShapeCylinder {
            centre: Vec3::new(-2.0, 0.0, 9.0),
            ..up
        };
        out.clear();
        cylinder_contacts(&by, by.centre, &kerb, 0.5, false, &mut out);
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|(_, n, sep)| n.x < -0.5 && *sep < 0.0),
            "{out:?}"
        );
    }

    #[test]
    fn the_post_step_sweep_stops_the_centre_and_keeps_its_speed() {
        // A wall at x = 10; a can (sweep half size 2.12, a box sweep of
        // 0.42) moving 30 units a step at it stops on the wall with its
        // speed; a bottle's point sweep backs off by half its size.
        let wall = [(-Vec3::X, -10.0)];
        let mut trace = planes(&wall);
        let d = Dvars::default();
        let preset = Preset {
            mass: 0.1,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 1.0,
        };
        let mut shape = Shape::of_bounds(Vec3::splat(-3.0), Vec3::splat(3.0));
        shape.sweep_half = 2.12;
        let mut b = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
        b.vel = Vec3::X * 1800.0;
        b.pos = Vec3::X * 30.0;
        b.sweep(Vec3::ZERO, &mut trace, &d);
        assert!((b.pos.x - (10.0 - SKIN)).abs() < 0.01, "{}", b.pos);
        assert_eq!((b.vel, b.state), (Vec3::X * 1800.0, 2));
        shape.sweep_half = 1.5;
        let mut b = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
        b.pos = Vec3::X * 30.0;
        b.vel = Vec3::X * 1800.0;
        b.sweep(Vec3::ZERO, &mut trace, &d);
        assert!((b.pos.x - (10.0 - SKIN - 0.75)).abs() < 0.01, "{}", b.pos);
        // From there 0.9 on: the back-off eats the whole fraction, under
        // 0.001, and no speed is left.
        let old = b.pos;
        b.pos = old + Vec3::X * 0.9;
        b.avel = Vec3::Z;
        b.sweep(old, &mut trace, &d);
        assert!(b.pos.distance(old) < 0.001, "{}", b.pos);
        assert_eq!((b.vel, b.avel), (Vec3::ZERO, Vec3::ZERO));
    }

    #[test]
    fn a_narrow_box_over_a_brushs_edge_takes_its_other_faces() {
        // A cube, and a plate 1 thick across its top +x edge, tipped 30
        // degrees about y, the edge a quarter unit into it. The box's
        // underside is the reference; only the narrow pass adds the cube's
        // top, which the plate's centre lies over.
        let cube: Vec<(Vec3, f32)> = (0..3)
            .flat_map(|k| [(Vec3::AXES[k], 10.0), (-Vec3::AXES[k], 0.0)])
            .collect();
        let s = Solid::brush(&cube).unwrap();
        let a = Mat3::from_rotation_y(30f32.to_radians());
        let (c, h) = (Vec3::new(10.2, 5.0, 10.2), Vec3::new(6.0, 4.0, 0.5));
        let tops = |narrow: bool| {
            let mut out = Vec::new();
            box_contacts(c, a, h, &s, 1.0, narrow, &mut out);
            out.iter()
                .filter(|&&(_, n, _)| n.distance(Vec3::Z) < 1e-4)
                .count()
        };
        assert_eq!(tops(false), 0);
        assert!(tops(true) > 0);
        // The plate is narrow under `phys_narrowObjMaxLength`.
        assert!(h.min_element() * 2.0 < Dvars::default().narrow_max_length);
    }

    /// A contact `r` off the centre with normal `n`, the rest unused.
    fn touch(r: Vec3, n: Vec3) -> Contact {
        Contact {
            r,
            n,
            target: 0.0,
            k_n: 1.0,
            t: [Vec3::X, Vec3::Y],
            k_t: [1.0; 2],
            lambda: 0.0,
            lambda_t: [0.0; 2],
            sep: 0.0,
            point: false,
            geom: 0,
        }
    }

    #[test]
    fn a_pinched_bodys_facing_contacts_keep_the_one_its_centre_is_in_front_of() {
        let falling = Vec3::new(0.0, 0.0, -10.0);
        let normals = |c: &[Contact]| c.iter().map(|c| c.n).collect::<Vec<_>>();
        // Walls either side of the centre, a floor under it: the first
        // wall stays, the second's slot takes the floor.
        let mut c = vec![
            touch(Vec3::new(-5.0, 0.0, 0.0), Vec3::X),
            touch(Vec3::new(5.0, 0.0, 0.0), -Vec3::X),
            touch(Vec3::new(0.0, 0.0, -3.0), Vec3::Z),
        ];
        unwedge(&mut c, falling, Vec3::ZERO);
        assert_eq!(normals(&c), [Vec3::X, Vec3::Z]);
        // The centre behind the first: the second takes its slot.
        let mut c = vec![
            touch(Vec3::new(5.0, 0.0, 0.0), Vec3::X),
            touch(Vec3::new(-5.0, 0.0, 0.0), -Vec3::X),
        ];
        unwedge(&mut c, falling, Vec3::ZERO);
        assert_eq!((normals(&c), c[0].r.x), (vec![-Vec3::X], -5.0));
        // The last contact moved into a dropped one's slot is passed over:
        // the third faces the first too and stays.
        let mut c = vec![
            touch(Vec3::new(-5.0, 0.0, 0.0), Vec3::X),
            touch(Vec3::new(5.0, 0.0, 0.0), -Vec3::X),
            touch(Vec3::new(5.0, 0.0, 1.0), -Vec3::X),
        ];
        unwedge(&mut c, falling, Vec3::ZERO);
        assert_eq!(normals(&c), [Vec3::X, -Vec3::X]);
    }

    #[test]
    fn the_post_step_sweep_runs_once_a_frame() {
        // A centre alone (no points, no geoms) of sweep half size 1.9, a
        // point sweep backing off 0.95, at 100 units/s at a wall at x = 10:
        // its first 17 ms step ends at 8.925, where the sweep stops it.
        let wall = [(-Vec3::X, -10.0)];
        let mut trace = planes(&wall);
        let d = Dvars {
            gravity: 0.0,
            ..Dvars::default()
        };
        let preset = Preset {
            mass: 1.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 1.0,
        };
        let shape = Shape {
            points: Vec::new(),
            boxes: Vec::new(),
            cylinders: Vec::new(),
            point_geoms: Vec::new(),
            box_geoms: Vec::new(),
            cylinder_geoms: Vec::new(),
            com: Vec3::ZERO,
            inertia: Mat3::IDENTITY,
            sweep_half: 1.9,
        };
        let start = Vec3::X * 7.225;
        let mut b = Body::new(start, Vec3::ZERO, &shape, &preset, 0);
        b.vel = Vec3::X * 100.0;
        let mut per_step = b.clone();
        // One frame of two steps sweeps once, from 7.225 to 10.625: it
        // stops at 8.925 half way, its speed kept.
        b.advance(34, &mut trace, &d);
        assert!((b.pos.x - 8.925).abs() < 0.01, "{}", b.pos);
        assert!(b.vel.x > 99.0, "{}", b.vel);
        // Swept each step, the second starts at the stop and loses it all.
        per_step.step(0.017, &mut trace, &d);
        per_step.step(0.017, &mut trace, &d);
        assert!((per_step.pos.x - 8.925).abs() < 0.01, "{}", per_step.pos);
        assert_eq!(per_step.vel, Vec3::ZERO);
    }

    #[test]
    fn msec_step_follows_the_bodies_awake() {
        let w1: Vec<i32> = [0, 32, 33, 41, 49, 50, 200].map(|n| msec_step(1, n)).into();
        assert_eq!(w1, [17, 17, 19, 42, 64, 67, 67]);
        assert_eq!((msec_step(2, 41), msec_step(2, 50)), (25, 34));
        // Two steps at most a frame, each at least `msecStep`, which the
        // count after each step picks for the next.
        let mut clock = WorldClock::new(1);
        let mut awake = 41;
        let mut seen = Vec::new();
        let mut frame = |clock: &mut WorldClock, time: i32| {
            clock.advance(time, &Dvars::default(), |ms, until, first, d| {
                seen.push((ms, until, first, d.step_ms));
                awake -= 9;
                awake + 9
            });
        };
        frame(&mut clock, 100);
        clock.start(0);
        // 17, the least: one step reaches 16, and its 41 awake pick 42.
        frame(&mut clock, 16);
        // 183 left: 91 at 42, whose 32 awake pick 17, then the other 92.
        frame(&mut clock, 200);
        assert_eq!(
            (seen, clock.step_ms),
            (
                vec![
                    (17, 17, true, 17),
                    (91, 108, true, 42),
                    (92, 200, false, 17)
                ],
                17
            )
        );
    }

    #[test]
    fn five_or_more_contacts_keep_each_clusters_extremes_and_deepest() {
        let at = |x: f32, y: f32, z: f32, n: Vec3, depth: f32| (Vec3::new(x, y, z), n, depth);
        // A floor (one corner deepest, one a near twin of another), a wall
        // facing +x and one facing +y. The seed is contact 104729 % 10 = 9
        // (+y), then the floor's first (most against it), then the +x
        // wall's first (most against both).
        let c = [
            at(0.0, 0.0, 0.0, Vec3::Z, 0.1),
            at(0.05, 0.0, 0.0, Vec3::Z, 0.1),
            at(10.0, 0.0, 0.0, Vec3::Z, 0.1),
            at(10.0, 10.0, 0.0, Vec3::Z, 0.3),
            at(0.0, 10.0, 0.0, Vec3::Z, 0.1),
            at(0.0, 2.0, 1.0, Vec3::X, 0.1),
            at(0.0, 5.0, 4.0, Vec3::X, 0.1),
            at(0.0, 8.0, 2.0, Vec3::X, 0.1),
            at(3.0, 0.0, 2.0, Vec3::Y, 0.1),
            at(6.0, 0.0, 5.0, Vec3::Y, 0.1),
        ];
        // The +y wall's tangents are x and -z, the floor's -x and -y, the
        // +x wall's -y and -z: of each the least along both, the most along
        // both and the deepest, each once. The floor's twin and its corner
        // at (0, 10) go.
        assert_eq!(reduce(&c), [8, 9, 2, 3, 0, 7, 6, 5]);
        // A normal within 0.999 of another's mean joins its cluster: the
        // floor's tipped corner is one of its extremes, not a cluster.
        let tipped = Vec3::new(0.0, 0.01, 1.0).normalize();
        let floor = [
            at(0.0, 0.0, 0.0, Vec3::Z, 0.1),
            at(10.0, 0.0, 0.0, Vec3::Z, 0.1),
            at(10.0, 10.0, 0.0, tipped, 0.1),
            at(0.0, 10.0, 0.0, Vec3::Z, 0.1),
            at(5.0, 5.0, 0.0, Vec3::Z, 0.1),
        ];
        assert_eq!(reduce(&floor), [1, 2, 0]);
    }

    #[test]
    fn the_contact_pass_pinches_on_one_geoms_facing_contacts_only() {
        let (shape, preset) = door();
        let mut b = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
        b.box_geoms = vec![0, 1];
        let wall = |g: usize, x: f32, n: Vec3| Contact {
            geom: g,
            ..touch(Vec3::new(x, 0.0, 0.0), n)
        };
        // Falling, so the drop of the contacts a pinched body leaves keeps
        // the walls'.
        let (d, still, falling) = (Dvars::default(), Vec3::ZERO, -Vec3::Z * 10.0);
        // Each end of a two-geom body against its own wall: neither geom's
        // contacts face each other, and both stay.
        let out = b.contact_pass(
            vec![wall(0, -5.0, Vec3::X), wall(1, 5.0, -Vec3::X)],
            &d,
            falling,
            still,
            0.017,
        );
        assert_eq!((out.len(), b.state, b.pinched), (2, 0, 0));
        // One geom against both: pinched, and the one its centre is behind
        // goes. Its other geom, with nothing, sets the state 2 first.
        let out = b.contact_pass(
            vec![wall(0, -5.0, Vec3::X), wall(0, 5.0, -Vec3::X)],
            &d,
            falling,
            still,
            0.017,
        );
        assert_eq!((out.len(), b.state, b.pinched), (1, 1, 1));
        // On a later step the first geom goes first: its pinch, then the
        // other's state 2.
        b.steps = 2;
        let out = b.contact_pass(
            vec![wall(0, -5.0, Vec3::X), wall(0, 5.0, -Vec3::X)],
            &d,
            falling,
            still,
            0.017,
        );
        assert_eq!((out.len(), b.state, b.pinched), (1, 2, 2));
        // The walk's order decides whose contacts a pinch drops: geom 0
        // pinched between two walls, geom 1 on a floor it rises off. Last
        // geom first (the first step) the floor's contact is taken before
        // the pinch and stays; first geom first the pinched body drops it.
        let rising = Vec3::Z * 10.0;
        let walk = |steps: u32| {
            let mut b = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
            b.box_geoms = vec![0, 1];
            b.steps = steps;
            let floor = Contact {
                geom: 1,
                ..touch(Vec3::new(0.0, 0.0, -3.0), Vec3::Z)
            };
            let all = vec![wall(0, -5.0, Vec3::X), wall(0, 5.0, -Vec3::X), floor];
            let out = b.contact_pass(all, &d, rising, still, 0.017);
            out.iter().filter(|c| c.geom == 1).count()
        };
        assert_eq!((walk(1), walk(2)), (1, 0));
        // A point of a geom with a box is vcod's: it neither pinches nor
        // goes, and comes after the box's contacts.
        let mut b = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
        let point = Contact {
            point: true,
            ..wall(0, 5.0, -Vec3::X)
        };
        let out = b.contact_pass(
            vec![point, wall(0, -5.0, Vec3::X)],
            &d,
            falling,
            still,
            0.017,
        );
        assert_eq!(
            (out.iter().map(|c| c.point).collect::<Vec<_>>(), b.pinched),
            (vec![false, true], 0)
        );
    }

    #[test]
    fn a_pinched_body_drops_the_contacts_its_points_leave() {
        let floor = |r: Vec3| touch(r, Vec3::Z);
        // Rising at 10: the floor's contacts go, a ceiling's stays.
        let mut c = vec![floor(Vec3::ZERO), touch(Vec3::Z, -Vec3::Z)];
        drop_separating(&mut c, Vec3::Z * 10.0, Vec3::ZERO);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].n, -Vec3::Z);
        // Turning about x: the end at +y rises off the floor, the end at
        // -y comes down into it.
        let mut c = vec![floor(Vec3::Y * 10.0), floor(-Vec3::Y * 10.0)];
        drop_separating(&mut c, Vec3::ZERO, Vec3::X);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].r, -Vec3::Y * 10.0);
        // A still point's velocity normalizes to +x: a contact facing +x
        // goes, one facing -x or up stays.
        let mut c = vec![
            touch(Vec3::ZERO, Vec3::X),
            touch(Vec3::ZERO, -Vec3::X),
            floor(Vec3::ZERO),
        ];
        drop_separating(&mut c, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(
            c.iter().map(|c| c.n).collect::<Vec<_>>(),
            [-Vec3::X, Vec3::Z]
        );
    }
}
