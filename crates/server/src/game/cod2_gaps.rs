//! The combat follow-ups on mp_toujane through `Cod2Level`: the script's
//! `radiusDamage` on CoD2 bodies. Needs `COD2_DIR`; without it every test
//! passes.

use glam::Vec3;
use vcod_gsc::Value;

use super::cod2_fight::{assert_no_script_errors, fight};

/// A gsc thread on the level that calls `radiusDamage` at `level.probe_at`.
const PROBE: &str = "probe_blast() { radiusDamage(level.probe_at, 300, 60, 20); }\n";

#[test]
fn radius_damage_reaches_a_cod2_body() {
    let Some(mut f) = fight() else {
        return;
    };
    f.face_off();
    let victim = f.level.players[1].as_ref().unwrap().ps.origin;
    let at = victim + Vec3::Z * 10.0;
    f.level.rt.install_for_test(PROBE);
    f.level
        .rt
        .set_level_field_for_test("probe_at", Value::Vector(at.to_array()));
    let ent = f.level.rt.client_entity(1).unwrap();
    f.level.rt.start_thread_for_test(ent, "probe_blast");
    f.frame();
    // 20 + (1 - 10 / 300) * 40 truncated, every probe clear, `none`'s 1.
    assert_eq!(f.health(1), 100 - 58);
    assert_eq!(f.health(0), 100, "the shooter stands outside the radius");
    assert_no_script_errors(&mut f);
}

const ADS: i32 = vcod_cod2::pmove::button::ADS;

/// The shooter down the sight, then one round.
fn fire_once(f: &mut super::cod2_fight::Fight) {
    f.buttons[0] = ADS;
    f.frames(20);
    f.buttons[0] = ADS | 1;
    f.frame();
    f.buttons[0] = 0;
}

/// A `trigger_damage` the round's leg crosses takes the `damage` notify
/// off `G_CheckHitTriggerDamage` with the weapon's own `damage` and the
/// shooter. The trigger is a box about the victim (no stock map has one).
const TRIGGER: &str = "probe_trigger() {\n\
    self waittill(\"damage\", amount, who);\n\
    level.probe_amount = amount;\n\
    level.probe_who = who getEntityNumber();\n\
}\n";

#[test]
fn a_round_fires_a_damage_trigger() {
    let Some(mut f) = fight() else {
        return;
    };
    f.face_off();
    let victim = f.level.players[1].as_ref().unwrap().ps.origin;
    let id = f.level.rt.spawn_map_entity_for_test(victim.to_array());
    f.level.rt.with_host(|h, cx| {
        let spec = crate::game::damage_trigger::spec_of(&[], false);
        let shape = crate::game::trigger::TriggerShape {
            mins: [-40.0, -40.0, 0.0],
            maxs: [40.0, 40.0, 80.0],
            model: None,
        };
        h.triggers.register_damage(id, shape, spec);
        crate::game::trigger::link(h, cx, id, 0);
    });
    f.level.rt.install_for_test(TRIGGER);
    f.level.rt.start_thread_for_test(id, "probe_trigger");
    fire_once(&mut f);
    f.frames(2);
    let weapon = f.level.rt.host.client_weapons[0].current;
    let damage = f.level.combat.weapons[usize::from(weapon)]
        .as_ref()
        .unwrap()
        .damage;
    assert_eq!(f.level.rt.level_field("probe_amount"), Value::Int(damage));
    assert_eq!(f.level.rt.level_field("probe_who"), Value::Int(0));
    assert_no_script_errors(&mut f);
}

/// A `script_model` with collision between the shooter and the victim
/// stops the round, which leaves its impact on it. Nothing in CoD2 MP gives
/// a script model `takedamage` (only `ClientEndFrame`, `player_die` and
/// `SP_trigger_damage` store a 1 there), so its `health` stands.
#[test]
fn a_script_model_stops_a_round() {
    let Some(mut f) = fight() else {
        return;
    };
    f.face_off();
    let eye = super::cod2::Cod2Level::view_origin(&f.level.players[0].as_ref().unwrap().ps);
    let chest = f.level.players[1].as_ref().unwrap().ps.origin + Vec3::Z * 44.0;
    let mid = (eye + chest) * 0.5 - Vec3::Z * 16.0;
    let id = f.level.rt.spawn_map_entity_for_test(mid.to_array());
    f.level.rt.with_host(|h, cx| {
        use vcod_gsc::Host;
        for (name, v) in [
            ("classname", Value::String(cx.intern_exact("script_model"))),
            ("model", Value::String(cx.intern_exact("xmodel/crate01"))),
            ("health", Value::Int(1000)),
        ] {
            let atom = cx.intern_folded(name);
            h.set_field(cx, id, atom, v).unwrap();
        }
    });
    fire_once(&mut f);
    let health = f.level.rt.with_host(|h, cx| {
        use vcod_gsc::Host;
        let health = cx.intern_folded("health");
        h.get_field(cx, id, health)
    });
    assert_eq!(health, Value::Int(1000));
    assert!(
        f.level.combat.hits.is_empty(),
        "the crate stopped the round"
    );
    assert_eq!(f.health(1), 100);
    use vcod_common::net::event_ids as v1;
    let impact = f.level.rt.temp_entities().any(|(_, te)| {
        te.event == v1::EV_BULLET_HIT_LARGE && Vec3::from(te.origin).distance(mid) < 40.0
    });
    assert!(impact, "the impact went out on the crate");
    assert_no_script_errors(&mut f);
}

