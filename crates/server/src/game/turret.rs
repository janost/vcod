//! The per-turret record `G_SpawnTurret` builds at map load
//! (`docs/research/cod11-turrets.md` sections 2 and 3): a weapon file parsed
//! once into a [`TurretDef`], combined with the entity's own spawn keys into
//! the [`TurretRecord`] the host keeps per turret entity, and the use key's
//! half of a mount (section 4). Aim, fire and release mutate the record in
//! place.

use crate::spectate::ClientSim;
use glam::Vec3;
use vcod_common::pmove::aim::{angle_normalize_180, angle_subtract};
use vcod_common::pmove::Stance;
use vcod_gsc::EntId;

/// The turret keys of a weapon file (`weapons/mp/<name>`), weapon-def
/// offsets in docs/research/cod11-turrets.md section 2.
#[derive(Clone, Debug, PartialEq)]
pub struct TurretDef {
    pub left_arc: f32,
    pub right_arc: f32,
    pub top_arc: f32,
    pub bottom_arc: f32,
    pub damage: i32,
    pub fire_time_ms: i32,
    pub stance: TurretStance,
    pub anim_hor_rotate_inc: f32,
    pub rifle_bullet: bool,
    pub loop_sound: Option<String>,
    pub stop_sound: Option<String>,
    pub use_hint_string: Option<String>,
}

/// `rec+0x20`: 0 stand, 1 duck, 2 prone, from the weapon's `stance` key. The
/// order is VERIFIED from the `.data` pointer table at 0x7c958..0x7c960
/// (section 2 of the research doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurretStance {
    Stand,
    Duck,
    Prone,
}

/// `rec+0x0c..0x1c`: a turret's live state, one per spawned `misc_mg42`/
/// `misc_turret`. Mount, aim, fire and release all mutate a
/// record already in `GameHost::turrets`.
#[derive(Clone, Debug, PartialEq)]
pub struct TurretRecord {
    pub weapon: String,
    pub def: TurretDef,
    /// [min, max] per axis after the key overrides: pitch [-top, bottom], yaw [-right, left].
    pub pitch_range: [f32; 2],
    pub yaw_range: [f32; 2],
    pub dmg: i32,
    pub rest_pitch: f32,
    /// `s.angles2`: barrel pitch, yaw, and the slew's leftover step.
    pub angles2: [f32; 3],
    pub owner: Option<usize>,
    /// Retail's busy byte: 1 manned, 2 release asked for by a use press.
    pub busy: u8,
    pub cooldown_ms: i32,
    pub loop_left_ms: i32,
    pub mount_origin: [f32; 3],
    pub mount_stance: Option<vcod_common::pmove::Stance>,
    /// `rec->flags & 0x800`: the first aim after a mount flips the teleport bit.
    pub fresh_mount: bool,
    pub teleport_bit: bool,
    pub firing: bool,
    /// The model's tags, `None` when the model or a tag failed to load: the
    /// gun then fires no bullet, retail's missing-tag arm (turrets doc 6.3).
    pub tags: Option<TurretTags>,
}

/// Bind-pose model-space positions of the tags the mounted frame reads
/// (turrets doc 6.3 and 7), loaded once per turret at spawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurretTags {
    pub aim: Vec3,
    pub player: Vec3,
    pub flash: Vec3,
    pub aim_animated: Vec3,
    pub weapon: Vec3,
}

impl TurretTags {
    /// The five tags out of a model's bones, `None` if any is missing.
    pub fn from_bones(bones: &[vcod_common::xmodel::Bone]) -> Option<TurretTags> {
        let tag = |n: &str| bones.iter().find(|b| b.name == n).map(|b| b.pos);
        Some(TurretTags {
            aim: tag("tag_aim")?,
            player: tag("tag_player")?,
            flash: tag("tag_flash")?,
            aim_animated: tag("tag_aim_animated")?,
            weapon: tag("tag_weapon")?,
        })
    }
}

