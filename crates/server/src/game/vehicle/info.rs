//! The vehicle definition files `G_ParseScrVehicleInfo` (game.mp.uo
//! 0x8a51f) reads at every level load: `vehicles/*`, one `VEHICLEFILE`
//! info string each, over the 58-key table at 0xae9c0
//! (docs/research/coduo-vehicles.md 10.1).

use vcod_common::pk3::Pk3Fs;
use vcod_common::text::atoi;

/// `type`: `VEH_ParseSpecificField` (0x7c1ca) maps these strings to 0..5.
pub const KIND_FOUR_WHEEL: i32 = 1;
pub const KIND_TANK: i32 = 2;
pub const KIND_ARTILLERY: i32 = 5;
const KINDS: [&str; 6] = [
    "** unknown **",
    "4 wheel",
    "tank",
    "plane",
    "boat",
    "artillery",
];

/// `maxSpeed`, `accel`, `collisionSpeed` and `engineSndSpeed` are mph in
/// the file, scaled to units/s after the parse (17.6f at 0xa72b4).
pub const MPH: f32 = 17.6;
/// The most files the parse takes (`Com_Error` past it).
const MAX_FILES: usize = 64;
/// The file must start with this and fit 8 KB past it.
const HEADER: &str = "VEHICLEFILE";
const MAX_BODY: usize = 0x2000;

/// The seven alias keys at `info+0x174 + i*0x40`, registered in this order.
pub const SOUND_KEYS: [&str; 7] = [
    "lowIdleSnd",
    "highIdleSnd",
    "lowEngineSnd",
    "highEngineSnd",
    "turretSpinSnd",
    "turretStopSnd",
    "impactSnd",
];
/// What every file registers after its seven (`info+0x33b`).
pub const HIT_PERSON_ALIAS: &str = "vehicle_hit_person";

/// A turret's or a gunner's arcs and rate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spans {
    pub horiz_left: f32,
    pub horiz_right: f32,
    pub vert_up: f32,
    pub vert_down: f32,
    pub rot_rate: f32,
}

/// One parsed `vehicles/<name>` file (the 0x3dc-byte info record).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleInfo {
    /// The file name as listed, which `vehicletype` matches case-insensitively.
    pub name: String,
    pub kind: i32,
    pub steer_wheels: bool,
    pub texture_scroll: bool,
    pub quad_barrel: bool,
    pub bullet_damage: bool,
    pub grenade_damage: bool,
    pub projectile_damage: bool,
    pub has_gunner: bool,
    pub num_passengers: i32,
    pub texture_scroll_scale: f32,
    /// Units/s.
    pub max_speed: f32,
    /// Units/s².
    pub accel: f32,
    /// Degrees/s.
    pub rot_rate: f32,
    /// Degrees/s².
    pub rot_accel: f32,
    pub max_body_pitch: f32,
    pub max_body_roll: f32,
    pub collision_damage: f32,
    pub collision_speed: f32,
    pub suspension_travel: f32,
    pub turret_weapon: String,
    pub turret_alt_weapon: String,
    pub turret: Spans,
    pub gunner_weapon: String,
    pub gunner: Spans,
    pub sounds: [String; 7],
    pub engine_snd_speed: f32,
    pub dmg_scale_front: f32,
    pub dmg_scale_side: f32,
    pub dmg_scale_behind: f32,
    pub dmg_scale_under: f32,
    pub dmg_scale_bullet: f32,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub accel_fwd_roll: f32,
    pub accel_side_roll: f32,
    pub accel_spring_tension: f32,
    pub stepsize: f32,
    pub collision_front_dist: f32,
    pub collision_back_dist: f32,
    pub use_hint_string: String,
    /// `info+0x334..0x33a`: the seven aliases' sound indices, 0 for an
    /// empty key; `info+0x33b` `vehicle_hit_person`'s. Filled by
    /// [`VehicleInfos::register_sounds`].
    pub sound_index: [i32; 7],
    pub hit_person_index: i32,
}

impl VehicleInfo {
    /// `info+0x354`/`0x360`: half the file's box, the hull the slide moves.
    pub fn half_mins(&self) -> [f32; 3] {
        self.mins.map(|v| v * 0.5)
    }
    pub fn half_maxs(&self) -> [f32; 3] {
        self.maxs.map(|v| v * 0.5)
    }