/// A body killed in the air: its clone falls under gravity from where the
/// player was, comes to rest on the floor (`TR_INTERPOLATE`, the world
/// its ground) and the fresh-corpse bit goes at the death
/// anim's end.
#[test]
fn a_corpse_falls_and_rests() {
    let Some(mut f) = fight() else {
        return;
    };
    f.face_off();
    let p2 = vcod_cod2::net::PROTOCOL_118.as_cod1_shim();
    let floor = f.level.players[1].as_ref().unwrap().ps.origin;
    let p = f.level.players[1].as_mut().unwrap();
    p.ps.origin = floor + Vec3::Z * 120.0;
    p.ps.velocity = Vec3::ZERO;
    f.frame();
    f.level.kill(1);
    f.frame();
    let body = |f: &mut super::cod2_fight::Fight| {
        f.level
            .rt
            .host
            .bodies
            .entities()
            .next()
            .map(|(_, e)| e)
            .expect("a corpse")
    };
    let z = |e: &vcod_common::net::msg::EntityState| {
        f32::from_bits(e.field_i32(p2, "pos.trBase[2]") as u32)
    };
    let first = body(&mut f);
    let tr_type = |e: &vcod_common::net::msg::EntityState| e.field_i32(p2, "pos.trType");
    assert_eq!(tr_type(&first), vcod_cod2::trajectory::TR_GRAVITY);
    assert!(z(&first) > floor.z + 60.0, "launched from the air");
    f.frames(20);
    let rest = body(&mut f);
    assert_eq!(tr_type(&rest), vcod_cod2::trajectory::TR_INTERPOLATE);
    // On the ground, after its death anim's root slid it along the slope.
    assert!(
        (z(&rest) - floor.z).abs() < 4.0,
        "{} on {}",
        z(&rest),
        floor.z
    );
    assert_eq!(rest.field_i32(p2, "groundEntityNum"), 1022);
    // `kill` draws the death anim, so `cloneplayer` gets its duration and
    // the body queue's think takes the fresh bit out at its end.
    let anim = f.level.rt.host.cod2().clients[1].death_anim;
    assert_ne!(anim, 0, "kill drew a death anim");
    assert_ne!(first.field_i32(p2, "eFlags") & 0x8_0000, 0);
    f.frames(80);
    assert_eq!(body(&mut f).field_i32(p2, "eFlags") & 0x8_0000, 0);
    assert_no_script_errors(&mut f);
}

/// The stock `_deathicons.gsc` puts its waypoints (`type` 13) at the
/// clone's `origin` plus 54 up: `cloneplayer`'s body reads the player's
/// origin as retail's `G_SetOrigin` left it, not `(0, 0, 0)`
/// (cod2-hud.md 19.1).
#[test]
fn a_death_icon_sits_over_the_corpse() {
    let Some(mut f) = fight() else {
        return;
    };
    f.face_off();
    let at = f.level.players[1].as_ref().unwrap().ps.origin;
    f.level.kill(1);
    f.frame();
    let icons: Vec<[f32; 3]> = f.level.rt.with_host(|h, cx| {
        let ids: Vec<_> = h
            .ents
            .iter_hud_elems()
            .filter(|(_, e)| e.hud.is_some_and(|s| s.elem_type == 13))
            .map(|(id, _)| id)
            .collect();
        ids.into_iter()
            .map(|id| {
                ["x", "y", "z"].map(|n| {
                    let a = cx.intern_folded(n);
                    match vcod_gsc::Host::get_field(h, cx, id, a) {
                        Value::Float(v) => v,
                        Value::Int(v) => v as f32,
                        v => panic!("{n} reads {v:?}"),
                    }
                })
            })
            .collect()
    });
    // One for the victim's team and one for the spectators.
    assert_eq!(icons.len(), 2, "{icons:?}");
    for icon in icons {
        assert_eq!([icon[0], icon[1]], [at.x, at.y], "{icon:?} over {at}");
        assert!((icon[2] - (at.z + 54.0)).abs() < 1.0, "{icon:?} over {at}");
    }
    assert_no_script_errors(&mut f);
}

