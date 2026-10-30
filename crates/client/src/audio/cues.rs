//! `GameEvent` to sound-cue resolution
//! (docs/research/cod11-sound-system.md, sections 6-8). Pure; everything
//! comes in through [`CueCtx`].
//!
//! As in retail: no `_default` alias fallback (section 7) and no own-entity
//! special case (section 5). The alias row owns the channel, so a [`Cue`]
//! never carries one.

use std::collections::HashMap;

use glam::Vec3;

use crate::fx::registry::*;
use vcod_common::net::events::GameEvent;
use vcod_common::weapon::WeaponSounds;

/// First of the 256 sound-alias configstrings (section 9). `EV_SOUND_ALIAS`
/// `eventParm` indexes it; 0 means no alias.
pub use vcod_common::net::protocol::CS_SOUNDS;

/// Entity number for the map ambient, so no cue can end it (retail uses
/// reserved stream slots instead, section 2). Outside 0..1024 and distinct
/// from the `u32::MAX` sentinel [`ps_entity`] returns.
pub const AMBIENT_ENTITY: u32 = 0xFFFF_FFFE;

/// Entity number for `musicPlay`'s track, retail's reserved stream slot
/// 0x20 (research doc, section 10).
pub const MUSIC_ENTITY: u32 = 0xFFFF_FFFD;

/// CoD2's second ambient stream slot, where a crossfade starts the next
/// track while the old one fades out on [`AMBIENT_ENTITY`] (or the other
/// way round; `SND_PlayAmbientAlias`, Mac 0x4c806).
pub const AMBIENT_ENTITY_2: u32 = 0xFFFF_FFFC;

/// Entity whose playerState we ride; `u32::MAX` when there is none, which
/// matches no entity.
pub fn ps_entity(client_num: i32) -> u32 {
    if client_num >= 0 {
        client_num as u32
    } else {
        u32::MAX
    }
}

/// Closest-point margin from either end of the shot (`cgame 0x300695a0`, section 6).
pub const WHIZBY_MARGIN: f32 = 64.0;
/// Max listener distance to the closest point (`cgame 0x30069598`, section 6).
pub const WHIZBY_RADIUS: f32 = 140.0;
/// Emitter set back toward the shooter so it pans off-center (`cgame 0x30069468`).
pub const WHIZBY_SETBACK: f32 = 16.0;

/// Alias-name surface suffixes in `surfType` order (section 7a). Index 22 is
/// the csv's `asphault`, not the engine's `asphalt`, on purpose: retail asks
/// for a name no csv row has and is silent there. `canvas` is not a surface.
const ALIAS_SURFACES: [&str; 23] = [
    "default", "bark", "brick", "carpet", "cloth", "concrete", "dirt", "flesh", "foliage", "glass",
    "grass", "gravel", "ice", "metal", "mud", "paper", "plaster", "rock", "sand", "snow", "water",
    "wood", "asphault",
];

/// No `Local` variant: 2D playback is the alias row's channel (section 2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    /// Played as `ENTITYNUM_WORLD` in retail.
    Point(Vec3),
    /// `pos` is the position at fire time.
    Entity { num: u32, pos: Vec3 },
    /// Started at `pos`, a tag on entity `num`, and carried with the entity
    /// at that offset from where it was drawn (`FUN_0044c040` keeps the
    /// start as an entity-space offset, section 3).
    Tag { num: u32, pos: Vec3 },
}

impl Source {
    /// Where the emitter is now.
    pub fn pos(&self) -> Vec3 {
        match *self {
            Source::Point(p) | Source::Entity { pos: p, .. } | Source::Tag { pos: p, .. } => p,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub alias: String,
    pub source: Source,
    /// Seconds before start. Always 0 today.
    pub delay_s: f32,
}

pub struct CueCtx<'a> {
    pub configstrings: &'a [String],
    /// The connection's `CS_SOUNDS` (`Protocol::cs`).
    pub sounds: usize,
    /// Keyed by 1-based CS7 weapon index; `None` = weapon file failed to load.
    pub weapon_sounds: &'a HashMap<i32, Option<WeaponSounds>>,
    /// `(position, forward)` per entity this frame; the whizby start point.
    pub muzzles: &'a HashMap<u32, (Vec3, Vec3)>,
    /// Bodies with no muzzle tag: `CG_CalcMuzzlePoint`'s stance point
    /// (`fx::registry::muzzle_fallback`), the whizby's start in its place.
    pub shot_fallbacks: &'a HashMap<u32, Vec3>,
    pub listener_pos: Vec3,
    /// Playerstate-ring events carry no entity and are attributed to this one.
    pub ps_entity: u32,
    /// A United Offensive connection: 128 weapon items at the head of
    /// `bg_itemlist` (64 elsewhere), and the sprint's own footsteps.
    pub uo: bool,
}

fn surface(index: i32) -> &'static str {
    ALIAS_SURFACES[index.clamp(0, ALIAS_SURFACES.len() as i32 - 1) as usize]
}

fn with_surface(prefix: &str, surf: i32) -> String {
    format!("{prefix}_{}", surface(surf))
}

fn cue(alias: impl Into<String>, source: Source) -> Cue {
    Cue {
        alias: alias.into(),
        source,
        delay_s: 0.0,
    }
}

/// Playerstate-ring events (`entity_num == u32::MAX`) land on the ridden
/// entity at the listener.
fn entity_source(ev: &GameEvent, ctx: &CueCtx) -> Source {
    if ev.entity_num == u32::MAX {
        Source::Entity {
            num: ctx.ps_entity,
            pos: ctx.listener_pos,
        }
    } else {
        Source::Entity {
            num: ev.entity_num,
            pos: Vec3::from(ev.pos),
        }
    }
}

/// `CG_FireWeapon`'s sound origin (1.1 0x30038b70, 1.5 0x3003a2a0, UO
/// 0x30047d20): another entity's world `tag_flash`, the ridden body's
/// viewmodel `tag_flash` (the muzzle map keys the view's under both
/// `u32::MAX` and the ridden number), else the entity's position.
fn fire_source(ev: &GameEvent, ctx: &CueCtx) -> Source {
    let own = ev.entity_num == u32::MAX || ev.entity_num == ctx.ps_entity;
    match ctx.muzzles.get(&ev.entity_num) {
        Some(&(pos, _)) => Source::Tag {
            num: if own { ctx.ps_entity } else { ev.entity_num },
            pos,
        },
        None => entity_source(ev, ctx),
    }
}