impl TurretDef {
    /// Parses a `weapons/mp/*` text blob (`vcod_common::xmodel::parse_weapon`'s
    /// backslash-delimited key/value pairs). `None` when the file is not a
    /// turret's (`weaponClass` other than `turret`), which is every ordinary
    /// carried weapon.
    pub fn parse(text: &str) -> Option<TurretDef> {
        let kv = vcod_common::xmodel::parse_weapon(text);
        let f = |k: &str| kv.get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        let s = |k: &str| kv.get(k).filter(|v| !v.is_empty()).cloned();
        if kv.get("weaponClass").map(String::as_str) != Some("turret") {
            return None;
        }
        Some(TurretDef {
            left_arc: f("leftArc"),
            right_arc: f("rightArc"),
            top_arc: f("topArc"),
            bottom_arc: f("bottomArc"),
            damage: f("damage") as i32,
            // The stock file holds seconds (0.05); the record wants ms, the
            // unit `docs/research/cod11-combat.md`'s type-7 reading uses.
            fire_time_ms: (f("fireTime") * 1000.0).round() as i32,
            stance: match kv.get("stance").map(String::as_str) {
                Some("prone") => TurretStance::Prone,
                Some("duck") | Some("crouch") => TurretStance::Duck,
                _ => TurretStance::Stand,
            },
            anim_hor_rotate_inc: f("animHorRotateInc"),
            rifle_bullet: f("rifleBullet") != 0.0,
            loop_sound: s("loopFireSound"),
            stop_sound: s("stopFireSound"),
            use_hint_string: s("useHintString"),
        })
    }
}

/// The `leftarc`/`rightarc`/`toparc`/`bottomarc`/`damage` spawn keys, `None` when unset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TurretKeys {
    pub left: Option<f32>,
    pub right: Option<f32>,
    pub top: Option<f32>,
    pub bottom: Option<f32>,
    pub damage: Option<i32>,
}

impl TurretRecord {
    /// `G_SpawnTurret`'s arc arithmetic (0x52e0e..0x52edc): each key falls
    /// back to the weapon file's arc when the map did not set it, and each
    /// range is widened to include 0 rather than clamped to it, so a
    /// negative arc still leaves the rest position inside range.
    pub fn new(weapon: &str, def: TurretDef, keys: TurretKeys, rest_pitch: f32) -> TurretRecord {
        let left = keys.left.unwrap_or(def.left_arc);
        let right = keys.right.unwrap_or(def.right_arc);
        let top = keys.top.unwrap_or(def.top_arc);
        let bottom = keys.bottom.unwrap_or(def.bottom_arc);
        TurretRecord {
            weapon: weapon.to_string(),
            pitch_range: [(-top).min(0.0), bottom.max(0.0)],
            yaw_range: [(-right).min(0.0), left.max(0.0)],
            dmg: keys.damage.unwrap_or(def.damage),
            rest_pitch,
            angles2: [rest_pitch, 0.0, 0.0],
            owner: None,
            busy: 0,
            cooldown_ms: 0,
            loop_left_ms: 0,
            mount_origin: [0.0; 3],
            mount_stance: None,
            fresh_mount: false,
            teleport_bit: false,
            firing: false,
            tags: None,
            def,
        }
    }
}

/// `G_IsTurretUsable` (0x5314c) with 0x52880's arc test (turrets doc 4.2):
/// nobody on the gun, no frag in hand, on the ground, and the player inside
/// the cone about the yaw arc's centre as seen from the gun, horizontally.
/// Retail's `takedamage` gate always passes: nothing clears it on a turret.
pub fn usable(
    rec: &TurretRecord,
    turret_origin: [f32; 3],
    turret_yaw: f32,
    player_origin: [f32; 3],
    grenade_time_left: i32,
    on_ground: bool,
) -> bool {
    if rec.busy != 0 || grenade_time_left != 0 || !on_ground {
        return false;
    }
    let [ymin, ymax] = rec.yaw_range;
    let half = (ymax.abs() + ymin.abs()) * 0.5;
    let centre = angle_normalize_180(turret_yaw + ymin + half).to_radians();
    let (dx, dy) = (
        turret_origin[0] - player_origin[0],
        turret_origin[1] - player_origin[1],
    );
    let len = dx.hypot(dy);
    if len == 0.0 {
        return true;
    }
    let dot = (centre.cos() * dx + centre.sin() * dy) / len;
    // Retail's compare lets a NaN through; the clamp keeps `acos` off one.
    dot.clamp(-1.0, 1.0).acos().to_degrees() <= half
}

