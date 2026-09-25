//! The per-turret record `G_SpawnTurret` builds at map load
//! (`docs/research/cod11-turrets.md` sections 2 and 3): a weapon file parsed
//! once into a [`TurretDef`], combined with the entity's own spawn keys into
//! the [`TurretRecord`] the host keeps per turret entity. Later mount, aim,
//! fire and release code mutates the record in place; this module only
//! builds it.

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
/// `misc_turret`. Mount, aim, fire and release (later tasks) all mutate a
/// record already in `GameHost::turrets`; this module only builds it.
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
            def,
        }
    }
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