fn weapon<'a>(ctx: &'a CueCtx, index: i32) -> Option<&'a WeaponSounds> {
    ctx.weapon_sounds.get(&index).and_then(Option::as_ref)
}

/// `CG_BulletWhizby` (`cgame 0x30038dc0`, section 6). Emitter position, or
/// `None` when the shot passes too far or misses the end margins.
fn whizby_point(start: Vec3, impact: Vec3, listener: Vec3) -> Option<Vec3> {
    let seg = impact - start;
    let len = seg.length();
    if len <= 0.0 {
        return None;
    }
    let u = seg / len;
    let t = (listener - start).dot(u);
    if t < WHIZBY_MARGIN || t + WHIZBY_MARGIN > len {
        return None;
    }
    let closest = start + u * t;
    if closest.distance(listener) > WHIZBY_RADIUS {
        return None;
    }
    Some(closest - u * WHIZBY_SETBACK)
}

/// The events `CG_FireWeapon` runs for.
pub fn fires_weapon(event: i32) -> bool {
    matches!(
        event,
        EV_FIRE_WEAPON
            | EV_FIRE_WEAPONB
            | EV_UO_FIRE_WEAPONC
            | EV_FIRE_WEAPON_LASTSHOT
            | EV_FIRE_WEAPON_MG42
            | EV_FIRE_QUADBARREL_1
            | EV_FIRE_QUADBARREL_2
    )
}

/// United Offensive's explode alias tables by explosion type (uo cgame
/// 0x3044bc30, 23 surfaces a row): only these five rows are registered.
fn uo_explode_prefix(kind: i32) -> Option<&'static str> {
    Some(match kind {
        0 => "grenade_explode",
        2 => "rocket_explode",
        4 => "artillery_explode",
        5 => "mortar_explode",
        6 => "tank_explode",
        _ => return None,
    })
}