/// `turret_use` (0x52a9c)'s record half (turrets doc 4.4): the gun is
/// manned, the barrel goes where the player looks, clamped to the arcs, and
/// the returned view is the barrel's, which [`mount_sim`] snaps the player to.
pub fn mount(
    rec: &mut TurretRecord,
    slot: usize,
    player_origin: [f32; 3],
    stance: Stance,
    view: [f32; 3],
    turret_angles: [f32; 3],
) -> [f32; 3] {
    rec.owner = Some(slot);
    rec.busy = 1;
    rec.fresh_mount = true;
    rec.mount_origin = player_origin;
    rec.mount_stance = Some(stance);
    let ranges = [rec.pitch_range, rec.yaw_range];
    for i in 0..2 {
        rec.angles2[i] =
            angle_subtract(view[i], turret_angles[i]).clamp(ranges[i][0], ranges[i][1]);
    }
    [
        rec.angles2[0] + turret_angles[0],
        rec.angles2[1] + turret_angles[1],
        0.0,
    ]
}

/// `turret_use`'s player half: the view lock, the mounted bits and the view
/// snapped onto the barrel.
pub fn mount_sim(sim: &mut ClientSim, turret: u32, stance: TurretStance, view: [f32; 3]) {
    sim.mounted_on = Some((turret, stance));
    sim.viewlocked = 1;
    sim.viewlocked_ent = turret;
    sim.ps.mounted = Some(match stance {
        TurretStance::Stand => Stance::Stand,
        TurretStance::Duck => Stance::Crouch,
        TurretStance::Prone => Stance::Prone,
    });
    sim.set_view_angle(view);
}

/// The most either barrel axis turns in one frame (turrets doc 6.2, double
/// 0x758f8).
const TURRET_MAX_STEP: f32 = 15.0;
/// What the cooldown and the loop timer drop by per server frame, whatever
/// the frame's length (turrets doc 6.3 and 6.4).
const FRAME_MS: i32 = 50;

/// 0x5201c (turrets doc 6.2): each barrel axis follows the view relative to
/// the gun, clamped to its arc, then limited to 15 degrees from where it
/// was. Returns the view to force when a clamp or the limit bit.
pub fn aim(rec: &mut TurretRecord, view: [f32; 3], turret_angles: [f32; 3]) -> Option<[f32; 3]> {
    let ranges = [rec.pitch_range, rec.yaw_range];
    let mut bit = false;
    for i in 0..2 {
        let mut want = angle_subtract(view[i], turret_angles[i]);
        if want < ranges[i][0] {
            want = ranges[i][0];
            bit = true;
        }
        if want > ranges[i][1] {
            want = ranges[i][1];
            bit = true;
        }
        let step = angle_subtract(want, rec.angles2[i]);
        if step > TURRET_MAX_STEP {
            want = rec.angles2[i] + TURRET_MAX_STEP;
            bit = true;
        }
        if step < -TURRET_MAX_STEP {
            want = rec.angles2[i] - TURRET_MAX_STEP;
            bit = true;
        }
        rec.angles2[i] = want;
    }
    rec.angles2[2] = 0.0;
    if rec.fresh_mount {
        rec.fresh_mount = false;
        rec.teleport_bit = !rec.teleport_bit;
    }
    bit.then(|| {
        [
            rec.angles2[0] + turret_angles[0],
            rec.angles2[1] + turret_angles[1],
            0.0,
        ]
    })
}

