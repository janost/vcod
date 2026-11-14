//! One CoD4 1.7 round after its end is drawn: `Bullet_FirePenetrate`
//! through walls and bodies by `info/bullet_penetration_mp`'s depths, or
//! `Bullet_FireExtended` with penetration off
//! (docs/research/cod4-combat.md 3.1). 1.7's calls are at
//! `cod4_lnxded` 0x808a9aa..0x808aefa.

use std::collections::HashMap;
use std::rc::Rc;

use glam::Vec3;
use vcod_cod4::pmove::weapon::WeaponDef;

use super::combat::{BULLET_PRIORITY, MASK_SHOT, RIFLE_PRIORITY, bullet_damage, segment_meets_box};
use super::level::Cod4Level;

/// `penetrationDepthTable`: a depth per `penetrateType` (none, small,
/// medium, large) and surface type; row 0 and surface 0 stay 0.
pub type PenTable = [[f32; 29]; 4];

/// `perks` bit 5, `specialty_bulletpenetration` (Deep Impact).
pub const PERK_BULLET_PENETRATION: u32 = 0x20;
/// `iDFlags` 8, `level.iDFLAGS_PENETRATION`: a hit past a first surface.
pub const DFLAG_PENETRATION: i32 = 8;
/// `CONTENTS_BODY` and the hit's surface flags for a body (flesh, 7).
const CONTENTS_BODY: u32 = 0x200_0000;
const SURF_FLESH: u32 = 0x70_0000;
/// `SURF_NOPENETRATE`: such a surface stops a round.
const SURF_NOPENETRATE: u32 = 0x100;
/// Glass's contents bit: an extended round shoots on through it
/// (`cod4_lnxded` 0x808a8f4, cod4-combat.md 13.2).
const CONTENTS_GLASS: u32 = 0x10;
/// How far past a world surface the next leg starts, along the round
/// (0x3e0a3d71, read at 0x808a901), and how far short of its far side
/// the trace back for a surface's exit begins.
const PAST_SURFACE: f32 = 0.135;
const SHORT_OF_EXIT: f32 = 0.01;
/// The cosine under which a round glancing off a world surface ends.
const GRAZE_COS: f32 = 0.125;
/// The extended round's legs (0x808a893) and the surfaces a penetrating
/// round goes through past its first (0x808acfc), cod4-combat.md 3.1.
const EXTENDED_LEGS: usize = 12;
const SURFACES_PAST_FIRST: usize = 5;

/// A surface's type, bits 20..24 of its flags (`vcod_cod4::sound::SURFACES`
/// by number; 7 is flesh, [`SURF_FLESH`]).
fn surface_type(flags: u32) -> usize {
    ((flags >> 20) & 0x1f) as usize
}

/// `BG_ParsePenetrationDepthTable` over the `BULLET_PEN_TABLE` info
/// string: `small_bark\20\...` for the three types and the 28 named
/// surfaces (`vcod_cod4::sound::SURFACES`). `None` off the wrong header.
pub fn parse_pen_table(text: &str) -> Option<PenTable> {
    let mut parts = text.split('\\');
    if parts.next()?.trim() != "BULLET_PEN_TABLE" {
        return None;
    }
    let mut keys = HashMap::new();
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        keys.insert(k.trim().to_ascii_lowercase(), v.trim());
    }
    let mut t = [[0.0; 29]; 4];
    for (row, kind) in ["small", "medium", "large"].iter().enumerate() {
        for (surf, name) in vcod_cod4::sound::SURFACES.iter().enumerate().skip(1) {
            if let Some(v) = keys.get(&format!("{kind}_{name}")) {
                t[row + 1][surf] = v.parse().unwrap_or(0.0);
            }
        }
    }
    Some(t)
}

/// One leg of a bullet: what passes, where the leg runs and the damage left.
#[derive(Clone, Copy, Debug)]
struct Params {
    /// The entity the next trace passes (a client's is its slot); `None` the world, which passes nothing.
    ignore: Option<u32>,
    orig_start: Vec3,
    start: Vec3,
    end: Vec3,
    dir: Vec3,
    mult: f32,
}

/// What a leg's trace met.
#[derive(Clone, Debug, Default)]
struct Hit {
    pos: Vec3,
    normal: Vec3,
    surface_flags: u32,
    contents: u32,
    startsolid: bool,
    allsolid: bool,
    client: Option<usize>,
    /// A damageable entity the round met (`entity_damage`).
    entity: Option<vcod_gsc::EntId>,
    /// The number of whatever entity it met, `None` the world
    /// (`Trace_GetEntityHitId`).
    number: Option<u32>,
    /// On a script model, `G_Damage`'s notify names: the struck
    /// attachment's model and tag (empty for the base model) and the
    /// surface's bone (`partName`); empty elsewhere.
    names: [String; 3],
    hitloc: u8,
    depth_surface: usize,
}