/// The cues one [`GameEvent`] starts, in `CG_EntityEvent` order.
pub fn resolve(ev: &GameEvent, ctx: &CueCtx) -> Vec<Cue> {
    let mut out = Vec::new();
    let e = ev.event;

    // ids 1-138: six groups of 23 keyed by `event - base`, not `surfType`
    // (section 7b). Jump replays the run aliases; there is no `jump_*` alias.
    const GROUPS: [(i32, &str, Option<&str>); 6] = [
        (EV_FOOTSTEP_RUN_BASE, "step_run", Some("gear_rattle_run")),
        (EV_FOOTSTEP_WALK_BASE, "step_walk", Some("gear_rattle_walk")),
        (
            EV_FOOTSTEP_PRONE_BASE,
            "step_prone",
            Some("gear_rattle_walk"),
        ),
        (EV_JUMP_BASE, "step_run", Some("gear_rattle_run")),
        (EV_LANDING_BASE, "land", None),
        (EV_LANDING_PAIN_BASE, "land", Some("land_damage")),
    ];
    for (base, prefix, extra) in GROUPS {
        if (base..base + SURFACE_GROUP_SIZE).contains(&e) {
            // UO's sprint steps (and so its jumps) ride the jump's ids
            // (`uo_cgame_mp_x86.dll` 0x3002289c, coduo-movement.md 3).
            let (prefix, extra) = if ctx.uo && base == EV_JUMP_BASE {
                ("step_sprint", Some("gear_rattle_sprint"))
            } else {
                (prefix, extra)
            };
            let src = entity_source(ev, ctx);
            out.push(cue(with_surface(prefix, e - base), src));
            if let Some(extra) = extra {
                out.push(cue(extra, src));
            }
            return out;
        }
    }

    match e {
        // body / movement, section 7c
        EV_FOLIAGE_SOUND => out.push(cue("movement_foliage", entity_source(ev, ctx))),
        EV_WATER_TOUCH => out.push(cue("player_water_in", entity_source(ev, ctx))),
        EV_WATER_LEAVE => out.push(cue("player_water_out", entity_source(ev, ctx))),
        EV_FLAMEBARREL_BOUNCE => out.push(cue("flamebarrel_bounce", entity_source(ev, ctx))),

        // pickups, section 7d. `eventParm` is the item index; 1-64 are weapons.
        // `EV_AMMO_PICKUP` reads the same column, so `ammoPickupSound` is never heard.
        EV_ITEM_PICKUP | EV_AMMO_PICKUP => {
            // United Offensive's `bg_itemlist` holds 128 weapons ahead of
            // the same two grenade and three health items (coduo-diff.md 12).
            let w = if ctx.uo { 128 } else { 64 };
            let alias = match ev.parm {
                p if (1..=w).contains(&p) => Some(
                    weapon(ctx, ev.parm)
                        .and_then(|w| w.pickup.clone())
                        .unwrap_or_else(|| "weap_pickup".to_string()),
                ),
                p if p == w + 1 || p == w + 2 => Some("grenade_pickup".to_string()),
                p if p == w + 3 => Some("health_pickup_small".to_string()),
                p if p == w + 4 => Some("health_pickup_medium".to_string()),
                p if p == w + 5 => Some("health_pickup_large".to_string()),
                _ => None,
            };
            if let Some(alias) = alias {
                out.push(cue(alias, entity_source(ev, ctx)));
            }
        }

        // weapon handling, sections 7c and 8
        EV_NOAMMO => {
            if !weapon(ctx, ev.weapon).is_some_and(|w| w.clip_only) {
                out.push(cue("player_out_of_ammo", entity_source(ev, ctx)));
            }
        }
        EV_RELOAD | EV_RELOAD_FROM_EMPTY | EV_RELOAD_START | EV_RELOAD_END | EV_RAISE_WEAPON
        | EV_PUTAWAY_WEAPON | EV_WEAPON_ALT | EV_PULLBACK_WEAPON | EV_RECHAMBER_WEAPON => {
            let w = weapon(ctx, ev.weapon);
            let alias = match e {
                EV_RELOAD => w.and_then(|w| w.reload.clone().or_else(|| w.reload_empty.clone())),
                EV_RELOAD_FROM_EMPTY => {
                    w.and_then(|w| w.reload_empty.clone().or_else(|| w.reload.clone()))
                }
                EV_RELOAD_START => w.and_then(|w| w.reload_start.clone()),
                EV_RELOAD_END => w.and_then(|w| w.reload_end.clone()),
                EV_RAISE_WEAPON => Some(
                    w.and_then(|w| w.raise.clone())
                        .unwrap_or_else(|| "weap_raise".to_string()),
                ),
                EV_PUTAWAY_WEAPON => Some(
                    w.and_then(|w| w.putaway.clone())
                        .unwrap_or_else(|| "weap_putaway".to_string()),
                ),
                EV_WEAPON_ALT => w.and_then(|w| w.alt_switch.clone()),
                EV_PULLBACK_WEAPON => w.and_then(|w| w.pullback.clone()),
                _ => w.and_then(|w| w.rechamber.clone()),
            };
            if let Some(alias) = alias {
                out.push(cue(alias, entity_source(ev, ctx)));
            }
        }
        // `CG_FireWeapon` (section 8): twice for the quad-barrel's two tags.
        // `loopFireSound` and `stopFireSound` are unused by the 1.1 MP client.
        EV_FIRE_WEAPON
        | EV_FIRE_WEAPONB
        | EV_FIRE_WEAPON_MG42
        | EV_FIRE_WEAPON_LASTSHOT
        | EV_FIRE_QUADBARREL_1
        | EV_FIRE_QUADBARREL_2 => {
            let alias = weapon(ctx, fire_weapon(ev)).and_then(|w| {
                if e == EV_FIRE_WEAPON_LASTSHOT {
                    w.last_shot.clone().or_else(|| w.fire.clone())
                } else {
                    w.fire.clone()
                }
            });
            if let Some(alias) = alias {
                let src = fire_source(ev, ctx);
                let barrels = match e {
                    EV_FIRE_QUADBARREL_1 | EV_FIRE_QUADBARREL_2 => 2,
                    _ => 1,
                };
                for _ in 0..barrels {
                    out.push(cue(alias.clone(), src));
                }
            }
        }

        // melee, section 7c
        EV_MELEE_SWIPE => {
            let alias = if weapon(ctx, ev.weapon).is_some_and(|w| w.rifle_bullet) {
                "melee_swing_large"
            } else {
                "melee_swing_small"
            };
            out.push(cue(alias, entity_source(ev, ctx)));
        }
        // On the victim (`otherEntityNum`).
        EV_MELEE_HIT => out.push(cue(
            "melee_hit",
            Source::Entity {
                num: ev.other_entity_num,
                pos: Vec3::from(ev.pos),
            },
        )),

        // scripted sounds, sections 7c and 9
        EV_SOUND_ALIAS => {
            let index = ev.parm.clamp(0, 255);
            if index > 0 {
                let name = ctx
                    .configstrings
                    .get(ctx.sounds + index as usize)
                    .map(String::as_str)
                    .unwrap_or("");
                if !name.is_empty() {
                    out.push(cue(name, entity_source(ev, ctx)));
                }
            }
        }

        // impacts, sections 6, 7a and 7c. The whizby test runs for every
        // surface, flesh included, from the shooter's (`otherEntityNum`)
        // muzzle; no muzzle yet means no whizby. The client-hit pair is the
        // victim's copy of a flesh hit and reads the same alias tables.
        EV_BULLET_HIT_SMALL
        | EV_BULLET_HIT_LARGE
        | EV_BULLET_HIT_CLIENT_SMALL
        | EV_BULLET_HIT_CLIENT_LARGE => {
            let prefix = if matches!(e, EV_BULLET_HIT_SMALL | EV_BULLET_HIT_CLIENT_SMALL) {
                "bullet_small"
            } else {
                "bullet_large"
            };
            let impact = Vec3::from(ev.pos);
            out.push(cue(
                with_surface(prefix, ev.surf_type),
                Source::Point(impact),
            ));
            if let Some(start) = shot_origin(ctx.muzzles, ctx.shot_fallbacks, ev.other_entity_num)
                && let Some(at) = whizby_point(start, impact, ctx.listener_pos)
            {
                out.push(cue("whizby", Source::Point(at)));
            }
        }
        // United Offensive's satchel (`cloth`) bounces with its own alias
        // (uo cgame 0x3002323f).
        EV_GRENADE_BOUNCE => {
            let alias = match weapon(ctx, ev.weapon).is_some_and(|w| w.cloth) {
                true => "satchel_bounce".to_string(),
                false => with_surface("grenade_bounce", ev.surf_type),
            };
            out.push(cue(alias, Source::Point(Vec3::from(ev.pos))));
        }
        EV_GRENADE_EXPLODE | EV_ROCKET_EXPLODE | EV_ROCKET_EXPLODE_NOMARKS => {
            let prefix = if e == EV_GRENADE_EXPLODE {
                "grenade_explode"
            } else {
                "rocket_explode"
            };
            let at = Source::Point(Vec3::from(ev.pos));
            out.push(cue(with_surface(prefix, ev.surf_type), at));
            if let Some(alias) = weapon(ctx, ev.weapon).and_then(|w| w.proj_explosion.clone()) {
                out.push(cue(alias, at));
            }
        }

        // United Offensive (`CG_EntityEvent`, uo cgame 0x30022810, and its
        // `CG_RegisterSounds` 0x3002b560; docs/research/coduo-diff.md 8).
        EV_UO_FATIGUE_LAST_SOUND => out.push(cue("fatigue_breath", entity_source(ev, ctx))),
        EV_UO_FATIGUE_SOUND => out.push(cue("sprint_breath_last", entity_source(ev, ctx))),
        EV_UO_DEPLOY_WEAPON | EV_UO_BREAKDOWN_WEAPON => {
            let alias = weapon(ctx, ev.weapon).and_then(|w| match e {
                EV_UO_DEPLOY_WEAPON => w.deploy.clone(),
                _ => w.breakdown.clone(),
            });
            if let Some(alias) = alias {
                out.push(cue(alias, entity_source(ev, ctx)));
            }
        }
        EV_UO_FIRE_WEAPONC => {
            if let Some(alias) = weapon(ctx, fire_weapon(ev)).and_then(|w| w.fire.clone()) {
                out.push(cue(alias, entity_source(ev, ctx)));
            }
        }
        // `smg` and `pistol` rounds hit small, the rest large (0x30048ef3).
        EV_UO_BULLET_HIT => {
            let small = weapon(ctx, ev.weapon)
                .is_some_and(|w| matches!(w.ammo_type.as_str(), "smg" | "pistol"));
            let prefix = if small {
                "bullet_small"
            } else {
                "bullet_large"
            };
            let impact = Vec3::from(ev.pos);
            out.push(cue(
                with_surface(prefix, ev.surf_type),
                Source::Point(impact),
            ));
            if let Some(start) = shot_origin(ctx.muzzles, ctx.shot_fallbacks, ev.other_entity_num)
                && let Some(at) = whizby_point(start, impact, ctx.listener_pos)
            {
                out.push(cue("whizby", Source::Point(at)));
            }
        }
        EV_UO_GRENADE_SPOON => out.push(cue("us_grenade_lever", Source::Point(Vec3::from(ev.pos)))),
        // The parm is the explosion type; only artillery (4) and mortar (5)
        // register a sound.
        EV_UO_PROJECTILE_LAUNCH | EV_UO_PROJECTILE_INCOMING if matches!(ev.parm, 4 | 5) => {
            let alias = match e {
                EV_UO_PROJECTILE_LAUNCH => "shell_flash",
                _ => "barrage_incoming",
            };
            out.push(cue(alias, Source::Point(Vec3::from(ev.pos))));
        }
        EV_UO_PROJECTILE_EXPLODE | EV_UO_PROJECTILE_EXPLODE_NOMARKS => {
            let at = Source::Point(Vec3::from(ev.pos));
            if let Some(prefix) = uo_explode_prefix(ev.scale) {
                out.push(cue(with_surface(prefix, ev.surf_type), at));
            }
            if let Some(alias) = weapon(ctx, ev.weapon).and_then(|w| w.proj_explosion.clone()) {
                out.push(cue(alias, at));
            }
        }
        // A carrier whose `eType` has a bit of 0xc (a turret, a vehicle)
        // overheats with the vehicle's alias (0x300232b4).
        EV_UO_OVERHEATING => {
            let alias = if ev.e_type & 0xc != 0 {
                "mg_overheat_vehicle"
            } else {
                "mg_overheat"
            };
            out.push(cue(alias, Source::Point(Vec3::from(ev.pos))));
        }

        // Everything else is silent in the 1.1 MP client (section 7c).
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: i32) -> GameEvent {
        GameEvent {
            event,
            parm: 0,
            entity_num: 5,
            client_num: 5,
            weapon: 0,
            surf_type: 0,
            pos: [10.0, 20.0, 30.0],
            dir: [0.0; 3],
            scale: 0,
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
            e_type: 0,
            eflags: 0,
        }
    }

    static EMPTY_FALLBACKS: std::sync::OnceLock<HashMap<u32, Vec3>> = std::sync::OnceLock::new();

    fn ctx<'a>(
        cs: &'a [String],
        ws: &'a HashMap<i32, Option<WeaponSounds>>,
        mz: &'a HashMap<u32, (Vec3, Vec3)>,
    ) -> CueCtx<'a> {
        CueCtx {
            configstrings: cs,
            sounds: CS_SOUNDS,
            weapon_sounds: ws,
            muzzles: mz,
            shot_fallbacks: EMPTY_FALLBACKS.get_or_init(HashMap::new),
            listener_pos: Vec3::ZERO,
            ps_entity: 2,
            uo: false,
        }
    }

    fn names(cues: &[Cue]) -> Vec<&str> {
        cues.iter().map(|c| c.alias.as_str()).collect()
    }

    const EV_POS: Vec3 = Vec3::new(10.0, 20.0, 30.0);

    #[test]
    fn run_footstep_plays_surface_step_then_gear_rattle() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let e = ev(EV_FOOTSTEP_RUN_BASE + 2); // brick
        let cues = resolve(&e, &ctx(&cs, &ws, &mz));
        assert_eq!(names(&cues), vec!["step_run_brick", "gear_rattle_run"]);
        for c in &cues {
            assert!(matches!(c.source, Source::Entity { num: 5, pos } if pos == EV_POS));
        }
    }

    #[test]
    fn walk_and_prone_use_their_own_step_prefix_and_walk_rattle() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let walk = resolve(&ev(EV_FOOTSTEP_WALK_BASE + 6), &ctx(&cs, &ws, &mz));
        assert_eq!(names(&walk), vec!["step_walk_dirt", "gear_rattle_walk"]);
        let prone = resolve(&ev(EV_FOOTSTEP_PRONE_BASE + 6), &ctx(&cs, &ws, &mz));
        assert_eq!(names(&prone), vec!["step_prone_dirt", "gear_rattle_walk"]);
    }

    #[test]
    fn jump_group_reuses_the_run_aliases() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let cues = resolve(&ev(EV_JUMP_BASE + 6), &ctx(&cs, &ws, &mz));
        assert_eq!(names(&cues), vec!["step_run_dirt", "gear_rattle_run"]);
        // On UO the group is the sprint's step.
        let uo = CueCtx {
            uo: true,
            ..ctx(&cs, &ws, &mz)
        };
        let cues = resolve(&ev(EV_JUMP_BASE + 6), &uo);
        assert_eq!(names(&cues), vec!["step_sprint_dirt", "gear_rattle_sprint"]);
    }

    #[test]
    fn landing_and_landing_pain() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let land = resolve(&ev(EV_LANDING_BASE + 6), &ctx(&cs, &ws, &mz));
        assert_eq!(names(&land), vec!["land_dirt"]);
        let pain = resolve(&ev(EV_LANDING_PAIN_BASE + 6), &ctx(&cs, &ws, &mz));
        assert_eq!(names(&pain), vec!["land_dirt", "land_damage"]);
    }

    #[test]
    fn asphalt_uses_the_csv_misspelling() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let cues = resolve(&ev(EV_FOOTSTEP_RUN_BASE + 22), &ctx(&cs, &ws, &mz));
        assert_eq!(cues[0].alias, "step_run_asphault");
    }

    #[test]
    fn fire_uses_the_weapon_cache_on_the_shooter() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            3,
            Some(WeaponSounds {
                fire: Some("weap_kar98k_fire".into()),
                ..Default::default()
            }),
        );
        let mz = HashMap::new();
        let mut e = ev(EV_FIRE_WEAPON);
        e.weapon = 3;
        let cues = resolve(&e, &ctx(&cs, &ws, &mz));
        assert_eq!(names(&cues), vec!["weap_kar98k_fire"]);
        assert!(matches!(cues[0].source, Source::Entity { num: 5, pos } if pos == EV_POS));
    }

    /// `CG_FireWeapon` starts the sound at the shooter's world `tag_flash`
    /// (1.1 0x30038c9d, 1.5 0x3003a2a0), the ridden body's at its
    /// viewmodel's (0x30038c55), on the ridden entity either way.
    #[test]
    fn fire_starts_at_the_shooters_flash_tag() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            3,
            Some(WeaponSounds {
                fire: Some("weap_mg42_fire".into()),
                ..Default::default()
            }),
        );
        let flash = Vec3::new(40.0, 20.0, 80.0);
        let mz = HashMap::from([(5, (flash, Vec3::X)), (2, (Vec3::ONE, Vec3::X))]);
        let mut e = ev(EV_FIRE_WEAPON_MG42);
        e.weapon = 3;
        let cues = resolve(&e, &ctx(&cs, &ws, &mz));
        assert_eq!(cues[0].source, Source::Tag { num: 5, pos: flash });
        let view = Vec3::new(1.0, 2.0, 3.0);
        let mz = HashMap::from([
            (5, (flash, Vec3::X)),
            (2, (view, Vec3::X)),
            (u32::MAX, (view, Vec3::X)),
        ]);
        for num in [2, u32::MAX] {
            e.entity_num = num;
            let cues = resolve(&e, &ctx(&cs, &ws, &mz));
            assert_eq!(cues[0].source, Source::Tag { num: 2, pos: view });
        }
        // Under a scope the view keys no muzzle: the listener.
        let cues = resolve(&e, &ctx(&cs, &ws, &HashMap::new()));
        assert!(matches!(cues[0].source, Source::Entity { num: 2, .. }));
    }

    #[test]
    fn ps_ring_events_resolve_to_the_ridden_entity_at_the_listener() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            3,
            Some(WeaponSounds {
                fire: Some("weap_kar98k_fire".into()),
                ..Default::default()
            }),
        );
        let mz = HashMap::new();
        let mut e = ev(EV_FIRE_WEAPON);
        e.weapon = 3;
        e.entity_num = u32::MAX;
        let mut c = ctx(&cs, &ws, &mz);
        c.listener_pos = Vec3::new(1.0, 2.0, 3.0);
        let cues = resolve(&e, &c);
        assert!(matches!(
            cues[0].source,
            Source::Entity { num: 2, pos } if pos == Vec3::new(1.0, 2.0, 3.0)
        ));
    }

    #[test]
    fn fire_without_a_cached_weapon_yields_nothing() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let mut e = ev(EV_FIRE_WEAPON);
        e.weapon = 9;
        assert!(resolve(&e, &ctx(&cs, &ws, &mz)).is_empty());
    }

    #[test]
    fn last_shot_falls_back_to_fire() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            3,
            Some(WeaponSounds {
                fire: Some("weap_kar98k_fire".into()),
                ..Default::default()
            }),
        );
        let mz = HashMap::new();
        let mut e = ev(EV_FIRE_WEAPON_LASTSHOT);
        e.weapon = 3;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["weap_kar98k_fire"]
        );
        ws.insert(
            3,
            Some(WeaponSounds {
                fire: Some("weap_kar98k_fire".into()),
                last_shot: Some("weap_kar98k_lastshot".into()),
                ..Default::default()
            }),
        );
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["weap_kar98k_lastshot"]
        );
    }

    #[test]
    fn quadbarrel_fires_the_same_alias_twice() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            4,
            Some(WeaponSounds {
                fire: Some("weap_flak_fire".into()),
                ..Default::default()
            }),
        );
        let mz = HashMap::new();
        let mut e = ev(EV_FIRE_QUADBARREL_1);
        e.weapon = 4;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["weap_flak_fire", "weap_flak_fire"]
        );
    }

    #[test]
    fn raise_falls_back_to_weap_raise_without_a_weapon() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let mut e = ev(EV_RAISE_WEAPON);
        e.weapon = 3;
        assert_eq!(names(&resolve(&e, &ctx(&cs, &ws, &mz))), vec!["weap_raise"]);
        let mut e = ev(EV_PUTAWAY_WEAPON);
        e.weapon = 3;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["weap_putaway"]
        );
        let mut e = ev(EV_RELOAD);
        e.weapon = 3;
        assert!(resolve(&e, &ctx(&cs, &ws, &mz)).is_empty());
    }

    #[test]
    fn pickups_map_the_item_index() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(3, Some(WeaponSounds::default()));
        let mz = HashMap::new();
        let mut e = ev(EV_ITEM_PICKUP);
        e.parm = 66;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["grenade_pickup"]
        );
        e.parm = 3; // weapon item whose weapon file has no pickupSound
        let cues = resolve(&e, &ctx(&cs, &ws, &mz));
        assert_eq!(names(&cues), vec!["weap_pickup"]);
        assert!(matches!(cues[0].source, Source::Entity { num: 5, .. }));
        e.parm = 68;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["health_pickup_medium"]
        );
        // UO's list puts 128 weapons first.
        let uo = CueCtx {
            uo: true,
            ..ctx(&cs, &ws, &mz)
        };
        e.parm = 100;
        assert_eq!(names(&resolve(&e, &uo)), vec!["weap_pickup"]);
        e.parm = 133;
        assert_eq!(names(&resolve(&e, &uo)), vec!["health_pickup_large"]);
        let mut e = ev(EV_AMMO_PICKUP);
        e.parm = 66;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["grenade_pickup"]
        );
        e.parm = 70;
        assert!(resolve(&e, &ctx(&cs, &ws, &mz)).is_empty());
    }

    #[test]
    fn sound_alias_reads_cs_524_plus_parm_on_the_entity() {
        let mut cs = vec![String::new(); 800];
        cs[CS_SOUNDS + 7] = "mp_bomb_plant".to_string();
        let (ws, mz) = (HashMap::new(), HashMap::new());
        let mut e = ev(EV_SOUND_ALIAS);
        e.parm = 7;
        let cues = resolve(&e, &ctx(&cs, &ws, &mz));
        assert_eq!(names(&cues), vec!["mp_bomb_plant"]);
        assert!(matches!(cues[0].source, Source::Entity { num: 5, pos } if pos == EV_POS));
        e.parm = 8; // empty configstring
        assert!(resolve(&e, &ctx(&cs, &ws, &mz)).is_empty());
        e.parm = 0; // index 0 is "no alias"
        assert!(resolve(&e, &ctx(&cs, &ws, &mz)).is_empty());
    }

    #[test]
    fn bullet_hits_play_the_surface_alias_at_the_impact() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let mut e = ev(EV_BULLET_HIT_SMALL);
        e.surf_type = 22;
        let cues = resolve(&e, &ctx(&cs, &ws, &mz));
        assert_eq!(names(&cues), vec!["bullet_small_asphault"]);
        assert!(matches!(cues[0].source, Source::Point(p) if p == EV_POS));
        let mut e = ev(EV_BULLET_HIT_LARGE);
        e.surf_type = 6;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["bullet_large_dirt"]
        );
    }

    #[test]
    fn grenade_explode_plays_surface_alias_and_weapon_explosion() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            4,
            Some(WeaponSounds {
                proj_explosion: Some("grenade_explode".into()),
                ..Default::default()
            }),
        );
        let mz = HashMap::new();
        let mut e = ev(EV_GRENADE_EXPLODE);
        e.weapon = 4;
        e.surf_type = 6; // dirt
        let cues = resolve(&e, &ctx(&cs, &ws, &mz));
        assert_eq!(
            names(&cues),
            vec!["grenade_explode_dirt", "grenade_explode"]
        );
        for c in &cues {
            assert!(matches!(c.source, Source::Point(p) if p == EV_POS));
        }
        let mut e = ev(EV_GRENADE_BOUNCE);
        e.surf_type = 6;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["grenade_bounce_dirt"]
        );
        let mut e = ev(EV_ROCKET_EXPLODE);
        e.weapon = 4;
        e.surf_type = 6;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["rocket_explode_dirt", "grenade_explode"]
        );
    }

    #[test]
    fn melee_hit_emits_from_the_victim() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let mut e = ev(EV_MELEE_HIT);
        e.other_entity_num = 9;
        let cues = resolve(&e, &ctx(&cs, &ws, &mz));
        assert_eq!(names(&cues), vec!["melee_hit"]);
        assert!(matches!(cues[0].source, Source::Entity { num: 9, pos } if pos == EV_POS));
    }

    #[test]
    fn melee_swipe_picks_its_size_from_rifle_bullet() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            3,
            Some(WeaponSounds {
                rifle_bullet: true,
                ..Default::default()
            }),
        );
        ws.insert(4, Some(WeaponSounds::default()));
        let mz = HashMap::new();
        let mut e = ev(EV_MELEE_SWIPE);
        e.weapon = 3;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["melee_swing_large"]
        );
        e.weapon = 4;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["melee_swing_small"]
        );
        e.weapon = 9; // no cache entry
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["melee_swing_small"]
        );
    }

    #[test]
    fn no_ammo_respects_clip_only() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            3,
            Some(WeaponSounds {
                clip_only: true,
                ..Default::default()
            }),
        );
        ws.insert(4, Some(WeaponSounds::default()));
        let mz = HashMap::new();
        let mut e = ev(EV_NOAMMO);
        e.weapon = 3;
        assert!(resolve(&e, &ctx(&cs, &ws, &mz)).is_empty());
        e.weapon = 4;
        assert_eq!(
            names(&resolve(&e, &ctx(&cs, &ws, &mz))),
            vec!["player_out_of_ammo"]
        );
    }

    #[test]
    fn body_events_play_on_the_entity() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        for (id, alias) in [
            (EV_FOLIAGE_SOUND, "movement_foliage"),
            (EV_WATER_TOUCH, "player_water_in"),
            (EV_WATER_LEAVE, "player_water_out"),
            (EV_FLAMEBARREL_BOUNCE, "flamebarrel_bounce"),
        ] {
            let cues = resolve(&ev(id), &ctx(&cs, &ws, &mz));
            assert_eq!(names(&cues), vec![alias]);
            assert!(matches!(cues[0].source, Source::Entity { num: 5, .. }));
        }
    }

    /// A 1000-unit shot up +Y, muzzle at y = -500.
    fn whizby_case(listener: Vec3, surf_type: i32) -> Vec<Cue> {
        let cs = vec![];
        let ws = HashMap::new();
        let mut mz = HashMap::new();
        mz.insert(7u32, (Vec3::new(0.0, -500.0, 0.0), Vec3::Y));
        let mut e = ev(EV_BULLET_HIT_SMALL);
        e.other_entity_num = 7;
        e.pos = [0.0, 500.0, 0.0];
        e.surf_type = surf_type;
        let mut c = ctx(&cs, &ws, &mz);
        c.listener_pos = listener;
        resolve(&e, &c)
    }

    #[test]
    fn whizby_fires_when_the_shot_passes_the_listener() {
        let cues = whizby_case(Vec3::new(70.0, 0.0, 0.0), 0);
        assert!(cues.iter().any(|q| q.alias == "whizby"
            && matches!(q.source, Source::Point(p) if p == Vec3::new(0.0, -WHIZBY_SETBACK, 0.0))));
    }

    #[test]
    fn whizby_is_silent_outside_the_radius() {
        let cues = whizby_case(Vec3::new(200.0, 0.0, 0.0), 0);
        assert!(!cues.iter().any(|q| q.alias == "whizby"));
    }

    #[test]
    fn whizby_needs_the_margin_at_both_ends() {
        // Listener 20 units past the muzzle: t < WHIZBY_MARGIN.
        let cues = whizby_case(Vec3::new(0.0, -480.0, 0.0), 0);
        assert!(!cues.iter().any(|q| q.alias == "whizby"));
        let cues = whizby_case(Vec3::new(0.0, 480.0, 0.0), 0);
        assert!(!cues.iter().any(|q| q.alias == "whizby"));
    }

    #[test]
    fn whizby_also_fires_for_flesh_hits() {
        let cues = whizby_case(Vec3::new(70.0, 0.0, 0.0), 7);
        assert_eq!(cues[0].alias, "bullet_small_flesh");
        assert!(cues.iter().any(|q| q.alias == "whizby"));
    }

    /// A body with no muzzle tag whizzes from its stance point, as
    /// `CG_Tracer` hands `CG_BulletWhizby` `CG_CalcMuzzlePoint`'s result.
    #[test]
    fn whizby_starts_at_a_tagless_bodys_stance_point() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let fallbacks = HashMap::from([(7u32, Vec3::new(0.0, -500.0, 0.0))]);
        let mut e = ev(EV_BULLET_HIT_SMALL);
        e.other_entity_num = 7;
        e.pos = [0.0, 500.0, 0.0];
        let c = CueCtx {
            shot_fallbacks: &fallbacks,
            listener_pos: Vec3::new(70.0, 0.0, 0.0),
            ..ctx(&cs, &ws, &mz)
        };
        assert!(resolve(&e, &c).iter().any(|q| q.alias == "whizby"));
    }

    #[test]
    fn client_hits_play_the_flesh_alias_and_the_whizby() {
        for (id, alias) in [
            (EV_BULLET_HIT_CLIENT_SMALL, "bullet_small_flesh"),
            (EV_BULLET_HIT_CLIENT_LARGE, "bullet_large_flesh"),
        ] {
            let cs = vec![];
            let ws = HashMap::new();
            let mut mz = HashMap::new();
            mz.insert(7u32, (Vec3::new(0.0, -500.0, 0.0), Vec3::Y));
            let mut e = ev(id);
            e.other_entity_num = 7;
            e.pos = [0.0, 500.0, 0.0];
            e.surf_type = 7;
            let mut c = ctx(&cs, &ws, &mz);
            c.listener_pos = Vec3::new(70.0, 0.0, 0.0);
            assert_eq!(names(&resolve(&e, &c)), vec![alias, "whizby"]);
        }
    }

    #[test]
    fn whizby_needs_a_muzzle() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        let mut e = ev(EV_BULLET_HIT_SMALL);
        e.other_entity_num = 7;
        assert_eq!(names(&resolve(&e, &ctx(&cs, &ws, &mz))).len(), 1);
    }

    #[test]
    fn unrelated_events_produce_no_cues() {
        let (cs, ws, mz) = (vec![], HashMap::new(), HashMap::new());
        for id in [
            EV_NONE,
            EV_OBITUARY,
            EV_STEP_VIEW,
            EV_PAIN,
            EV_CROUCH_PAIN,
            EV_DEATH,
            EV_EMPTYCLIP,
            EV_EJECT_BRASS,
            EV_FIRE_MELEE,
            EV_MELEE_MISS,
            EV_ITEM_PICKUP_QUIET,
            EV_BULLET_TRACER,
            EV_ITEM_RESPAWN,
            EV_PLAYER_TELEPORT_IN,
        ] {
            assert!(
                resolve(&ev(id), &ctx(&cs, &ws, &mz)).is_empty(),
                "event {id} should be silent"
            );
        }
    }

    #[test]
    fn uo_events_play_their_aliases() {
        let cs = vec![];
        let mut ws = HashMap::new();
        ws.insert(
            3,
            Some(WeaponSounds {
                deploy: Some("weap_mg34_deploy".into()),
                breakdown: Some("weap_mg34_undeploy".into()),
                fire: Some("weap_mg34_fire".into()),
                ammo_type: "lmg".into(),
                ..Default::default()
            }),
        );
        ws.insert(
            4,
            Some(WeaponSounds {
                ammo_type: "pistol".into(),
                cloth: true,
                proj_explosion: Some("shell_boom".into()),
                ..Default::default()
            }),
        );
        let mz = HashMap::new();
        let play = |event: i32, f: &dyn Fn(&mut GameEvent)| {
            let mut e = ev(event);
            f(&mut e);
            names(&resolve(&e, &ctx(&cs, &ws, &mz)))
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(play(EV_UO_FATIGUE_LAST_SOUND, &|_| {}), ["fatigue_breath"]);
        assert_eq!(
            play(EV_UO_DEPLOY_WEAPON, &|e| e.weapon = 3),
            ["weap_mg34_deploy"]
        );
        assert_eq!(
            play(EV_UO_BREAKDOWN_WEAPON, &|e| e.weapon = 3),
            ["weap_mg34_undeploy"]
        );
        // The gunner's fire names its weapon in the parm.
        assert_eq!(
            play(EV_UO_FIRE_WEAPONC, &|e| e.parm = 3),
            ["weap_mg34_fire"]
        );
        let hit = |w: i32| {
            play(EV_UO_BULLET_HIT, &move |e| {
                e.weapon = w;
                e.surf_type = 6;
            })
        };
        assert_eq!(hit(3), ["bullet_large_dirt"]);
        assert_eq!(hit(4), ["bullet_small_dirt"]);
        assert_eq!(
            play(EV_GRENADE_BOUNCE, &|e| e.weapon = 4),
            ["satchel_bounce"]
        );
        assert_eq!(play(EV_UO_GRENADE_SPOON, &|_| {}), ["us_grenade_lever"]);
        assert_eq!(
            play(EV_UO_PROJECTILE_LAUNCH, &|e| e.parm = 4),
            ["shell_flash"]
        );
        assert_eq!(
            play(EV_UO_PROJECTILE_INCOMING, &|e| e.parm = 5),
            ["barrage_incoming"]
        );
        assert!(play(EV_UO_PROJECTILE_INCOMING, &|e| e.parm = 0).is_empty());
        let boom = |kind: i32| {
            play(EV_UO_PROJECTILE_EXPLODE, &move |e| {
                e.scale = kind;
                e.surf_type = 6;
                e.weapon = 4;
            })
        };
        assert_eq!(boom(4), ["artillery_explode_dirt", "shell_boom"]);
        assert_eq!(boom(1), ["shell_boom"]);
        assert_eq!(
            play(EV_UO_OVERHEATING, &|e| e.e_type = 12),
            ["mg_overheat_vehicle"]
        );
        assert_eq!(play(EV_UO_OVERHEATING, &|e| e.e_type = 1), ["mg_overheat"]);
    }
}