/// 0x521d4's cooldown half (turrets doc 6.3): whether this frame fires. The
/// attack bit is the frame's last cmd's, held rather than an edge.
pub fn fire_tick(rec: &mut TurretRecord, attack_held: bool) -> bool {
    rec.firing = false;
    rec.cooldown_ms -= FRAME_MS;
    if rec.cooldown_ms > 0 {
        return false;
    }
    rec.cooldown_ms = 0;
    if !attack_held {
        return false;
    }
    rec.cooldown_ms = rec.def.fire_time_ms;
    rec.loop_left_ms = 3 * rec.def.fire_time_ms;
    rec.firing = true;
    true
}

/// `turret_think_client`'s loop-sound block (turrets doc 6.4), after
/// [`fire_tick`] on the same frame: the `loopSound` to write and whether the
/// stop alias plays now. With no stop alias the loop stays on through the
/// frame the timer runs out, as retail's `je` past the clear leaves it.
pub fn loop_tick(rec: &mut TurretRecord, loop_index: i32) -> (i32, bool) {
    if rec.loop_left_ms <= 0 {
        return (0, false);
    }
    rec.loop_left_ms -= FRAME_MS;
    if rec.loop_left_ms > 0 || rec.def.stop_sound.is_none() {
        return (loop_index, false);
    }
    (0, true)
}

/// 0x51488's muzzle (turrets doc 6.3): `tag_player` in the world, moved
/// along the player's view by `tag_flash`'s distance from it. `tag_player`
/// turns with the barrel about `tag_aim`; the reach is read off the bind
/// pose (INFERRED, turrets doc 13). `forward` is `AngleVectors` of the view.
pub fn muzzle(
    tags: &TurretTags,
    turret_origin: [f32; 3],
    turret_angles: [f32; 3],
    angles2: [f32; 3],
    forward: [f32; 3],
) -> [f32; 3] {
    use crate::game::spawn::{angles_to_axis, transform};
    let barrel = angles_to_axis([angles2[0], angles2[1], 0.0]);
    let player = tags.aim + transform(tags.player - tags.aim, &barrel);
    let player = Vec3::from(turret_origin) + transform(player, &angles_to_axis(turret_angles));
    let reach = (tags.flash - tags.player).length();
    (player + Vec3::from(forward) * reach).into()
}

/// `EV_STANCE_FORCE_STAND`/`_CROUCH`/`_PRONE` (`cod11-events-and-fx.md`).
const EV_STANCE_FORCE_STAND: i32 = 140;
const EV_STANCE_FORCE_CROUCH: i32 = 141;
const EV_STANCE_FORCE_PRONE: i32 = 142;
/// `viewlocked_entNum` after a release (turrets doc 12.7).
const ENTITYNUM_NONE: u32 = 1023;

/// The record half of `G_ClientStopUsingTurret` (0x53054, turrets doc 8):
/// the gun is free and its loop timer spent. Returns the gunner's slot and
/// the spot and stance it mounted from, `None` for a gun nobody mans. The
/// caller writes the gun's `loopSound` 0.
pub fn release(rec: &mut TurretRecord) -> Option<(usize, [f32; 3], Option<Stance>)> {
    let slot = rec.owner.take()?;
    rec.busy = 0;
    rec.loop_left_ms = 0;
    rec.firing = false;
    rec.fresh_mount = false;
    Some((slot, rec.mount_origin, rec.mount_stance.take()))
}

/// The player half: the stance event for the saved stance, `TeleportPlayer`
/// back to the mount spot facing the current view, and the lock let go.
/// Returns `TeleportPlayer`'s temp entities for the caller to queue. The
/// stance itself is the client's to restore off the event (12.7).
pub fn release_sim(
    sim: &mut ClientSim,
    slot: usize,
    origin: [f32; 3],
    stance: Option<Stance>,
) -> Vec<crate::game::temp_entity::TempEntity> {
    if let Some(stance) = stance {
        sim.add_event(
            match stance {
                Stance::Prone => EV_STANCE_FORCE_PRONE,
                Stance::Crouch => EV_STANCE_FORCE_CROUCH,
                Stance::Stand => EV_STANCE_FORCE_STAND,
            },
            0,
        );
    }
    let temps = sim.teleport_player(slot, origin, sim.view_angles());
    sim.mounted_on = None;
    sim.ps.mounted = None;
    sim.viewlocked = 0;
    sim.viewlocked_ent = ENTITYNUM_NONE;
    sim.gunfx = 0;
    sim.firing = false;
    temps
}