impl Cod4Level {
    /// `BG_GetSurfacePenetrationDepth` for `d`, Deep Impact's
    /// `perk_bulletPenetrationMultiplier` on the shooter's perks.
    fn pen_depth(&mut self, slot: usize, d: &WeaponDef, surface: usize) -> f32 {
        let table = self
            .combat
            .penetration
            .get_or_insert_with(|| {
                let text = self
                    .rt
                    .host
                    .assets
                    .as_ref()?
                    .read("info/bullet_penetration_mp")?;
                let t = parse_pen_table(&String::from_utf8_lossy(&text));
                if t.is_none() {
                    log::warn!("cod4: info/bullet_penetration_mp is no BULLET_PEN_TABLE");
                }
                t.map(Rc::new)
            })
            .clone();
        let depth = match (&table, surface) {
            (Some(t), 1..=28) => t
                .get(d.penetrate_type as usize)
                .map_or(0.0, |row| row[surface]),
            _ => 0.0,
        };
        if self.rt.host.cod4().clients[slot].perks & PERK_BULLET_PENETRATION != 0 {
            let m = crate::game::builtins::cvar::atof(
                self.rt.host.cvars.get("perk_bulletPenetrationMultiplier"),
            );
            depth * if m == 0.0 { 2.0 } else { m }
        } else {
            depth
        }
    }

    /// `Bullet_Trace`: the clip map and the static models under the shot
    /// mask, then every playing body's hit boxes but the passed one's
    /// before the world; a body reads flesh. `last` fills a surface of
    /// type 0. `ease` steps the bodies' controls (the round's first leg).
    fn bullet_trace(
        &mut self,
        bp: &Params,
        d: &WeaponDef,
        last: usize,
        origins: &HashMap<usize, Vec3>,
        ease: bool,
    ) -> Option<Hit> {
        let priority = if d.rifle_bullet {
            &RIFLE_PRIORITY
        } else {
            &BULLET_PRIORITY
        };
        let (start, end) = (bp.start, bp.end);
        let mut t = vcod_common::clipmap::ClipTrace::clear();
        if let Some(c) = &self.clip {
            t = c.trace(
                start,
                end,
                Vec3::ZERO,
                Vec3::ZERO,
                MASK_SHOT & !vcod_cod4::pmove::CONTENTS_BODY,
            );
            c.trace_static_models(start, end, MASK_SHOT, &mut t);
        }
        let mut fraction = t.fraction;
        let mut hit = Hit {
            normal: t.normal,
            surface_flags: t.surface_flags,
            contents: t.contents,
            startsolid: t.startsolid,
            allsolid: t.allsolid,
            ..Default::default()
        };
        let mut found = fraction < 1.0 || t.startsolid;
        for slot in 0..self.players.len() {
            if Some(slot as u32) == bp.ignore || !self.playing(slot) {
                continue;
            }
            let Some(ps) = self.pm_state(slot) else {
                continue;
            };
            let origin = origins.get(&slot).copied().unwrap_or(ps.origin);
            let lo = origin + super::combat::HIT_BROAD_MINS;
            let hi = origin + super::combat::HIT_BROAD_MAXS;
            if !segment_meets_box(start, end, lo, hi, fraction) {
                continue;
            }
            if let Some(h) =
                self.body_trace_eased(slot, origin, start, end, priority, fraction, ease)
                && h.fraction < fraction
            {
                fraction = h.fraction;
                let yaw = ps.view_angles[1].to_radians();
                hit = Hit {
                    normal: glam::Quat::from_rotation_z(yaw) * h.normal,
                    surface_flags: SURF_FLESH,
                    contents: CONTENTS_BODY,
                    // `SV_ClipMoveToEntities` keeps the world's start in
                    // solid.
                    startsolid: t.startsolid || h.start_solid,
                    client: Some(slot),
                    number: Some(slot as u32),
                    hitloc: h.hit_location,
                    ..Default::default()
                };
                found = true;
            }
        }
        // The script models, open to damage or not, by their models'
        // collision surfaces (`DObjGeomTraceline` on the bind pose), and
        // the brush entities (`CM_TransformedBoxTrace`), each with its
        // surface flags (cod4-combat.md 13.1).
        for b in self.shot_boxes() {
            if Some(b.id.0) == bp.ignore {
                continue;
            }
            if let Some(m) = b.trace_surfs(start, end, fraction, MASK_SHOT)
                && m.fraction < fraction
            {
                fraction = m.fraction;
                hit = Hit {
                    normal: m.normal,
                    surface_flags: m.surface_flags,
                    contents: 0x2000,
                    entity: self.takedamage_entity(b.id.0),
                    number: Some(b.id.0),
                    names: [m.model, m.tag, m.part],
                    ..Default::default()
                };
                found = true;
            }
        }
        if let Some(c) = &self.clip {
            for b in self.brushes.iter().filter(|b| Some(b.entity) != bp.ignore) {
                if b.model == 0 || b.model >= c.submodels.len() {
                    continue;
                }
                let q = vcod_common::turretpose::angles_quat(b.angles.to_array());
                let local = |p: Vec3| q.inverse() * (p - b.origin);
                let t = c.trace_model(
                    b.model,
                    local(start),
                    local(end),
                    Vec3::ZERO,
                    Vec3::ZERO,
                    MASK_SHOT,
                );
                if t.fraction < fraction {
                    fraction = t.fraction;
                    hit = Hit {
                        normal: q * t.normal,
                        surface_flags: t.surface_flags,
                        contents: t.contents,
                        startsolid: t.startsolid,
                        allsolid: t.allsolid,
                        entity: self.takedamage_entity(b.entity),
                        number: Some(b.entity),
                        ..Default::default()
                    };
                    found = true;
                }
            }
        }
        if !found {
            return None;
        }
        hit.pos = start.lerp(end, fraction);
        hit.depth_surface = if hit.surface_flags & SURF_NOPENETRATE != 0 {
            0
        } else {
            match surface_type(hit.surface_flags) {
                0 => last,
                s => s,
            }
        };
        Some(hit)
    }

