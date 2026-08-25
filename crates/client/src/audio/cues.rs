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
pub const CS_SOUND_ALIASES: usize = 524;

/// Entity number for the map ambient, so no cue can end it (retail uses
/// reserved stream slots instead, section 2). Outside 0..1024 and distinct
/// from the `u32::MAX` sentinel [`ps_entity`] returns.
pub const AMBIENT_ENTITY: u32 = 0xFFFF_FFFE;

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
    /// Keyed by 1-based CS7 weapon index; `None` = weapon file failed to load.
    pub weapon_sounds: &'a HashMap<i32, Option<WeaponSounds>>,
    /// `(position, forward)` per entity this frame; the whizby start point.
    pub muzzles: &'a HashMap<u32, (Vec3, Vec3)>,
    pub listener_pos: Vec3,
    /// Playerstate-ring events carry no entity and are attributed to this one.
    pub ps_entity: u32,
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
            let alias = match ev.parm {
                1..=64 => Some(
                    weapon(ctx, ev.parm)
                        .and_then(|w| w.pickup.clone())
                        .unwrap_or_else(|| "weap_pickup".to_string()),
                ),
                65 | 66 => Some("grenade_pickup".to_string()),
                67 => Some("health_pickup_small".to_string()),
                68 => Some("health_pickup_medium".to_string()),
                69 => Some("health_pickup_large".to_string()),
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
            let alias = weapon(ctx, ev.weapon).and_then(|w| {
                if e == EV_FIRE_WEAPON_LASTSHOT {
                    w.last_shot.clone().or_else(|| w.fire.clone())
                } else {
                    w.fire.clone()
                }
            });
            if let Some(alias) = alias {
                let src = entity_source(ev, ctx);
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
                    .get(CS_SOUND_ALIASES + index as usize)
                    .map(String::as_str)
                    .unwrap_or("");
                if !name.is_empty() {
                    out.push(cue(name, entity_source(ev, ctx)));
                }
            }
        }

        // impacts, sections 6, 7a and 7c. The whizby test runs for every
        // surface, flesh included, from the shooter's (`otherEntityNum`)
        // muzzle; no muzzle yet means no whizby.
        EV_BULLET_HIT_SMALL | EV_BULLET_HIT_LARGE => {
            let prefix = if e == EV_BULLET_HIT_SMALL {
                "bullet_small"
            } else {
                "bullet_large"
            };
            let impact = Vec3::from(ev.pos);
            out.push(cue(
                with_surface(prefix, ev.surf_type),
                Source::Point(impact),
            ));
            if let Some(&(start, _)) = ctx.muzzles.get(&ev.other_entity_num) {
                if let Some(at) = whizby_point(start, impact, ctx.listener_pos) {
                    out.push(cue("whizby", Source::Point(at)));
                }
            }
        }
        EV_GRENADE_BOUNCE => out.push(cue(
            with_surface("grenade_bounce", ev.surf_type),
            Source::Point(Vec3::from(ev.pos)),
        )),
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
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
        }
    }

    fn ctx<'a>(
        cs: &'a [String],
        ws: &'a HashMap<i32, Option<WeaponSounds>>,
        mz: &'a HashMap<u32, (Vec3, Vec3)>,
    ) -> CueCtx<'a> {
        CueCtx {
            configstrings: cs,
            weapon_sounds: ws,
            muzzles: mz,
            listener_pos: Vec3::ZERO,
            ps_entity: 2,
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
        cs[CS_SOUND_ALIASES + 7] = "mp_bomb_plant".to_string();
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
}