/// The live weapon items: entity, row, `count`, clip, origin.
fn items(f: &mut super::cod2_fight::Fight) -> Vec<(vcod_gsc::EntId, u8, Vec3)> {
    f.level.rt.with_host(|h, cx| {
        let ids: Vec<_> = h
            .ents
            .iter_inuse()
            .filter_map(|(id, e)| e.item.filter(|i| !i.taken).map(|i| (id, i.index)))
            .collect();
        ids.into_iter()
            .map(|(id, index)| {
                let at = crate::game::item::origin_of(h, cx, id);
                (id, index, Vec3::from(at))
            })
            .collect()
    })
}

/// The stock `dm` killed callback's `dropWeapon` leaves the victim's
/// weapon on the ground, which falls and rests; the killer standing on it
/// with both primary slots full takes nothing by touch, and the use key,
/// looking down at it, swaps his weapon for it, his own left where it lay.
#[test]
fn a_dead_players_weapon_drops_and_is_swapped_up() {
    let Some(mut f) = fight() else {
        return;
    };
    // The victim holds an mp40 in his primary slot, which the shooter has
    // not got.
    let (index, clip_index, ammo_index, clip_size) = f.level.rt.with_host(|h, _| {
        let i = h.weapon_index("mp40_mp").expect("mp40_mp");
        let d = h.weapons.get(i).unwrap();
        (i, d.clip_index, d.ammo_index, d.clip_size)
    });
    let h = &mut f.level.rt.host;
    let held = h.client_weapons[1].current;
    h.client_weapons[1].take(usize::from(held));
    h.client_weapons[1].give(index, 1);
    h.client_weapons[1].current = index as u8;
    h.client_ammo[1].clip[clip_index] = clip_size as i16;
    h.client_ammo[1].ammo[ammo_index] = 64;
    f.face_off();
    let victim_weapon = f.level.rt.host.client_weapons[1].current;
    let own = f.level.rt.host.client_weapons[0].current;
    assert_ne!(victim_weapon, own);
    fire_once(&mut f);
    f.frames(2);
    assert_eq!(f.field(1, "sessionstate"), "dead");
    // The weapon in hand, and the offhand grenades (`dropOffhand`).
    let weapon_item = |f: &mut super::cod2_fight::Fight| {
        items(f)
            .into_iter()
            .find(|i| i.1 == victim_weapon)
            .expect("the dropped weapon")
    };
    let (id, _, _) = weapon_item(&mut f);
    f.frames(40);
    let at = weapon_item(&mut f).2;
    let floor = f.level.combat.floor_under(at + Vec3::Z * 8.0).unwrap();
    assert!((at.z - floor.z).abs() < 4.0, "rests at {at} over {floor}");
    // On top of it: no room, so no touch pickup.
    let p = f.level.players[0].as_mut().unwrap();
    p.ps.origin = at + Vec3::new(20.0, 0.0, 0.25);
    p.ps.velocity = Vec3::ZERO;
    f.frames(4);
    assert!(
        items(&mut f).iter().any(|i| i.0 == id),
        "a full player takes nothing by touch"
    );
    assert_eq!(f.level.rt.host.client_weapons[0].current, own);
    let eye = super::cod2::Cod2Level::view_origin(&f.level.players[0].as_ref().unwrap().ps);
    let ang = crate::game::builtins::math::vectoangles((at - eye).to_array());
    f.aim[0] = [ang[0], ang[1]];
    f.frames(2);
    f.buttons[0] = 0x8;
    f.frame();
    f.buttons[0] = 0;
    f.frames(2);
    let w = f.level.rt.host.client_weapons[0];
    assert!(
        w.holds(usize::from(victim_weapon)),
        "the swap gave the weapon"
    );
    assert!(!w.holds(usize::from(own)), "the swap took his own");
    let left = items(&mut f);
    assert!(left.iter().all(|i| i.0 != id), "{left:?}");
    assert!(
        left.iter().any(|i| i.1 == own && i.2.distance(at) < 1.0),
        "{left:?}"
    );
    assert_no_script_errors(&mut f);
}