    /// `Bullet_Process` without the effect (`sv_clientSideBullets` 1, the
    /// client draws its own impacts): a body hit takes `Bullet_GetDamage`
    /// (the range falloff off the round's first start) times what is left
    /// of the round, truncated; the shooter's own body takes nothing.
    fn bullet_process(
        &mut self,
        slot: usize,
        d: &WeaponDef,
        bp: &Params,
        h: &Hit,
        dflags: i32,
        game_time: i32,
    ) {
        if let Some(id) = h.entity {
            let damage = bullet_damage(d, h.pos.distance(bp.orig_start));
            let damage = (f64::from(damage) * f64::from(bp.mult)) as i32;
            let mod_ = if d.rifle_bullet {
                "MOD_RIFLE_BULLET"
            } else {
                "MOD_PISTOL_BULLET"
            };
            let dflags = dflags | if d.armor_piercing { 2 } else { 0 };
            let [m, t, p] = &h.names;
            self.damage_entity(
                id,
                Some(slot),
                damage,
                dflags,
                mod_,
                bp.dir,
                h.pos,
                [m, t, p],
            );
            return;
        }
        let Some(victim) = h.client.filter(|&v| v != slot) else {
            return;
        };
        let damage = bullet_damage(d, h.pos.distance(bp.orig_start));
        let damage = (f64::from(damage) * f64::from(bp.mult)) as i32;
        self.damage_client(
            slot, victim, d, damage, dflags, bp.dir, h.pos, h.hitloc, game_time,
        );
    }

    /// One round from `start` toward `end`: `Bullet_FirePenetrate` while
    /// `bullet_penetrationEnabled` and the weapon has a `penetrateType`,
    /// else `Bullet_FireExtended`.
    pub(super) fn bullet_fire_round(
        &mut self,
        slot: usize,
        d: &WeaponDef,
        start: Vec3,
        end: Vec3,
        game_time: i32,
        origins: &HashMap<usize, Vec3>,
    ) {
        let mut bp = Params {
            ignore: Some(slot as u32),
            orig_start: start,
            start,
            end,
            dir: (end - start).normalize_or_zero(),
            mult: 1.0,
        };
        let enabled = self.rt.host.cvars.get("bullet_penetrationEnabled") != "0";
        if enabled && d.penetrate_type != 0 {
            self.fire_penetrate(slot, d, &mut bp, game_time, origins);
        } else {
            self.fire_extended(slot, d, &mut bp, game_time, origins);
        }
    }