    /// `ParseConfigStringToStruct` over one file's info string, the header
    /// already stripped, then the scalings. `None` on a bad `type`, which is
    /// retail's `Com_Error`.
    pub fn parse(name: &str, body: &str) -> Option<VehicleInfo> {
        let kv = info_pairs(body);
        let get = |k: &str| {
            kv.iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let f = |k: &str| atof(get(k));
        let b = |k: &str| atoi(get(k)) != 0;
        let s = |k: &str| get(k).to_string();
        let kind_text = get("type");
        let kind = KINDS
            .iter()
            .position(|t| t.eq_ignore_ascii_case(kind_text))? as i32;
        let spans = |pre: &str| Spans {
            horiz_left: f(&format!("{pre}HorizSpanLeft")),
            horiz_right: f(&format!("{pre}HorizSpanRight")),
            vert_up: f(&format!("{pre}VertSpanUp")),
            vert_down: f(&format!("{pre}VertSpanDown")),
            rot_rate: f(&format!("{pre}RotRate")),
        };
        Some(VehicleInfo {
            name: name.to_string(),
            kind,
            steer_wheels: b("steerWheels"),
            texture_scroll: b("texureScroll"),
            quad_barrel: b("quadBarrel"),
            bullet_damage: b("bulletDamage"),
            grenade_damage: b("grenadeDamage"),
            projectile_damage: b("projectileDamage"),
            has_gunner: b("hasGunner"),
            num_passengers: atoi(get("numPassengers")),
            texture_scroll_scale: f("texureScrollScale"),
            max_speed: f("maxSpeed") * MPH,
            accel: f("accel") * MPH,
            rot_rate: f("rotRate"),
            rot_accel: f("rotAccel"),
            max_body_pitch: f("maxBodyPitch"),
            max_body_roll: f("maxBodyRoll"),
            collision_damage: f("collisionDamage"),
            collision_speed: f("collisionSpeed") * MPH,
            suspension_travel: f("suspensionTravel"),
            turret_weapon: s("turretWeapon"),
            turret_alt_weapon: s("turretAltWeapon"),
            turret: spans("turret"),
            gunner_weapon: s("turretGunnerWeapon"),
            gunner: spans("turretGunner"),
            sounds: SOUND_KEYS.map(s),
            engine_snd_speed: f("engineSndSpeed") * MPH,
            dmg_scale_front: f("dmgScaleFront"),
            dmg_scale_side: f("dmgScaleSide"),
            dmg_scale_behind: f("dmgScaleBehind"),
            dmg_scale_under: f("dmgScaleUnder"),
            dmg_scale_bullet: f("dmgScaleBullet"),
            mins: [f("minsX"), f("minsY"), f("minsZ")],
            maxs: [f("maxsX"), f("maxsY"), f("maxsZ")],
            accel_fwd_roll: f("accelFwdRollDegrees"),
            accel_side_roll: f("accelSideRollDegrees"),
            accel_spring_tension: f("accelSpringTension"),
            stepsize: f("stepsize"),
            collision_front_dist: f("collisionFrontDist"),
            collision_back_dist: f("collisionBackDist"),
            use_hint_string: s("useHintString"),
            sound_index: [0; 7],
            hit_person_index: 0,
        })
    }
}

/// Every vehicle file of the level, in `trap_FS_GetFileList`'s order.
#[derive(Clone, Debug, Default)]
pub struct VehicleInfos {
    pub list: Vec<VehicleInfo>,
}

impl VehicleInfos {
    /// The files under `vehicles/` in search order (newest pak first, each
    /// pak's own entry order, a name seen before skipped), at most 64.
    /// A file that fails the header, size or `type` check is skipped where
    /// retail stops the level.
    pub fn load(fs: &Pk3Fs) -> VehicleInfos {
        let mut list = Vec::new();
        for name in fs
            .list_dir_in_search_order("vehicles")
            .into_iter()
            .take(MAX_FILES)
        {
            let Some(bytes) = fs.read(&format!("vehicles/{name}")) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            let Some(body) = text.strip_prefix(HEADER) else {
                log::warn!("vehicles/{name}: not a VEHICLEFILE");
                continue;
            };
            if body.len() >= MAX_BODY {
                log::warn!("vehicles/{name}: over 8 KB");
                continue;
            }
            match VehicleInfo::parse(&name, body) {
                Some(info) => list.push(info),
                None => log::warn!("vehicles/{name}: unknown vehicle type"),
            }
        }
        VehicleInfos { list }
    }

    /// The tail of `G_ParseScrVehicleInfo`: per file in list order, each
    /// non-empty alias of the seven, then `vehicle_hit_person`, through
    /// `index` (`G_SoundAliasIndex`), which returns the alias number.
    pub fn register_sounds(&mut self, mut index: impl FnMut(&str) -> i32) {
        for info in &mut self.list {
            for (i, alias) in info.sounds.iter().enumerate() {
                info.sound_index[i] = if alias.is_empty() { 0 } else { index(alias) };
            }
            info.hit_person_index = index(HIT_PERSON_ALIAS);
        }
    }

