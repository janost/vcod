//! Rounds and blasts on the entities a script opened to damage
//! (`setCanDamage`, `GameHost::script_takedamage`): the stock maps'
//! destructible cars and barrels (`_destructible.gsc`), which break on the
//! `damage` notify `G_Damage`'s entity arm sends (cod4-combat.md 13).

use std::rc::Rc;

use glam::{Quat, Vec3};
use vcod_cod4::assets::model::XModelCollSurf;
use vcod_cod4::pmove::weapon::WeaponDef;
use vcod_gsc::{EntId, Host, Value};

use super::level::Cod4Level;
use super::missile;

/// One model of an entity's DObj as a round meets it: its collision
/// surfaces (model space), its bone names and the `partBits` `hidePart`
/// left (a hidden bone's surfaces never clip), and where its model space
/// sits in the entity's: the base model's at the origin, an `attach`ed
/// one's at the base model's tag it hangs on.
#[derive(Clone, Debug)]
pub(crate) struct ModelPart {
    pub surfs: Rc<Vec<XModelCollSurf>>,
    pub bones: Rc<Vec<String>>,
    pub hidden: [u32; 4],
    /// The attachment's model and tag, both empty for the base model.
    pub model: String,
    pub tag: String,
    pub at: Vec3,
    pub rot: Quat,
}

/// Where a round first meets an entity's models ([`DamageBox::trace_surfs`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelHit {
    pub fraction: f32,
    /// The struck triangle's normal in the world.
    pub normal: Vec3,
    pub surface_flags: u32,
    /// `G_Damage`'s notify names: the attachment's model and tag (empty
    /// for the base model) and the struck surface's bone.
    pub model: String,
    pub tag: String,
    pub part: String,
}

/// A script model as a round or a blast meets it: its models' collision
/// surfaces placed by its origin and angles, and their bounds in its
/// space (the base model's, else the attachments').
#[derive(Clone, Debug)]
pub(crate) struct DamageBox {
    pub id: EntId,
    pub origin: Vec3,
    pub axis: [Vec3; 3],
    pub mins: Vec3,
    pub maxs: Vec3,
    pub parts: Vec<ModelPart>,
}

impl DamageBox {
    fn local(&self, p: Vec3) -> Vec3 {
        let d = p - self.origin;
        Vec3::new(
            d.dot(self.axis[0]),
            d.dot(self.axis[1]),
            d.dot(self.axis[2]),
        )
    }

    /// Where `start` to `end` first crosses one of the models' collision
    /// triangles under `mask` before `limit` (`DObjGeomTraceline` on the
    /// bind pose, every model of the DObj, an attachment's whatever its
    /// `ignoreCollision`: a hidden bone's surfaces skipped).
    pub(crate) fn trace_surfs(
        &self,
        start: Vec3,
        end: Vec3,
        limit: f32,
        mask: u32,
    ) -> Option<ModelHit> {
        let mut tr = vcod_common::clipmap::ClipTrace::clear();
        tr.fraction = limit;
        let (a, b) = (self.local(start), self.local(end));
        let mut found = None;
        for (k, p) in self.parts.iter().enumerate() {
            let inv = p.rot.inverse();
            let (pa, pb) = (inv * (a - p.at), inv * (b - p.at));
            for s in p.surfs.iter() {
                let bone = s.bone.max(0) as usize;
                if bone < 128 && p.hidden[bone / 32] & (0x8000_0000 >> (bone % 32)) != 0 {
                    continue;
                }
                if vcod_cod4::collision::trace_line(std::slice::from_ref(s), pa, pb, mask, &mut tr)
                {
                    found = Some((k, bone, p.rot * tr.normal));
                }
            }
        }
        let (k, bone, n) = found?;
        let p = &self.parts[k];
        Some(ModelHit {
            fraction: tr.fraction,
            normal: self.axis[0] * n.x + self.axis[1] * n.y + self.axis[2] * n.z,
            surface_flags: tr.surface_flags,
            model: p.model.clone(),
            tag: p.tag.clone(),
            part: p.bones.get(bone).cloned().unwrap_or_default(),
        })
    }