    /// `Bullet_FireExtended` (penetration off, or a weapon of no
    /// `penetrateType`): each leg damages what it meets; glass lets the
    /// round on past it, and a rifle round goes on through an enemy at
    /// half strength (a teammate stops it without friendly fire). The
    /// shooter's own body passes the round untouched. Anything else ends
    /// it (cod4-combat.md 3.1, 13.2).
    fn fire_extended(
        &mut self,
        slot: usize,
        d: &WeaponDef,
        bp: &mut Params,
        game_time: i32,
        origins: &HashMap<usize, Vec3>,
    ) {
        for leg in 0..EXTENDED_LEGS {
            let Some(hit) = self.bullet_trace(bp, d, 0, origins, leg == 0) else {
                break;
            };
            self.bullet_process(slot, d, bp, &hit, 0, game_time);
            let goes_on = if hit.contents & CONTENTS_GLASS != 0 {
                advance(bp, &hit, PAST_SURFACE)
            } else {
                match hit.client {
                    Some(body) if body == slot => advance(bp, &hit, 0.0),
                    Some(body) if self.rifle_round_passes(slot, body, d) => {
                        bp.mult *= 0.5;
                        advance(bp, &hit, 0.0)
                    }
                    _ => false,
                }
            };
            if !goes_on {
                break;
            }
        }
    }

    /// Whether an extended round of `d` carries on through `body`: rifle
    /// rounds only, and never through a teammate without
    /// `scr_friendlyfire`.
    fn rifle_round_passes(&mut self, shooter: usize, body: usize, d: &WeaponDef) -> bool {
        if !d.rifle_bullet {
            return false;
        }
        let friendly_fire =
            crate::game::builtins::cvar::atoi(self.rt.host.cvars.get("scr_friendlyfire")) != 0;
        friendly_fire || !self.same_team(shooter, body)
    }

    /// `OnSameTeam`: both on one team that is not `free`.
    fn same_team(&mut self, a: usize, b: usize) -> bool {
        let ta = self.rt.client_field(a, "sessionteam");
        let tb = self.rt.client_field(b, "sessionteam");
        ta.is_some() && ta.as_deref() != Some("free") && ta == tb
    }

    /// `Bullet_FirePenetrate`: the round damages its first contact, then
    /// goes through up to five more surfaces. Each one costs the round its
    /// thickness over the lesser of the entry's and the exit's depths
    /// (`pen_depth`); a surface of no depth, a glancing hit or a spent
    /// round ends it, and whatever it reaches past a surface takes
    /// `iDFLAGS_PENETRATION` (cod4-combat.md 3.1). An entity the round
    /// struck is passed by the next leg and by the trace back, so a body
    /// or a script model costs only its surface's depth check (13.1).
    fn fire_penetrate(
        &mut self,
        slot: usize,
        d: &WeaponDef,
        bp: &mut Params,
        game_time: i32,
        origins: &HashMap<usize, Vec3>,
    ) {
        let Some(mut entry) = self.bullet_trace(bp, d, 0, origins, true) else {
            return;
        };
        self.bullet_process(slot, d, bp, &entry, 0, game_time);
        for _ in 0..SURFACES_PAST_FIRST {
            let entry_depth = self.pen_depth(slot, d, entry.depth_surface);
            if entry_depth <= 0.0 {
                return;
            }
            let entered_at = entry.pos;
            if !advance(bp, &entry, PAST_SURFACE) {
                return;
            }
            let beyond = self.bullet_trace(bp, d, entry.depth_surface, origins, false);
            // Through the wall when no exit is found, else only if the
            // round is left with something and the wall was not all solid.
            let reaches_beyond = match self.find_exit(bp, d, entered_at, beyond.as_ref(), origins) {
                None => true,
                Some(exit) => {
                    let depth = entry_depth.min(self.pen_depth(slot, d, exit.surface));
                    if depth <= 0.0 {
                        return;
                    }
                    bp.mult -= exit.thickness / depth;
                    if bp.mult <= 0.0 {
                        return;
                    }
                    !exit.solid
                }
            };
            let Some(next) = beyond else {
                return;
            };
            if reaches_beyond {
                self.bullet_process(slot, d, bp, &next, DFLAG_PENETRATION, game_time);
            }
            entry = next;
        }
    }

    /// The far side of the surface the round entered at `entered_at`: a
    /// trace back toward the entry, from just short of the next contact
    /// (`beyond`) or from the round's end, stopping 0.01 short of the
    /// entry. `None` when the trace back meets nothing.
    fn find_exit(
        &mut self,
        bp: &Params,
        d: &WeaponDef,
        entered_at: Vec3,
        beyond: Option<&Hit>,
        origins: &HashMap<usize, Vec3>,
    ) -> Option<Exit> {
        let back = -bp.dir;
        let mut ray = Params {
            dir: back,
            end: entered_at + back * SHORT_OF_EXIT,
            ..*bp
        };
        if let Some(next) = beyond {
            ray.ignore = next.number;
            ray.start = start_past(next.pos, -next.normal, next.number, back, SHORT_OF_EXIT).0;
        } else {
            ray.start = bp.end;
        }
        let last = beyond.map_or(0, |n| n.depth_surface);
        let hit = self.bullet_trace(&ray, d, last, origins, false)?;
        let solid = hit.allsolid || (beyond.is_some_and(|n| n.startsolid) && hit.startsolid);
        let thickness = if solid {
            ray.end.distance(ray.start)
        } else {
            entered_at.distance(hit.pos)
        };
        Some(Exit {
            thickness: thickness.max(1.0),
            surface: hit.depth_surface,
            solid,
        })
    }
}