/// An unmanned barrel's turn per frame, 200 degrees a second (turrets doc 9).
const SLEW_STEP: f32 = 10.0;

/// `turret_think`'s 0x524cc (turrets doc 9): an unmanned barrel steps toward
/// (rest pitch, 0), each axis capped at 10 degrees on its own; the pitch is
/// then capped again against where it started, the refused part carried in
/// `angles2[2]`. The stock gun never sets the record flags that retarget the
/// second cap, so they are left out.
pub fn slew_home(rec: &mut TurretRecord) {
    let entry = rec.angles2[0];
    rec.angles2[0] += rec.angles2[2];
    let target = [rec.rest_pitch, 0.0];
    for (i, t) in target.into_iter().enumerate() {
        rec.angles2[i] += angle_subtract(t, rec.angles2[i]).clamp(-SLEW_STEP, SLEW_STEP);
    }
    let stepped = rec.angles2[0];
    rec.angles2[0] = entry + angle_subtract(stepped, entry).clamp(-SLEW_STEP, SLEW_STEP);
    rec.angles2[2] = stepped - rec.angles2[0];
}

/// One round a manned gun fired this frame, for the server's trace.
#[derive(Clone, Debug, PartialEq)]
pub struct TurretShot {
    pub slot: usize,
    pub muzzle: Vec3,
    pub dir: Vec3,
    pub damage: i32,
    pub rifle_bullet: bool,
}

/// A turret change the host made that the sim has to follow, since the host
/// holds the record and the server the sim. A mount is queued by
/// `ScriptRuntime::item_pass` and applied right after it; a release by
/// `GameHost::free_entity` on a manned gun, applied in `ClientEndFrame`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TurretOp {
    Mount {
        slot: usize,
        turret: EntId,
    },
    Release {
        slot: usize,
        origin: [f32; 3],
        stance: Option<Stance>,
    },
}