/// What the client plays for the retail UO captures' events
/// (docs/research/coduo-diff.md 19, "Captures").
#[cfg(test)]
mod uo_capture_gates {
    use super::*;
    use crate::fx::registry::uo_captures::{events, weapons};

    /// Every cue alias of a capture's events, with how often it played.
    fn aliases(tag: &str, keep: impl Fn(&GameEvent) -> bool) -> HashMap<String, usize> {
        let sounds: HashMap<i32, Option<WeaponSounds>> = weapons(tag)
            .into_iter()
            .enumerate()
            .map(|(i, d)| (i as i32, d.map(|d| d.sounds)))
            .collect();
        let none = HashMap::new();
        let ctx = CueCtx {
            configstrings: &[],
            sounds: 0,
            weapon_sounds: &sounds,
            muzzles: &none,
            shot_fallbacks: &HashMap::new(),
            listener_pos: Vec3::ZERO,
            ps_entity: 1,
            uo: true,
        };
        let mut out = HashMap::new();
        for ev in events(tag).iter().filter(|e| keep(e)) {
            for c in resolve(ev, &ctx) {
                *out.entry(c.alias).or_insert(0) += 1;
            }
        }
        out
    }

    fn uo_only(e: &GameEvent) -> bool {
        e.event & vcod_common::net::event_ids::EV_FOREIGN != 0
    }