    /// `fn_7c33f`: the file a `vehicletype` names, by `strcasecmp`.
    pub fn find(&self, vehicletype: &str) -> Option<usize> {
        self.list
            .iter()
            .position(|i| i.name.eq_ignore_ascii_case(vehicletype))
    }
}

/// `\k\v\k\v` pairs, the leading backslash optional.
fn info_pairs(s: &str) -> Vec<(String, String)> {
    let s = s.strip_prefix('\\').unwrap_or(s);
    let parts: Vec<&str> = s.split('\\').collect();
    parts
        .chunks(2)
        .map(|kv| (kv[0].to_string(), kv.get(1).unwrap_or(&"").to_string()))
        .collect()
}

fn atof(s: &str) -> f32 {
    crate::game::builtins::cvar::atof(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANZER: &str = r"\type\tank\steerWheels\0\texureScroll\1\quadBarrel\0\bulletDamage\0\grenadeDamage\1\projectileDamage\1\hasGunner\1\numPassengers\0\texureScrollScale\5.5\maxSpeed\15\accel\12.55\rotRate\50\rotAccel\70\maxBodyPitch\5\maxBodyRoll\5\collisionDamage\100\collisionSpeed\15\suspensionTravel\7\useHintString\GMI_VEHICLE_PANZERIV_HINT\turretWeapon\PanzerIV_turret_mp\turretAltWeapon\mg34_tank_mp\turretHorizSpanLeft\180\turretHorizSpanRight\180\turretVertSpanUp\30\turretVertSpanDown\15\turretRotRate\30\turretGunnerWeapon\mg42_turret_mp\turretGunnerHorizSpanLeft\180\turretGunnerHorizSpanRight\180\turretGunnerVertSpanUp\50\turretGunnerVertSpanDown\30\turretGunnerRotRate\360\lowIdleSnd\panzerIV_idle_low\highIdleSnd\panzerIV_idle_high\lowEngineSnd\panzerIV_engine_low\highEngineSnd\panzerIV_engine_high\turretSpinSnd\panzerIV_turret_spin\turretStopSnd\panzerIV_turret_stop\impactSnd\vehicle_impact\engineSndSpeed\10\dmgScaleFront\33\dmgScaleSide\50\dmgScaleBehind\100\dmgScaleBullet\100\dmgScaleUnder\200\minsX\-70\minsY\-70\minsZ\0\maxsX\70\maxsY\70\maxsZ\120\accelFwdRollDegrees\2.5\accelSideRollDegrees\2.5\accelSpringTension\1.2\stepsize\42\collisionFrontDist\105\collisionBackDist\112";

    #[test]
    fn the_panzer_file_parses_with_retails_scalings() {
        let i = VehicleInfo::parse("panzerIV_mp", PANZER).unwrap();
        assert_eq!(i.kind, KIND_TANK);
        assert!(i.has_gunner && !i.steer_wheels && !i.bullet_damage);
        assert_eq!(i.max_speed, 15.0 * MPH);
        assert_eq!(i.accel, 12.55 * MPH);
        assert_eq!(i.turret.rot_rate, 30.0);
        assert_eq!(i.gunner.vert_up, 50.0);
        assert_eq!(i.turret_weapon, "PanzerIV_turret_mp");
        assert_eq!(i.sounds[6], "vehicle_impact");
        assert_eq!(i.half_maxs(), [35.0, 35.0, 60.0]);
        assert_eq!(i.use_hint_string, "GMI_VEHICLE_PANZERIV_HINT");
    }

    /// The UO install's 39 files in retail's order: `pakuo07` first, then
    /// `pakuo03` less its `_mp` copies, then `main/pak0`
    /// (docs/research/coduo-vehicles.md 10.1).
    #[test]
    fn the_uo_install_lists_retails_files_in_retails_order() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let infos = VehicleInfos::load(&fs);
        let names: Vec<&str> = infos.list.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names.len(), 39, "{names:?}");
        assert_eq!(
            &names[..9],
            [
                "elefant_mp",
                "flak88_mp",
                "gaz67b_mp",
                "horch_mp",
                "panzerIV_mp",
                "shermantank_mp",
                "su152_mp",
                "t34_mp",
                "willyjeep_mp"
            ]
        );
        assert_eq!(names[9], "BF109");
        assert_eq!(names[23], "C47");
    }
}