    /// Where `start` to `end` first meets the box, as a fraction, and the
    /// struck side's normal in the world. `None` past `limit` or a miss.
    pub(crate) fn trace(&self, start: Vec3, end: Vec3, limit: f32) -> Option<(f32, Vec3)> {
        let (a, b) = (self.local(start), self.local(end));
        let d = b - a;
        let (mut t0, mut t1, mut axis) = (0.0f32, limit, None);
        for i in 0..3 {
            if d[i].abs() < 1e-6 {
                if a[i] < self.mins[i] || a[i] > self.maxs[i] {
                    return None;
                }
                continue;
            }
            let (mut n, mut f) = ((self.mins[i] - a[i]) / d[i], (self.maxs[i] - a[i]) / d[i]);
            let mut sign = -1.0;
            if n > f {
                std::mem::swap(&mut n, &mut f);
                sign = 1.0;
            }
            if n > t0 {
                t0 = n;
                axis = Some((i, sign));
            }
            t1 = t1.min(f);
            if t0 > t1 {
                return None;
            }
        }
        let (i, sign) = axis?;
        Some((t0, self.axis[i] * sign))
    }
}

impl Cod4Level {
    /// What a round meets besides the world and the bodies
    /// ([`Self::shot_boxes`]), for the bots' sight
    /// (`bots::cod4`'s `enemy`).
    pub(crate) fn shot_blockers(&mut self) -> Vec<DamageBox> {
        self.shot_boxes()
    }

    /// Every entity open to damage that is not a client, with a model
    /// whose collision LOD has surfaces, in entity order: what a blast
    /// meets.
    pub(super) fn damage_boxes(&mut self) -> Vec<DamageBox> {
        let ids: Vec<EntId> = self.rt.host.script_takedamage.iter().copied().collect();
        self.boxes_of(ids)
    }

    /// What a round meets past the world and the bodies: [`Self::damage_boxes`]'
    /// entities and every linked script model whose contents (0x2080,
    /// `SP_script_model` 0x809b964; `notSolid` 0) meet the shot's mask,
    /// open to damage or not (`SV_PointTraceToEntity`: a DObj
    /// entity flagged 4, 0x809b95d, is traced by its models'
    /// collision), in entity order.
    pub(super) fn shot_boxes(&mut self) -> Vec<DamageBox> {
        use crate::game::entity::LinkKind;
        let mut ids: std::collections::BTreeSet<EntId> =
            self.rt.host.script_takedamage.iter().copied().collect();
        ids.extend(self.rt.host.ents.iter_inuse().filter_map(|(id, e)| {
            let l = e.link.as_ref()?;
            (l.kind == LinkKind::ScriptModel
                && l.contents as u32 & super::combat::MASK_SHOT != 0
                && !e.unlinked)
                .then_some(id)
        }));
        self.boxes_of(ids.into_iter().collect())
    }

    fn boxes_of(&mut self, ids: Vec<EntId>) -> Vec<DamageBox> {
        let mut out = Vec::new();
        for id in ids {
            let Some(e) = self.rt.host.ents.get(id) else {
                continue;
            };
            if e.client.is_some() {
                continue;
            }
            let attachments = e.attachments.clone();
            let (model, origin, angles, attached) = self.rt.with_host(|h, cx| {
                let f = |h: &mut crate::game::host::GameHost, cx: &mut vcod_gsc::Cx, n: &str| {
                    let a = cx.intern_folded(n);
                    h.get_field(cx, id, a)
                };
                let model = match f(h, cx, "model") {
                    Value::String(s) => cx.resolve(s).to_ascii_lowercase(),
                    _ => String::new(),
                };
                let v = |v: Value| match v {
                    Value::Vector(v) => Vec3::from(v),
                    _ => Vec3::ZERO,
                };
                let attached: Vec<(String, String)> = attachments
                    .iter()
                    .map(|&(m, t)| (cx.resolve(m).to_string(), cx.resolve(t).to_string()))
                    .collect();
                (
                    model,
                    v(f(h, cx, "origin")),
                    v(f(h, cx, "angles")),
                    attached,
                )
            });
            let c4 = self.rt.host.cod4();
            let bones = |m: &str| c4.model_bones.get(m).cloned().unwrap_or_default();
            let mut parts = Vec::new();
            if let Some(surfs) = c4.model_coll.get(&model).filter(|s| !s.is_empty()) {
                parts.push(ModelPart {
                    surfs: surfs.clone(),
                    bones: bones(&model),
                    hidden: c4
                        .part_bits
                        .get(&id.0)
                        .filter(|p| p.0 == id)
                        .map_or([0; 4], |p| p.1),
                    model: String::new(),
                    tag: String::new(),
                    at: Vec3::ZERO,
                    rot: Quat::IDENTITY,
                });
            }
            for (m, t) in attached {
                let key = m.to_ascii_lowercase();
                let Some(surfs) = c4.model_coll.get(&key).filter(|s| !s.is_empty()) else {
                    continue;
                };
                // The attachment's root sits on the base model's tag at its
                // bind pose (`cod4_model_tag`'s placement); no tag, the root.
                let pose = if t.is_empty() {
                    Some((Vec3::ZERO, Quat::IDENTITY))
                } else {
                    c4.model_tags.get(&model).and_then(|tags| {
                        tags.iter()
                            .find(|(n, _, _)| n.eq_ignore_ascii_case(&t))
                            .map(|&(_, p, r)| (p, r))
                    })
                };
                let Some((at, rot)) = pose else { continue };
                parts.push(ModelPart {
                    surfs: surfs.clone(),
                    bones: bones(&key),
                    hidden: [0; 4],
                    model: m,
                    tag: t,
                    at,
                    rot,
                });
            }
            // The bounds a blast reads: the base model's surfaces, else
            // the attachments' placed.
            let mut corners = Vec::new();
            let base_only = parts.first().is_some_and(|p| p.model.is_empty());
            for p in parts.iter().filter(|p| !base_only || p.model.is_empty()) {
                for s in p.surfs.iter() {
                    let (lo, hi) = (Vec3::from(s.mins), Vec3::from(s.maxs));
                    for k in 0..8 {
                        let c = Vec3::new(
                            if k & 1 == 0 { lo.x } else { hi.x },
                            if k & 2 == 0 { lo.y } else { hi.y },
                            if k & 4 == 0 { lo.z } else { hi.z },
                        );
                        corners.push(p.at + p.rot * c);
                    }
                }
            }
            let Some(&first) = corners.first() else {
                continue;
            };
            let (mins, maxs) = corners
                .iter()
                .fold((first, first), |(lo, hi), &c| (lo.min(c), hi.max(c)));
            out.push(DamageBox {
                id,
                origin,
                axis: vcod_common::x87::angles_to_axis(angles),
                mins,
                maxs,
                parts,
            });
        }
        out
    }