    /// The barrage: a `shell_flash` per launch, a `barrage_incoming` per
    /// warning, an `artillery_explode_<surface>` per shell, and the
    /// binoculars' sight raising and dropping (160, 161) silent, as their
    /// weapon file names no `deploySound`.
    #[test]
    fn the_uo_artillery_capture_plays_the_barrage() {
        let a = aliases("uo-arty", uo_only);
        assert!(a["shell_flash"] >= 12, "{a:?}");
        assert!(a["barrage_incoming"] >= 12, "{a:?}");
        let explodes: usize = a
            .iter()
            .filter(|(k, _)| k.starts_with("artillery_explode_"))
            .map(|(_, n)| n)
            .sum();
        assert!(explodes >= 12, "{a:?}");
        assert!(
            a.keys().all(|k| k == "shell_flash"
                || k == "barrage_incoming"
                || k.starts_with("artillery_explode_")),
            "{a:?}"
        );
    }

    /// A shell on a hull sounds off the struck surface, metal, not a
    /// vehicle column: the alias table is 23 surfaces a row (uo cgame
    /// 0x3044bc30), and the tank's shell names no `projExplosionSound`.
    #[test]
    fn the_uo_shellveh_capture_explodes_on_the_metal_alias() {
        let a = aliases("uo-shellveh", |e| {
            e.event == EV_UO_PROJECTILE_EXPLODE || e.event == EV_UO_PROJECTILE_EXPLODE_NOMARKS
        });
        assert_eq!(a, HashMap::from([("tank_explode_metal".to_string(), 3)]));
    }