/// The stock spawn's `giveBinoculars` and the `+binoculars` press on
/// vcod-server: the rifle goes down, the binoculars come up and hold, and
/// `binocular_enter` reaches the script (a `waittill` on the player).
#[test]
fn the_binoculars_come_up_on_the_server() {
    let Some(mut f) = fight() else {
        return;
    };
    const WATCH: &str = "probe_binoc() {\n\
        self waittill(\"binocular_enter\");\n\
        level.probe_binoc = 1;\n\
    }\n";
    f.level.rt.install_for_test(WATCH);
    let ent = f.level.rt.client_entity(0).unwrap();
    f.level.rt.start_thread_for_test(ent, "probe_binoc");
    use vcod_cod2::pmove::weapon::{key, state, wpf};
    f.buttons[0] = key::BINOCULARS;
    f.frame();
    f.buttons[0] = 0;
    f.frames(30);
    let p = f.level.players[0].as_ref().unwrap();
    assert_eq!(p.ws.state, state::BINOCULARS_HOLD);
    assert_ne!(p.ps.pm_flags & wpf::BINOCULARS, 0);
    assert_eq!(f.level.rt.level_field("probe_binoc"), Value::Int(1));
    assert_no_script_errors(&mut f);
}

/// The damage callback's view of a shot's rewind: `level.probe_origin` and
/// `level.probe_attacker` are `self.origin` and the attacker's inside it.
const REWIND: &str = "probe_hook() {\n\
    setarchive(true);\n\
    level.callbackPlayerDamage = ::probe_damage;\n\
}\n\
probe_damage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, timeOffset) {\n\
    level.probe_origin = self.origin;\n\
    level.probe_offset = timeOffset;\n\
}\n";

/// `G_AntiLagRewindClientPos` moves the victim's `r.currentOrigin` for the
/// whole shot: the damage callback reads the rewound `self.origin`, and
/// `G_AntiLag_RestoreClientPos` puts it back after (cod2-combat.md 2.3.2).
#[test]
fn a_shot_moves_its_victim_back_until_the_rounds_are_done() {
    let Some(mut f) = fight() else {
        return;
    };
    f.level.rt.install_for_test(REWIND);
    let ent = f.level.rt.client_entity(0).unwrap();
    f.level.rt.start_thread_for_test(ent, "probe_hook");
    f.face_off();
    // The victim slides sideways a unit a frame, the archive keeping each.
    for _ in 0..12 {
        f.level.players[1].as_mut().unwrap().ps.origin.y += 1.0;
        f.frame();
        let world = f.level.world_entities(f.now);
        let roster: Vec<_> = ["shooter", "victim"]
            .iter()
            .enumerate()
            .map(|(s, n)| f.level.client_state(s, n))
            .collect();
        f.level.archive_frame(&world, &roster);
    }
    assert!(f.level.rt.archive_on());
    let lag = f.now - 200;
    let live = f.level.players[1].as_ref().unwrap().ps.origin;
    let back = f
        .level
        .rewound_origin(1, lag)
        .and_then(|r| r.at())
        .expect("archived");
    assert!(back.distance(live) > 2.0, "{back} {live}");
    f.level.fire_weapon(0, lag);
    let seen = match f.level.rt.level_field("probe_origin") {
        Value::Vector(v) => Vec3::from(v),
        v => panic!("the callback never ran: {v:?}"),
    };
    assert_eq!(seen, back);
    assert_eq!(f.level.rt.level_field("probe_offset"), Value::Int(200));
    assert_eq!(Vec3::from(f.level.rt.client_origin(1)), live);
    let body = f.level.rt.host.cod2().clients[1].body.unwrap();
    assert_eq!((body.origin, body.ps_origin), (live, live));
    assert_no_script_errors(&mut f);
}

/// A shot whose lag reaches past the archive's first frame: retail's
/// lookup divides 0 by 0 and links the victim at NaN, which no round
/// meets; vcod leaves him where he stands, unstruck (cod2-combat.md 2.3.3).
#[test]
fn a_shot_past_the_archive_start_strikes_no_rewound_body() {
    let Some(mut f) = fight() else {
        return;
    };
    f.level.rt.install_for_test(REWIND);
    f.face_off();
    // Down the sight, so the round flies true.
    f.buttons[0] = ADS;
    f.frames(20);
    // On only now: no frame archived yet.
    let ent = f.level.rt.client_entity(0).unwrap();
    f.level.rt.start_thread_for_test(ent, "probe_hook");
    f.frame();
    assert!(f.level.rt.archive_on());
    let live = f.level.players[1].as_ref().unwrap().ps.origin;
    let lag = f.now - 200;
    assert_eq!(
        f.level.rewound_origin(1, lag),
        Some(super::cod2_archive::Rewind::Nowhere)
    );
    f.level.fire_weapon(0, lag);
    assert_eq!(f.level.rt.level_field("probe_origin"), Value::Undefined);
    assert_eq!(f.health(1), 100);
    assert_eq!(Vec3::from(f.level.rt.client_origin(1)), live);
    // The same round with no lag strikes.
    f.level.fire_weapon(0, f.now);
    assert!(matches!(
        f.level.rt.level_field("probe_origin"),
        Value::Vector(_)
    ));
    assert_no_script_errors(&mut f);
}