    /// Test-facing: the script models a round meets ([`Self::shot_boxes`]),
    /// `(number, origin, open to damage)`.
    pub fn shot_models_for_test(&mut self) -> Vec<(u32, Vec3, bool)> {
        self.shot_boxes()
            .iter()
            .map(|b| (b.id.0, b.origin, self.takedamage_entity(b.id.0).is_some()))
            .collect()
    }

    /// Test-facing: where `start` to `end` first meets the clip map, a
    /// static model or a script model under the shot mask, and the script
    /// model's number when it is one.
    pub fn shot_model_trace_for_test(&mut self, start: Vec3, end: Vec3) -> (Vec3, Option<u32>) {
        let mask = super::combat::MASK_SHOT;
        let mut fraction = self.clip.as_ref().map_or(1.0, |c| {
            let mut t = c.trace(start, end, Vec3::ZERO, Vec3::ZERO, mask);
            c.trace_static_models(start, end, mask, &mut t);
            t.fraction
        });
        let mut number = None;
        for b in self.shot_boxes() {
            if let Some(m) = b.trace_surfs(start, end, fraction, mask)
                && m.fraction < fraction
            {
                (fraction, number) = (m.fraction, Some(b.id.0));
            }
        }
        (start.lerp(end, fraction), number)
    }

    /// The script entity numbered `num` when it is open to damage.
    pub(super) fn takedamage_entity(&self, num: u32) -> Option<EntId> {
        self.rt
            .host
            .script_takedamage
            .iter()
            .copied()
            .find(|id| id.0 == num)
    }