    /// The smoke grenade pops on its own `projExplosionSound` alone (type
    /// 1 has no alias row) and bounces on `grenade_bounce_<surface>`.
    #[test]
    fn the_uo_smoke_capture_plays_the_pop_and_the_bounces() {
        let a = aliases("uo-smoke", |e| {
            e.event == EV_UO_PROJECTILE_EXPLODE || e.event == EV_GRENADE_BOUNCE
        });
        assert_eq!(
            a,
            HashMap::from([
                ("smokegrenade_explode".to_string(), 1),
                ("grenade_bounce_default".to_string(), 2),
            ])
        );
    }

    /// The bipod: the mg34's deploy and breakdown aliases, once each past
    /// the follow's replay of the ring.
    #[test]
    fn the_uo_mg_capture_deploys_and_breaks_down() {
        let a = aliases("uo-mg", uo_only);
        assert!(a["weap_mg34_deploy"] >= 1, "{a:?}");
        assert_eq!(a["weap_mg34_undeploy"], 1, "{a:?}");
    }

    /// The tank: its main gun fires `panzerIV_fire`, its MGs (whose
    /// `fireSound` is blank) nothing, the overheat
    /// `mg_overheat_vehicle`; the jeep's .50 `weap_50cal_box_fire`.
    #[test]
    fn the_uo_vehicle_captures_play_the_guns_and_the_overheat() {
        let vehicle = |e: &GameEvent| e.e_type == vcod_common::net::flags::ET_VEHICLE_V22;
        let tank = aliases("uo-overheat", vehicle);
        assert_eq!(tank["mg_overheat_vehicle"], 1, "{tank:?}");
        assert_eq!(tank["panzerIV_fire"], 2, "{tank:?}");
        let jeep = aliases("uo-jeep", |e| vehicle(e) && fires_weapon(e.event));
        assert!(jeep["weap_50cal_box_fire"] >= 1, "{jeep:?}");
        // A mounted MG's overheat plays the vehicle alias too (`eType & 0xc`).
        let turret = aliases("uo-turret", |e| {
            e.e_type == vcod_common::net::flags::ET_TURRET && e.event == EV_UO_OVERHEATING
        });
        assert_eq!(turret["mg_overheat_vehicle"], 1, "{turret:?}");
    }
}