/// The gun's loop and stop aliases as `CS_SOUNDS` indices, 0 for one the
/// table does not hold.
pub fn sound_indices(rec: &TurretRecord, cs: &[String]) -> (i32, i32) {
    let alias = |name: &Option<String>| {
        name.as_deref()
            .and_then(|n| crate::configstrings::sound_alias_index(cs, n))
            .unwrap_or(0)
    };
    (alias(&rec.def.loop_sound), alias(&rec.def.stop_sound))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAND: &str = "WEAPONFILE\\weaponClass\\turret\\weaponType\\bullet\\damage\\60\\fireTime\\0.05\\rifleBullet\\1\\leftArc\\45\\rightArc\\45\\topArc\\40\\bottomArc\\40\\stance\\stand\\animHorRotateInc\\15\\loopFireSound\\weap_mg42_loop\\stopFireSound\\weap_mg42_cooldown\\useHintString\\CGAME_USEMG42";

    #[test]
    fn the_stand_file_parses_to_the_stock_turret() {
        let d = TurretDef::parse(STAND).unwrap();
        assert_eq!(
            (d.left_arc, d.right_arc, d.top_arc, d.bottom_arc),
            (45.0, 45.0, 40.0, 40.0)
        );
        assert_eq!(
            (d.damage, d.fire_time_ms, d.stance),
            (60, 50, TurretStance::Stand)
        );
        assert_eq!(d.loop_sound.as_deref(), Some("weap_mg42_loop"));
        assert_eq!(d.use_hint_string.as_deref(), Some("CGAME_USEMG42"));
        assert!(d.rifle_bullet);
    }

    #[test]
    fn spawn_keys_override_the_file_and_ranges_straddle_zero() {
        let d = TurretDef::parse(STAND).unwrap();
        let keys = TurretKeys {
            left: Some(30.0),
            top: Some(-5.0),
            damage: Some(80),
            ..Default::default()
        };
        let r = TurretRecord::new("mg42_bipod_stand_mp", d, keys, -63.0);
        assert_eq!(r.yaw_range, [-45.0, 30.0]);
        // A negative top arc still leaves 0 inside the range: [min(-top,0), max(bottom,0)].
        assert_eq!(r.pitch_range, [0.0, 40.0]);
        assert_eq!(r.dmg, 80);
        assert_eq!(r.angles2, [-63.0, 0.0, 0.0]);
    }

    fn rec() -> TurretRecord {
        TurretRecord::new(
            "mg42_bipod_stand_mp",
            TurretDef::parse(STAND).unwrap(),
            TurretKeys::default(),
            -63.0,
        )
    }

    #[test]
    fn a_player_behind_the_gun_can_use_it() {
        // Gun at the origin facing +x; player 40 units behind.
        assert!(usable(&rec(), [0.0; 3], 0.0, [-40.0, 0.0, 0.0], 0, true));
    }

    #[test]
    fn the_arc_edge_is_inclusive_and_past_it_is_refused() {
        let at = |deg: f32| {
            let r = deg.to_radians();
            [-40.0 * r.cos(), -40.0 * r.sin(), 0.0]
        };
        assert!(usable(&rec(), [0.0; 3], 0.0, at(44.9), 0, true));
        assert!(!usable(&rec(), [0.0; 3], 0.0, at(46.0), 0, true));
        assert!(
            !usable(&rec(), [0.0; 3], 0.0, [40.0, 0.0, 0.0], 0, true),
            "in front of the gun"
        );
    }

    #[test]
    fn a_busy_turret_is_not_usable() {
        let mut r = rec();
        r.busy = 1;
        assert!(!usable(&r, [0.0; 3], 0.0, [-40.0, 0.0, 0.0], 0, true));
    }

    #[test]
    fn a_held_frag_or_the_air_refuses_the_mount() {
        assert!(!usable(
            &rec(),
            [0.0; 3],
            0.0,
            [-40.0, 0.0, 0.0],
            4000,
            true
        ));
        assert!(!usable(&rec(), [0.0; 3], 0.0, [-40.0, 0.0, 0.0], 0, false));
    }

    #[test]
    fn mounting_clamps_the_view_into_the_arcs() {
        let mut r = rec();
        let view = mount(
            &mut r,
            3,
            [-40.0, 0.0, 0.0],
            Stance::Crouch,
            [0.0, 80.0, 0.0],
            [0.0, 0.0, 0.0],
        );
        assert_eq!(r.owner, Some(3));
        assert_eq!(r.busy, 1);
        assert_eq!(r.angles2[1], 45.0);
        assert_eq!(view[1], 45.0);
        assert_eq!(r.mount_stance, Some(Stance::Crouch));
        assert!(r.fresh_mount);
    }

    #[test]
    fn aim_follows_the_view_inside_the_arcs() {
        let mut r = rec();
        r.angles2 = [0.0, 0.0, 0.0];
        assert_eq!(aim(&mut r, [5.0, 10.0, 0.0], [0.0; 3]), None);
        assert_eq!((r.angles2[0], r.angles2[1]), (5.0, 10.0));
    }

    #[test]
    fn a_view_past_the_edge_clamps_to_the_near_edge() {
        let mut r = rec();
        r.angles2 = [0.0, 40.0, 0.0];
        let forced = aim(&mut r, [0.0, 170.0, 0.0], [0.0; 3]).unwrap();
        assert_eq!(r.angles2[1], 45.0);
        assert_eq!(forced[1], 45.0);
        // Past 180 the subtract wraps; the far side must still clamp to the near edge.
        r.angles2 = [0.0, -40.0, 0.0];
        aim(&mut r, [0.0, 200.0, 0.0], [0.0; 3]);
        assert_eq!(r.angles2[1], -45.0);
    }

    #[test]
    fn the_barrel_turns_at_most_fifteen_degrees_a_frame() {
        let mut r = rec();
        r.angles2 = [0.0, 0.0, 0.0];
        let forced = aim(&mut r, [0.0, 40.0, 0.0], [0.0; 3]).unwrap();
        assert_eq!(r.angles2[1], 15.0);
        assert_eq!(forced[1], 15.0);
    }

    #[test]
    fn the_forced_view_is_the_barrel_plus_the_guns_own_angles() {
        let mut r = rec();
        r.angles2 = [0.0, 0.0, 0.0];
        let forced = aim(&mut r, [0.0, 229.0 + 90.0, 0.0], [0.0, 229.0, 0.0]).unwrap();
        assert_eq!(r.angles2[1], 15.0);
        assert_eq!(forced, [0.0, 244.0, 0.0]);
    }

    #[test]
    fn the_first_aim_after_a_mount_flips_the_teleport_bit_once() {
        let mut r = rec();
        r.fresh_mount = true;
        aim(&mut r, [0.0; 3], [0.0; 3]);
        assert!(r.teleport_bit);
        aim(&mut r, [0.0; 3], [0.0; 3]);
        assert!(r.teleport_bit);
    }

    #[test]
    fn a_held_trigger_fires_every_frame_and_a_released_one_does_not() {
        let mut r = rec();
        assert!(fire_tick(&mut r, true));
        assert!(r.firing);
        assert!(
            fire_tick(&mut r, true),
            "fireTime 50 against a 50 ms decrement"
        );
        assert!(!fire_tick(&mut r, false));
        assert!(!r.firing);
        assert_eq!(
            r.loop_left_ms, 150,
            "3 x fireTime from the last shot; loop_tick spends it"
        );
    }

    #[test]
    fn the_loop_runs_three_fire_times_then_plays_the_stop_alias_once() {
        let mut r = rec();
        fire_tick(&mut r, true);
        let mut out = vec![];
        for _ in 0..5 {
            out.push(loop_tick(&mut r, 9));
        }
        assert_eq!(
            out,
            vec![(9, false), (9, false), (0, true), (0, false), (0, false)]
        );
    }

    #[test]
    fn with_no_stop_alias_the_last_loop_frame_keeps_the_loop() {
        let mut r = rec();
        r.def.stop_sound = None;
        fire_tick(&mut r, true);
        let out: Vec<_> = (0..4).map(|_| loop_tick(&mut r, 9)).collect();
        assert_eq!(out, vec![(9, false), (9, false), (9, false), (0, false)]);
    }

    #[test]
    fn the_muzzle_sits_the_flash_distance_along_the_view_from_tag_player() {
        let tags = TurretTags {
            aim: Vec3::new(-3.0, 0.06, 12.9),
            player: Vec3::new(-45.1, 0.0, 20.9),
            flash: Vec3::new(8.4, 0.15, 15.2),
            aim_animated: Vec3::new(-3.0, 0.06, 12.9),
            weapon: Vec3::new(-33.6, 0.0, 12.1),
        };
        let m = muzzle(&tags, [0.0; 3], [0.0; 3], [0.0; 3], [1.0, 0.0, 0.0]);
        let reach = (tags.flash - tags.player).length();
        assert!((m[0] - (-45.1 + reach)).abs() < 0.01, "{m:?}");
        assert!((m[2] - 20.9).abs() < 0.01);
        // The barrel's 90 swings tag_player about tag_aim and the gun's own 90
        // turns the model: 180 in all, so tag_player lands on +x.
        let m = muzzle(
            &tags,
            [100.0, 0.0, 0.0],
            [0.0, 90.0, 0.0],
            [0.0, 90.0, 0.0],
            [0.0, 0.0, 1.0],
        );
        let back = -3.0 - (-45.1);
        assert!((m[0] - (100.0 + back - 0.06)).abs() < 0.01, "{m:?}");
        assert!((m[2] - (20.9 + reach)).abs() < 0.01, "{m:?}");
    }

    #[test]
    fn release_hands_back_the_mount_spot_and_frees_the_gun() {
        let mut r = rec();
        mount(
            &mut r,
            2,
            [-40.0, 0.0, 0.0],
            Stance::Crouch,
            [0.0; 3],
            [0.0; 3],
        );
        r.loop_left_ms = 100;
        r.firing = true;
        let (slot, origin, stance) = release(&mut r).unwrap();
        assert_eq!(
            (slot, origin, stance),
            (2, [-40.0, 0.0, 0.0], Some(Stance::Crouch))
        );
        assert_eq!((r.owner, r.busy, r.loop_left_ms), (None, 0, 0));
        assert!(!r.firing && !r.fresh_mount);
        assert!(release(&mut r).is_none(), "releasing twice is a no-op");
    }

    /// The sim half (turrets doc 8 and 12.7): the stance event on the ring,
    /// back to the mount spot a unit up, 200 and 199, the lock let go.
    #[test]
    fn release_sim_puts_the_gunner_back_and_unlocks_the_view() {
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        mount_sim(&mut sim, 298, TurretStance::Stand, [0.0, 90.0, 0.0]);
        sim.firing = true;
        sim.gunfx = 1;
        let seq = sim.ring.seq;
        let temps = release_sim(&mut sim, 3, [10.0, 20.0, 30.0], Some(Stance::Crouch));
        assert_eq!(sim.ring.seq, seq + 1);
        assert_eq!(sim.ring.events[(seq & 3) as usize], 141);
        assert_eq!(sim.origin(), [10.0, 20.0, 31.0]);
        let events: Vec<i32> = temps.iter().map(|t| t.event).collect();
        assert_eq!(events, vec![200, 199]);
        assert_eq!(
            (sim.viewlocked, sim.viewlocked_ent, sim.gunfx),
            (0, 1023, 0)
        );
        assert_eq!(
            (sim.mounted_on, sim.ps.mounted, sim.firing),
            (None, None, false)
        );
        assert_eq!(sim.view_angles()[1], 90.0, "the view it had on the gun");
    }

    #[test]
    fn the_saved_stance_picks_the_force_event_and_none_sends_none() {
        for (stance, event) in [
            (Some(Stance::Stand), Some(140)),
            (Some(Stance::Crouch), Some(141)),
            (Some(Stance::Prone), Some(142)),
            (None, None),
        ] {
            let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
            sim.become_player([0.0; 3], 0.0, [0; 3]);
            release_sim(&mut sim, 0, [0.0; 3], stance);
            let got = (sim.ring.seq > 0).then(|| sim.ring.events[0]);
            assert_eq!(got, event, "{stance:?}");
        }
    }

    #[test]
    fn an_unowned_barrel_walks_home_ten_degrees_a_frame() {
        let mut r = rec(); // rest -63
        r.angles2 = [0.0, 25.0, 0.0];
        slew_home(&mut r);
        assert_eq!((r.angles2[0], r.angles2[1]), (-10.0, 15.0));
        for _ in 0..10 {
            slew_home(&mut r);
        }
        assert_eq!(r.angles2, [-63.0, 0.0, 0.0]);
    }

    /// The crouch remount's slew in the capture (turrets doc 12.8): each
    /// axis capped on its own, yaw done on a 1.6 step while pitch takes 10.
    #[test]
    fn the_captured_slew_replays() {
        let mut r = rec();
        r.rest_pitch = -72.0;
        r.angles2 = [-13.6, -41.6, 0.0];
        let want = [
            (-23.6, -31.6),
            (-33.6, -21.6),
            (-43.6, -11.6),
            (-53.6, -1.6),
            (-63.6, 0.0),
            (-72.0, 0.0),
        ];
        for (pitch, yaw) in want {
            slew_home(&mut r);
            assert!(
                (r.angles2[0] - pitch).abs() < 1e-3 && (r.angles2[1] - yaw).abs() < 1e-3,
                "{:?} against ({pitch}, {yaw})",
                r.angles2
            );
            assert_eq!(r.angles2[2], 0.0);
        }
    }

    #[test]
    fn the_real_stand_file_parses() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let text = String::from_utf8(fs.read("weapons/mp/mg42_bipod_stand_mp").unwrap()).unwrap();
        let d = TurretDef::parse(&text).unwrap();
        assert_eq!((d.left_arc, d.top_arc, d.fire_time_ms), (45.0, 40.0, 50));
    }
}