    /// `G_Damage`'s entity arm (`cod4_lnxded` 0x80b18be..0x80b1a66): at
    /// least 1 off `health` (ent+0x1a0), then `damage` notified with
    /// (damage, attacker, direction, point, means of death, model, tag,
    /// part, damage flags), `names` the last three; once health is gone it
    /// is held at -999 at worst and `death` notified with the attacker.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn damage_entity(
        &mut self,
        id: EntId,
        attacker: Option<usize>,
        damage: i32,
        dflags: i32,
        mod_: &'static str,
        dir: Vec3,
        point: Vec3,
        names: [&str; 3],
    ) {
        let who = attacker
            .and_then(|a| self.rt.client_entity(a))
            .map_or(Value::Undefined, Value::Entity);
        let damage = damage.max(1);
        let dead = self.rt.with_host(|h, cx| {
            let a = cx.intern_folded("health");
            let health = match h.get_field(cx, id, a) {
                Value::Int(n) => n,
                Value::Float(f) => f as i32,
                _ => 0,
            };
            let left = health - damage;
            let dead = left <= 0;
            let _ = h.set_field(cx, id, a, Value::Int(left.max(-999)));
            dead
        });
        let args = self.rt.with_host(|_, cx| {
            let [model, tag, part] = names.map(|n| Value::String(cx.intern_exact(n)));
            vec![
                Value::Int(damage),
                who,
                Value::Vector(dir.to_array()),
                Value::Vector(point.to_array()),
                Value::String(cx.intern_exact(mod_)),
                model,
                tag,
                part,
                Value::Int(dflags),
            ]
        });
        self.rt.host.engine_notifies.push((id, "damage", args));
        if dead {
            self.rt.host.engine_notifies.push((id, "death", vec![who]));
        }
    }

    /// `G_RadiusDamage` over the damageable entities: one whose origin
    /// lies inside the radius takes the inner damage falling to the outer
    /// at the edge (`G_RadiusDamage` 0x80b2f8c), whole when one of
    /// `CanDamage`'s five traces from the blast to its bounds' middle and
    /// four corners across it gets through, radius flags (5), toward its
    /// origin 24 up.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn radius_damage_entities(
        &mut self,
        origin: Vec3,
        attacker: usize,
        d: &WeaponDef,
        cone: f32,
        cone_dir: Vec3,
        mod_: &'static str,
    ) {
        let radius = (d.explosion_radius as f32).max(1.0);
        for b in self.damage_boxes() {
            let dist = b.origin.distance(origin);
            if dist >= radius {
                continue;
            }
            let scale = self.entity_can_damage(&b, origin, cone, cone_dir);
            if scale <= 0.0 {
                continue;
            }
            let mut dir = b.origin - origin;
            dir.z += 24.0;
            let damage = ((d.explosion_inner_damage - d.explosion_outer_damage) as f32
                * (1.0 - dist / radius)
                + d.explosion_outer_damage as f32)
                * scale;
            self.damage_entity(
                b.id,
                Some(attacker),
                damage as i32,
                5,
                mod_,
                dir,
                origin,
                ["", "", ""],
            );
        }
    }

    /// `CanDamage` on an entity (cod4-combat.md 13): whole when a line
    /// from `center` under 0x802011 reaches the middle of its bounds at
    /// its origin, unturned (`DObjPhysicsGetBounds`), or one of the four
    /// points that middle pushed across the line from the blast, sideways
    /// and up, by as far as the bounds reach along each; else nothing.
    fn entity_can_damage(&mut self, b: &DamageBox, center: Vec3, cone: f32, cone_dir: Vec3) -> f32 {
        let mid = b.origin + (b.mins + b.maxs) * 0.5;
        let toward = (center - mid).normalize_or_zero();
        let side = Vec3::new(-toward.y, toward.x, 0.0).normalize_or_zero();
        let up = toward.cross(side);
        let half = (b.maxs - b.mins) * 0.5;
        let reach = |axis: Vec3| axis * half.dot(axis.abs());
        let points = missile::aim_points(mid, reach(side), reach(up), missile::BOX_CORNERS);
        self.blast_share(
            center,
            points,
            cone,
            cone_dir,
            missile::MASK_RADIUS_LOS,
            missile::LineScore::FirstClear,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A car-sized box turned a quarter: a line across it meets the near
    /// side, one past it or stopping short meets nothing.
    #[test]
    fn a_turned_box_stops_the_lines_through_it() {
        let b = DamageBox {
            id: EntId(1, 0),
            origin: Vec3::new(100.0, 0.0, 0.0),
            axis: vcod_common::x87::angles_to_axis(Vec3::new(0.0, 90.0, 0.0)),
            mins: Vec3::new(-100.0, -40.0, 0.0),
            maxs: Vec3::new(100.0, 40.0, 60.0),
            parts: Vec::new(),
        };
        let (t, n) = b
            .trace(Vec3::new(0.0, 0.0, 30.0), Vec3::new(200.0, 0.0, 30.0), 1.0)
            .unwrap();
        assert!((t - 0.3).abs() < 1e-4, "{t}");
        assert!(n.abs_diff_eq(-Vec3::X, 1e-4), "{n}");
        assert!(
            b.trace(Vec3::new(0.0, 0.0, 80.0), Vec3::new(200.0, 0.0, 80.0), 1.0)
                .is_none()
        );
        assert!(
            b.trace(Vec3::new(0.0, 0.0, 30.0), Vec3::new(50.0, 0.0, 30.0), 1.0)
                .is_none()
        );
    }
}