/// A penetrated surface's far side ([`Cod4Level::find_exit`]): how thick
/// it was along the round (at least 1), the exit's depth surface, and
/// whether the trace back ran all in solid (the round does not reach what
/// lies past it).
struct Exit {
    thickness: f32,
    surface: usize,
    solid: bool,
}

#[cfg(test)]
impl Cod4Level {
    /// One round of the weapon named `weapon` from `start` toward `end`,
    /// fired by `slot` at the level's time with the bodies where they
    /// stand; for tests.
    pub fn fire_round_for_test(&mut self, slot: usize, weapon: &str, start: Vec3, end: Vec3) {
        self.sync_defs();
        let d = self
            .combat
            .defs
            .iter()
            .find(|d| d.name == weapon)
            .cloned()
            .unwrap_or_else(|| panic!("no weapon {weapon}"));
        let now = self.level_time();
        self.bullet_fire_round(slot, &d, start, end, now, &HashMap::new());
    }
}

impl Cod4Level {
    /// The clip map alone under the shot mask, for the tests.
    pub fn shot_trace_for_test(&self, start: Vec3, end: Vec3) -> vcod_common::clipmap::ClipTrace {
        let mut t = vcod_common::clipmap::ClipTrace::clear();
        if let Some(c) = &self.clip {
            t = c.trace(
                start,
                end,
                Vec3::ZERO,
                Vec3::ZERO,
                MASK_SHOT & !vcod_cod4::pmove::CONTENTS_BODY,
            );
            c.trace_static_models(start, end, MASK_SHOT, &mut t);
        }
        t
    }
}

/// `BG_AdvanceTrace` (`cod4_lnxded` 0x8060e44): the next leg passes
/// what `h` struck and starts past it ([`start_past`]). False when the
/// round glanced off and ends.
fn advance(bp: &mut Params, h: &Hit, dist: f32) -> bool {
    let (start, goes_on) = start_past(h.pos, h.normal, h.number, bp.dir, dist);
    bp.ignore = h.number;
    bp.start = start;
    goes_on
}

/// Where a leg along `dir` starts after a contact at `at` whose face
/// points along `normal`: off an entity (`struck` names one) at the
/// contact itself, off the world `dist` along `dir` stretched by the
/// cosine to the face. Under [`GRAZE_COS`] the stretch is capped and the
/// round ends (cod4-combat.md 13.1).
fn start_past(at: Vec3, normal: Vec3, struck: Option<u32>, dir: Vec3, dist: f32) -> (Vec3, bool) {
    if struck.is_some() || dist <= 0.0 {
        return (at, true);
    }
    let cos = -normal.dot(dir);
    let grazed = cos < GRAZE_COS;
    let along = if grazed { GRAZE_COS } else { cos };
    (at + dir * (dist / along), !grazed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reads_its_three_rows() {
        let t = parse_pen_table(
            "BULLET_PEN_TABLE\\small_bark\\20\\medium_brick\\20\\large_paintedmetal\\12",
        )
        .unwrap();
        assert_eq!(t[1][1], 20.0);
        assert_eq!(t[2][2], 20.0);
        assert_eq!(t[3][28], 12.0);
        assert_eq!(t[0][1], 0.0);
    }

    #[test]
    fn a_grazing_hit_ends_the_round() {
        let mut bp = Params {
            ignore: Some(0),
            orig_start: Vec3::ZERO,
            start: Vec3::ZERO,
            end: Vec3::X * 100.0,
            dir: Vec3::X,
            mult: 1.0,
        };
        let h = Hit {
            pos: Vec3::X * 10.0,
            normal: Vec3::new(-0.1, 0.995, 0.0).normalize(),
            ..Default::default()
        };
        assert!(!advance(&mut bp, &h, 0.135));
        let h = Hit {
            normal: -Vec3::X,
            ..h
        };
        assert!(advance(&mut bp, &h, 0.135));
        assert!((bp.start.x - 10.135).abs() < 1e-4);
        assert_eq!(bp.ignore, None);
    }
}
