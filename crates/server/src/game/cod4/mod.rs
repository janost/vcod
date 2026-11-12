//! CoD4 1.7's level on the shared host: its zones as the asset source, its
//! scripts off the zones' rawfiles, its configstring layout and the state
//! its game module keeps beside the shared tables (docs/research/cod4-gsc.md).
//! The server does not serve CoD4 yet; [`load`] builds the host and runs the
//! bootstrap.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use vcod_cod4::assets::misc::StringTable;
use vcod_cod4::install::Install;
use vcod_cod4::source::Cod4Source;
use vcod_common::asset_source::AssetSource;
use vcod_common::net::protocol::{Cod1Wire, CsLayout, CursorHints};
use vcod_gsc::ScriptSource;

use crate::defs::Defs;
use crate::game::script::{Carry, LevelMap, ScriptRuntime};
use crate::server::MAX_CLIENTS;

pub mod bot_feed;
pub mod bullet;
pub mod clip;
pub mod combat;
pub mod corpse;
pub mod entity_damage;
pub mod level;
pub mod missile;
mod turret;
mod use_key;
pub mod vehicle;
pub mod weapons;
pub mod wire;

#[cfg(test)]
pub(crate) mod boot;
#[cfg(test)]
mod census;
#[cfg(test)]
mod turret_tests;

/// 1.7's configstring blocks as vcod-server allocates in them: each range
/// runs from the first slot its indexer in `cod4_lnxded` writes to the
/// last it scans, the same upper bounds as the wire's
/// `vcod_cod4::net::wire::CS_LAYOUT_17` (cod4-handshake.md 5). The indexers
/// that take a name (`G_FindConfigstringIndex`, 0x80cae70, whose scan
/// starts at 1, 0x80caeb6; `G_ModelIndex` 0x80cac52, from 1 below 512,
/// 0x80caccc..0x80cace6) never write their block's base; the status icon,
/// head icon and menu loops start on it. The capture's first model (831),
/// effect (1599), shellshock (1955), localized string (310) and material
/// (2003) sit one past their bases. VERIFIED (the constants, the capture).
pub const CS_LAYOUT_COD4: CsLayout = CsLayout {
    ranges: [
        // Unrolled over 0x8d3..0x8da (0x80bc95d..0x80bca91).
        (2259, 2266), // StatusIcon
        // 0x8db up to 0x8ea (0x80bc62e, 0x80bc664).
        (2267, 2281), // HeadIcon
        // From 0x8ea, 32 slots (0x80cb1b2).
        (2283, 2313), // Tag
        // 0x33e plus 1..511 (0x80cac45, 0x80cad3d).
        (831, 1341), // Model
        // From 0x53e, 256 slots (0x80cb114).
        (1343, 1597), // SoundAlias
        // From 0x63e, 100 slots (0x80cb17f).
        (1599, 1697), // Effect
        // From 0x6a2, 256 slots (0x80bdfa5).
        (1699, 1953), // FxTag
        // From 0x7a2, 16 slots (0x80cb14a).
        (1955, 1969), // ShellShock
        // 0x7b2 up to 0x7d2 (0x80bcc6c, 0x80bcc7c).
        (1970, 2001), // Menu
        // From 0x135, 512 slots (0x80cb266).
        (310, 820), // Localized
        // From 0x7d2, 256 slots (0x80cb202).
        (2003, 2257), // Shader (server materials)
    ],
    // The mirrored dvars (flag 0x100): names from 0x14, 0x80 of them
    // (0x8175017..0x817502e), each value 0x80 past its name (0x8174378).
    cvar_names: (20, 147),
    cvar_values: 148,
    // `setHintString`'s 32 slots from 0x115 (0x80c6270, the bound at
    // 0x80c6267; `Too many different hintstring values`, 0x821f224).
    hint_strings: 277,
    match_timeout: None,
    // `callvote`'s time, string, yes and no at 0xd..0x10 (0x80ae074..0x80ae0d2).
    vote: 13,
    // Fog, the end time and the winner are the slots the captures carry
    // (cod4-handshake.md 5).
    fog: 9,
    level_start: 11,
    // `g_motd`'s value, after the worldspawn's `message` at 3 (0x80c82bd).
    motd: 10,
    winner: 19,
};

/// `MAX_CONFIGSTRINGS`: `SV_SetConfigstring` (0x8173e54) refuses an index
/// above 2441 (the compare at 0x8173e60) and `SV_SendClientGameState`
/// (0x816e10a) loops to 2442, as the wire's
/// `vcod_cod4::net::parse::MAX_CONFIGSTRINGS`. VERIFIED. The highest slot
/// 1.7's gamestate carries is 2314 (cod4-handshake.md 5).
pub const MAX_CONFIGSTRINGS: usize = 2442;

/// The level time the match ends (`setGameEndTime`).
pub const CS_GAMEENDTIME: usize = 11;
/// The map centre `setMapCenter` writes.
pub const CS_MAPCENTER: usize = 12;
/// 1.7's ambient sound, north yaw, minimap and the two vision sets.
pub const CS_AMBIENT: usize = 821;
pub const CS_NORTHYAW: usize = 822;
pub const CS_MINIMAP: usize = 823;
pub const CS_VISIONSET_NAKED: usize = 824;
pub const CS_VISIONSET_NIGHT: usize = 825;
/// `beginLocationSelection`'s materials, three slots.
pub const CS_LOC_SEL_MTLS: usize = 827;
/// The weapon files, space-separated, `defaultweapon_mp` first.
pub const CS_WEAPONFILES: usize = 2258;
/// The registered weapons' bits.
pub const CS_ITEMS: usize = 2314;
/// `MAX_WEAPONS`: the items bitstring's 32 digits.
pub const MAX_WEAPONS: usize = 128;

/// The host's CoD1-shaped protocol for a CoD4 level: CoD2's shim with 1.7's
/// configstring blocks. Nothing goes on a wire through it yet (S-C4-1 adds
/// `Cod4Wire`); the host reads only its `cs`.
pub static COD4_HOST_PROTO: Cod1Wire = {
    use vcod_cod2::net::PROTOCOL_118 as P;
    let shim = P.as_cod1_shim();
    Cod1Wire {
        transport: P.transport,
        entity_fields: P.entity_fields,
        vehicle_fields: &[],
        vehicle_field_slots: &[],
        player_fields: P.player_fields,
        client_fields: P.client_fields,
        pm_flags_to_v1: shim.pm_flags_to_v1,
        pm_flags_from_v1: shim.pm_flags_from_v1,
        map_restart_command: P.map_restart_command,
        eflags_to_v1: shim.eflags_to_v1,
        eflags_from_v1: shim.eflags_from_v1,
        event_from_v1: shim.event_from_v1,
        max_weapons: P.max_weapons,
        cursor_hints: CursorHints {
            health: 0,
            weapon: 0,
        },
        hud: P.hud,
        usercmd_weapon_bits: P.usercmd_weapon_bits,
        cs: CS_LAYOUT_COD4,
        et_events: P.et_events,
        event_to_v1: shim.event_to_v1,
        mod_to_v1: shim.mod_to_v1,
        pure_dlls: ["", ""],
        team_colour_codes: false,
    }
};

/// 1.7's tables. The weapon list is the level's (placeholder: none yet);
/// `modNames` in the order `cod4_lnxded` registers them (0x80a4109).
pub static COD4_DEFS: Defs = Defs {
    proto: &COD4_HOST_PROTO,
    weapons: "",
    mod_names: &[
        "MOD_UNKNOWN",
        "MOD_PISTOL_BULLET",
        "MOD_RIFLE_BULLET",
        "MOD_GRENADE",
        "MOD_GRENADE_SPLASH",
        "MOD_PROJECTILE",
        "MOD_PROJECTILE_SPLASH",
        "MOD_MELEE",
        "MOD_HEAD_SHOT",
        "MOD_CRUSH",
        "MOD_TELEFRAG",
        "MOD_FALLING",
        "MOD_SUICIDE",
        "MOD_TRIGGER_HURT",
        "MOD_EXPLOSIVE",
        "MOD_IMPACT",
    ],
    mod_flagged: &[],
};

/// The zones a 1.7 server loads ahead of the map's (the three
/// `sv_referencedFFNames` lists, cod4-handshake.md 5).
pub const BASE_ZONES: [&str; 2] = ["code_post_gfx_mp", "common_mp"];

/// The zones walked for a level: the asset source over them and the
/// install's iwds, and their string tables by lowercased name.
pub struct Cod4Zones {
    pub source: Rc<Cod4Source>,
    pub string_tables: HashMap<String, Rc<StringTable>>,
    /// Every weapon file the zones carry, lowercased, with the models its
    /// registration indexes: the 16 `worldModel`s then `projectileModel`
    /// ([`crate::game::builtins::cod4::register_weapon`]).
    pub weapon_files: HashMap<String, Rc<WeaponFile>>,
    /// Each xmodel's bone names in its order, by lowercased model name
    /// (`hidePart`'s bits).
    pub model_bones: HashMap<String, Rc<Vec<String>>>,
    /// Each xmodel's bones at its bind pose (`baseMat`), model space, by
    /// lowercased model name (`getTagOrigin`).
    pub model_tags: HashMap<String, Rc<ModelTags>>,
    /// Each xmodel's bone parents and radius by lowercased name
    /// ([`ModelRig`]).
    pub model_rigs: HashMap<String, Rc<ModelRig>>,
    /// Each xmodel's collision surfaces (`collSurfs`, model space at the
    /// bind pose) by lowercased name, for models whose `collLod` is not
    /// negative: what `DObjGeomTraceline` traces on a vehicle.
    pub model_coll: HashMap<String, Rc<Vec<vcod_cod4::assets::model::XModelCollSurf>>>,
    /// Every xanim's length and notetracks, by lowercased name
    /// (`getAnimLength`, `animHasNotetrack`, `getNotetrackTimes`).
    pub anim_clips: HashMap<String, Rc<AnimClip>>,
    pub map: vcod_cod4::map::Map,
    /// The map zone's clip map; `None` when it does not build.
    pub clip: Option<Arc<vcod_cod4::collision::ClipMap>>,
    /// The player animtree, its clips and every xmodel's skeleton and hit
    /// boxes, what the shots pose bodies with (`combat`); `None` when the
    /// tree does not load.
    pub bodies: Option<Rc<vcod_cod4::anim::BodySource>>,
    /// `main/`'s iwds for the scripts' file reads, opened by the level's
    /// first [`load_with`] and kept for its `fast_restart`s: a reopen
    /// indexes every file again, 60 ms of each round's restart
    /// (cod4-server-frame.md 9).
    pub iwds: std::cell::OnceCell<Rc<vcod_common::pk3::Pk3Fs>>,
}

impl Cod4Zones {
    /// [`BASE_ZONES`] then `map`'s zone, from `install`'s `zone/english/`.
    pub fn load(install: &Install, map: &str) -> anyhow::Result<Cod4Zones> {
        let mut zones = Vec::new();
        for z in BASE_ZONES {
            let path = install.zone("english", z);
            let zone = vcod_cod4::zone::Zone::open_path(&path)
                .with_context(|| path.display().to_string())?;
            let walked = vcod_cod4::walk::walk(zone, Default::default())
                .with_context(|| path.display().to_string())?;
            zones.push(Arc::new(walked.assets));
        }
        let (m, walked) = vcod_cod4::map::Map::load(install, "english", map)?;
        zones.push(Arc::new(walked.assets));
        Self::from_zones(install, zones, None, m)
    }

    /// The level over zones already walked, in load order, the map's last,
    /// and a mod's `mod.ff`, whose assets win over all of them
    /// (cod4-fastfile.md 13).
    pub fn from_zones(
        install: &Install,
        mut zones: Vec<Arc<vcod_cod4::assets::Assets>>,
        mod_zone: Option<Arc<vcod_cod4::assets::Assets>>,
        map: vcod_cod4::map::Map,
    ) -> anyhow::Result<Cod4Zones> {
        let mut source = Cod4Source::open(install)?;
        let mut string_tables = HashMap::new();
        let mut weapon_files = HashMap::new();
        let mut model_bones = HashMap::new();
        let clip = zones.split_last().and_then(|(a, base)| {
            let base: Vec<_> = mod_zone
                .iter()
                .chain(base.iter().rev())
                .map(|z| z.as_ref())
                .collect();
            vcod_cod4::collision::ClipMap::from_zones(a, &base)
                .map_err(|e| log::warn!("cod4: clip map: {e:#}"))
                .ok()
                .map(Arc::new)
        });
        let mut skeletons = HashMap::new();
        let mut anim_clips = HashMap::new();
        let mut model_coll = HashMap::new();
        let mut model_rigs = HashMap::new();
        for assets in mod_zone.iter().chain(&zones) {
            for x in &assets.xanims {
                let name = x.name.to_ascii_lowercase();
                if name.starts_with(',') || anim_clips.contains_key(&name) {
                    continue;
                }
                let notes = x
                    .notify
                    .iter()
                    .map(|&(n, t)| (vcod_cod4::anim::script_string(assets, n).to_string(), t))
                    .collect();
                let length = if x.frequency == 0.0 {
                    0.0
                } else {
                    1.0 / x.frequency
                };
                let root = vcod_cod4::rootmotion::RootClip::of(x);
                anim_clips.insert(
                    name,
                    Rc::new(AnimClip {
                        length,
                        notes,
                        root,
                    }),
                );
            }
        }
        for assets in mod_zone.iter().chain(&zones) {
            for x in &assets.xmodels {
                if x.name.starts_with(',') || x.bone_names.is_empty() {
                    continue;
                }
                if let Ok(m) = vcod_cod4::anim::skeleton_model(assets, x) {
                    let key = x.name.to_ascii_lowercase();
                    model_rigs.entry(key.clone()).or_insert_with(|| {
                        Rc::new(ModelRig {
                            parents: m.bones.iter().map(|b| b.parent).collect(),
                            radius: x.radius,
                        })
                    });
                    skeletons.entry(key).or_insert(m);
                }
                if x.coll_lod >= 0 && !x.coll.is_empty() {
                    model_coll
                        .entry(x.name.to_ascii_lowercase())
                        .or_insert_with(|| Rc::new(x.coll.clone()));
                }
            }
        }
        let model_tags = skeletons
            .iter()
            .map(|(k, m): (&String, &vcod_common::model::Model)| {
                let tags = m
                    .bones
                    .iter()
                    .map(|b| (b.name.clone(), b.pos, b.rot))
                    .collect();
                (k.clone(), Rc::new(tags))
            })
            .collect();
        let refs: Vec<&vcod_cod4::assets::Assets> = mod_zone
            .iter()
            .chain(zones.iter().rev())
            .map(|a| a.as_ref())
            .collect();
        // `Mantle_CreateAnims`: the clips pmove's `Rules` take.
        vcod_cod4::pmove::mantle::install(&refs);
        let bodies = vcod_cod4::anim::BodySource::from_zones(&refs, skeletons)
            .inspect(|b| log::info!("cod4: {} player clips for the hit poses", b.clip_count()))
            .inspect_err(|e| log::warn!("cod4: player anims: {e:#}, shots meet no body"))
            .ok()
            .map(Rc::new);
        // The first zone to name an asset keeps it: the mod's ahead.
        let has_mod = mod_zone.is_some();
        let mut mod_assets = None;
        zones.splice(0..0, mod_zone);
        for (i, assets) in zones.into_iter().enumerate() {
            for x in &assets.xmodels {
                // A model a zone takes from another carries a leading comma
                // and no bones of its own.
                if x.name.starts_with(',') || x.bone_names.is_empty() {
                    continue;
                }
                let names = x
                    .bone_names
                    .iter()
                    .map(|&i| vcod_cod4::anim::script_string(&assets, i).to_string())
                    .collect();
                model_bones
                    .entry(x.name.to_ascii_lowercase())
                    .or_insert_with(|| Rc::new(names));
            }
            for w in &assets.weapons {
                let model = |id: &vcod_cod4::assets::Id<_>| {
                    assets.xmodels[id.index()]
                        .name
                        .trim_start_matches(',')
                        .to_string()
                };
                let world_models: Vec<Option<String>> = w
                    .model_lists
                    .get("worldModel")
                    .into_iter()
                    .flatten()
                    .map(|m| m.as_ref().map(model))
                    .collect();
                let projectile = w.models.get("projectileModel").map(model);
                weapon_files
                    .entry(w.name.to_ascii_lowercase())
                    .or_insert_with(|| Rc::new(WeaponFile::of(w, world_models, projectile)));
            }
            for t in &assets.string_tables {
                string_tables
                    .entry(t.name.to_ascii_lowercase())
                    .or_insert_with(|| Rc::new(t.clone()));
            }
            if has_mod && i == 0 {
                // Over the zones that load after it too, by its flags.
                mod_assets = Some(assets);
            } else {
                source.add_zone(assets);
            }
        }
        if let Some(m) = mod_assets {
            source.add_zone_flagged(m, vcod_cod4::install::zone_flags::MOD);
        }
        if install.fs_game().is_some() {
            weapon_files = weapon_files_on_disk(&source, weapon_files);
        }
        Ok(Cod4Zones {
            source: Rc::new(source),
            string_tables,
            weapon_files,
            model_bones,
            model_tags,
            model_rigs,
            model_coll,
            anim_clips,
            map,
            clip,
            bodies,
            iwds: Default::default(),
        })
    }
}

/// One model's bones at its bind pose: name, model-space origin and turn.
pub type ModelTags = Vec<(String, glam::Vec3, glam::Quat)>;

/// What a DObj of one xmodel reads beside [`ModelTags`]: each bone's
/// parent in the same order (-1 for a root), and the model's radius, which
/// `DObjGetBounds` (`cod4_lnxded` 0x81ac44c) makes the entity's box on
/// every axis (the DObj's radius is its models' summed, 0x81ac954).
#[derive(Debug)]
pub struct ModelRig {
    pub parents: Vec<i32>,
    pub radius: f32,
}

/// What registering one weapon file touches.
pub struct WeaponFile {
    /// `worldModel[0..16]`, `None` where one is empty (`getWeaponModel`).
    pub world_models: Vec<Option<String>>,
    /// The 16 `worldModel`s then `projectileModel`, the ones it has.
    pub models: Vec<String>,
    /// `altWeapon`, registered right after it.
    pub alt: Option<String>,
    /// The def fields the loadout reads.
    pub def: weapons::WeaponDef4,
    /// The def fields pmove's weapon half and the shots read.
    pub pm: Option<vcod_cod4::pmove::weapon::WeaponDef>,
    /// `useHintString` and `dropHintString` (+2012, +2016), which
    /// `G_RegisterWeapon` takes hint-string slots for, `None` when empty.
    pub use_hint: Option<String>,
    pub drop_hint: Option<String>,
    /// A turret's `playerSpread` (+1968), the gunner's spread in degrees.
    pub player_spread: f32,
}

impl WeaponFile {
    /// The file off a weapon's struct, its world models and projectile
    /// named.
    fn of(
        w: &vcod_cod4::assets::weapon::Weapon,
        world_models: Vec<Option<String>>,
        projectile: Option<String>,
    ) -> WeaponFile {
        let models = world_models
            .iter()
            .flatten()
            .cloned()
            .chain(projectile)
            .collect();
        let alt = w
            .strings
            .get("altWeapon")
            .filter(|a| !a.is_empty())
            .map(|a| a.to_ascii_lowercase());
        let hint = |k: &str| w.strings.get(k).filter(|s| !s.is_empty()).cloned();
        WeaponFile {
            world_models,
            models,
            alt,
            def: weapons::WeaponDef4::of(w),
            pm: vcod_cod4::pmove::weapon::WeaponDef::of(w),
            use_hint: hint("useHintString"),
            drop_hint: hint("dropHintString"),
            player_spread: w
                .raw
                .get(1968..1972)
                .map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap())),
        }
    }
}

/// Under a set `fs_game` `BG_LoadWeaponDef` reads `weapons/mp/<name>` off
/// the search path, not the zone's struct (cod4-fastfile.md 13), a mod's
/// iwd over `main/`'s: every weapon the zones carry and every file
/// there. A weapon whose file is missing or fails to parse takes
/// `defaultweapon_mp`'s under its own name, as `BG_LoadWeaponDef_LoadObj`
/// does; the zone's struct only when that fails too.
fn weapon_files_on_disk(
    source: &Cod4Source,
    zone: HashMap<String, Rc<WeaponFile>>,
) -> HashMap<String, Rc<WeaponFile>> {
    let mut names: Vec<String> = zone.keys().cloned().collect();
    for path in source.iwds().list_prefix("weapons/mp/") {
        let name = &path["weapons/mp/".len()..];
        if !name.is_empty() && !name.contains('/') {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    let text = |name: &str| match vcod_cod4::weapon_file::load(source, name)? {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("cod4: weapons/mp/{name}: {e:#}");
            None
        }
    };
    let file = |t: vcod_cod4::weapon_file::TextWeapon, name: &str| {
        let mut w = t.weapon;
        w.name = name.to_string();
        w.strings.insert("name", name.to_string());
        let world = t
            .model_lists
            .get("worldModel")
            .cloned()
            .unwrap_or_else(|| vec![None; 16]);
        let projectile = t.models.get("projectileModel").cloned();
        let mut f = WeaponFile::of(&w, world, projectile);
        if let Some(guns) = t.model_lists.get("gunModel") {
            f.def.gun_models = weapons::gun_bits(guns.iter().map(Option::is_some));
        }
        Rc::new(f)
    };
    let mut out = HashMap::new();
    for name in names {
        let disk = text(&name).or_else(|| text("defaultweapon_mp"));
        match disk {
            Some(t) => {
                out.insert(name.clone(), file(t, &name));
            }
            None => {
                if let Some(z) = zone.get(&name) {
                    out.insert(name, z.clone());
                }
            }
        }
    }
    log::info!("cod4: {} weapon files read off the search path", out.len());
    out
}

/// The scripts a CoD4 level compiles: a directory's loose `.gsc` first,
/// when one is named (the probe override, as `fs_game` makes retail read
/// loose files), then the zones' rawfiles.
pub struct Cod4Scripts {
    assets: Rc<dyn AssetSource>,
    overrides: Option<PathBuf>,
}

impl Cod4Scripts {
    pub fn new(assets: Rc<dyn AssetSource>, overrides: Option<PathBuf>) -> Cod4Scripts {
        Cod4Scripts { assets, overrides }
    }
}

impl ScriptSource for Cod4Scripts {
    fn read(&self, canonical: &str) -> Option<String> {
        let path = format!("{canonical}.gsc");
        if let Some(dir) = &self.overrides
            && let Ok(text) = std::fs::read(dir.join(&path))
        {
            return Some(String::from_utf8_lossy(&text).into_owned());
        }
        let bytes = self.assets.read(&path)?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// What CoD4's game module keeps on the host that CoD1's has no field for
/// ([`crate::game::host::GameHost::cod4`]).
pub struct Cod4HostState {
    /// The zones' string tables by lowercased name (`tableLookup`).
    pub string_tables: HashMap<String, Rc<StringTable>>,
    /// Each slot's stats block (`getStat`/`setStat`): the client's own
    /// once its `stats` packets are in, a fresh profile's before.
    pub stats: Vec<vcod_cod4::net::stats::Stats>,
    /// The weapons the level named, index 1 first (placeholder until the
    /// weapon defs land; configstring [`CS_WEAPONFILES`]).
    pub weapons: Vec<String>,
    /// The same weapons' defs and ammo slots, by number.
    pub table: weapons::Cod4Weapons,
    /// Every weapon file the zones carry and the models it registers
    /// ([`Cod4Zones::weapon_files`]; `precacheItem` refuses others).
    pub weapon_files: HashMap<String, Rc<WeaponFile>>,
    /// [`Cod4Zones::model_bones`].
    pub model_bones: HashMap<String, Rc<Vec<String>>>,
    /// `s.partBits` of each entity `hidePart` touched, by number, with the
    /// handle it was set on: one bit per bone of its model.
    pub part_bits: HashMap<u32, (vcod_gsc::EntId, [u32; 4])>,
    /// `script_struct` blocks in lump order, for the load to turn into
    /// `level.struct` once the spawn pass is done.
    pub script_structs: Vec<Vec<(String, String)>>,
    /// Each slot's client-state fields the script sets.
    pub clients: Vec<Cod4ClientState>,
    /// The tenth and later arguments of the damage and killed callbacks
    /// (`timeOffset`, `deathAnimDuration`), set around each call.
    pub callback: crate::game::host::Cod2CallbackArgs,
    /// What `finishPlayerDamage` did to each victim's playerstate, for the
    /// level to apply (`combat::Damage4`).
    pub damage_ops: Vec<combat::Damage4>,
    /// The live grenades `player_die` drops, for the level to fire
    /// (`combat::DeathGrenade4`).
    pub death_grenades: Vec<combat::DeathGrenade4>,
    /// The weapons `dropItem` put in the world (`combat::Item4`).
    pub items: Vec<combat::Item4>,
    /// The grenades in the world and the fuses' burst entities
    /// (`missile`).
    pub missiles: Vec<missile::Missile4>,
    pub bursts: Vec<missile::Burst4>,
    /// Entities freed once the next frame's engine notifies have reached
    /// them (a burst grenade's `explode` and `death`).
    pub free_next: Vec<vcod_gsc::EntId>,
    /// Each slot's body as its last end frame left it, what `clonePlayer`
    /// and `dropItem` read.
    pub corpse_source: Vec<Option<crate::game::builtins::cod4_combat::CorpseSource>>,
    /// Each slot's `death` event commands as its last end frame would play
    /// them; `player_die` draws one (`vcod_cod4::pmove::anim::event_choices`).
    pub death_anims: Vec<Vec<vcod_cod4::pmove::anim::EventAnim>>,
    /// Each slot's `pm_flags` after its last cmd (`isOnLadder`).
    pub pm_flags: Vec<u32>,
    /// Each slot's view after its last cmd (`getPlayerAngles`).
    pub view_angles: Vec<[f32; 3]>,
    /// The map's clip map ([`Cod4Zones::clip`]).
    pub clip: Option<Arc<vcod_cod4::collision::ClipMap>>,
    /// The process's glibc `rand()`, seeded by `G_InitGame`'s `srand`
    /// (0x80b56ee) at each load: the map's weapon pickups, the challenges,
    /// the anim script's events, the fire recoil, a drop's spreads, the
    /// death anim, the grenades (cod4-combat.md 10.9). The scripts'
    /// `randomInt` and `randomFloat` draw elsewhere.
    pub rand: vcod_common::rng::GlibcRand,
    /// The effect entities `spawnFx` and `playLoopedFx` made, by handle
    /// (`builtins::cod4_script`); the level sends each while it lives.
    pub fx: HashMap<vcod_gsc::EntId, Fx4>,
    /// Every xanim the zones carry, by lowercased name
    /// ([`Cod4Zones::anim_clips`]).
    pub anim_clips: HashMap<String, Rc<AnimClip>>,
    /// The player animtree's anims by wire index (`legsAnim & 0x1ff`), once
    /// the level has loaded them.
    pub anim_names: Rc<Vec<String>>,
    /// Each body queue corpse's `legsAnim`, by entity number
    /// (`getCorpseAnim`).
    pub corpse_anims: HashMap<u32, u32>,
    /// [`Cod4Zones::model_tags`].
    pub model_tags: HashMap<String, Rc<ModelTags>>,
    /// [`Cod4Zones::model_rigs`].
    pub model_rigs: HashMap<String, Rc<ModelRig>>,
    /// [`Cod4Zones::model_coll`].
    pub model_coll: HashMap<String, Rc<Vec<vcod_cod4::assets::model::XModelCollSurf>>>,
    /// Each slot's `ps.velocity` after its last cmd (`getVelocity`).
    pub velocity: Vec<[f32; 3]>,
    /// Each slot's last cmd's buttons and the ones pressed since the last
    /// end frame (`client->buttons`, `buttonsSinceLastFrame`).
    pub buttons: Vec<u32>,
    pub buttons_since: Vec<u32>,
    /// `level.teamHasRadar` by team (none, axis, allies).
    pub team_radar: [bool; 3],
    /// `detonate`'s grenades, each with its new parent (`Some(None)` the
    /// world, `None` unchanged), for the next missile pass.
    pub detonations: Vec<(vcod_gsc::EntId, Option<Option<usize>>)>,
    /// `spawnPlane`'s planes, each with its `teamAndOwnerIndex`
    /// (`vehicle`).
    pub planes: HashMap<vcod_gsc::EntId, u32>,
    /// `spawnHelicopter`'s helicopters (`vehicle::Heli4`).
    pub helis: Vec<vehicle::Heli4>,
    /// `physicsLaunch`'s entities: the launch's level time, contact point
    /// and force, which the client's physics flies from (`TR_PHYSICS`).
    pub physics: HashMap<vcod_gsc::EntId, (i32, [f32; 3], [f32; 3])>,
    /// `fireWeapon`'s rounds, for the level's next vehicle pass.
    pub vehicle_shots: Vec<vehicle::VehShot>,
    /// The missile attractors and repulsors' 32 slots (0x8361a80,
    /// `missile::Attractor`).
    pub attractors: [Option<missile::Attractor>; missile::MAX_ATTRACTORS],
    /// The bodies and brush entities the level's last frame linked, what
    /// the script traces cut (`clip::Body4`, `clip::Brush4`).
    pub trace_bodies: Vec<clip::Body4>,
    pub trace_brushes: Vec<clip::Brush4>,
    /// The same frame's playing bodies posed (`combat::PosedBody4`), what
    /// the locational script traces score instead of their boxes.
    pub posed_bodies: Vec<combat::PosedBody4>,
}

/// 1.7's `ET_FX` and `ET_LOOP_FX`.
pub const ET_FX: u32 = 8;
pub const ET_LOOP_FX: u32 = 9;

/// One effect entity as it goes out: `un1` the effect id, its place and
/// angles, `time2` the start `triggerFx` set, and a loop's
/// `lerp.u.loopFx` cull distance and period.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fx4 {
    pub etype: u32,
    pub fx: i32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub time2: i32,
    pub cull: f32,
    pub period_ms: i32,
}

impl Fx4 {
    /// Its entity state under number `n`.
    pub fn entity(&self, n: u32) -> vcod_cod4::net::delta::EntityState {
        use vcod_cod4::net::delta::EntityState;
        let mut e = EntityState::default();
        e.0[0] = n;
        e.0[1] = self.etype;
        let table = EntityState::table(self.etype);
        let mut set = |name: &str, v: u32| {
            if let Some(f) = table.iter().find(|f| f.name == name) {
                e.0[f.word()] = v;
            }
        };
        set("un1", self.fx as u32);
        set("time2", self.time2 as u32);
        for k in 0..3 {
            set(&format!("lerp.pos.trBase[{k}]"), self.origin[k].to_bits());
            set(&format!("lerp.apos.trBase[{k}]"), self.angles[k].to_bits());
        }
        set("lerp.u.loopFx.cullDist", self.cull.to_bits());
        set("lerp.u.loopFx.period", self.period_ms as u32);
        e
    }
}

/// One xanim's length in seconds (`1 / frequency`) and its notetracks,
/// each with its time as a fraction of the clip.
#[derive(Clone, Debug, Default)]
pub struct AnimClip {
    pub length: f32,
    pub notes: Vec<(String, f32)>,
    /// The root's motion, for a corpse's death anim (`corpse`).
    pub root: Option<vcod_cod4::rootmotion::RootClip>,
}

/// The minimap's frame as `GScr_SetMiniMap` keeps it: north off
/// configstring 822, the upper left corner and the world size along the
/// map's axes off 823.
pub struct Compass {
    pub north: [f32; 2],
    pub upper_left: [f32; 2],
    pub world_size: [f32; 2],
}

impl Compass {
    /// The world point under a location cursor (`usercmd.selectedLocation`,
    /// a signed byte a side): each side's fraction of the minimap, `(c +
    /// 128) / 255`, times its world size, then turned onto the map's axes
    /// from the upper left corner, in the order `ClientThink_real` sums
    /// them (`cod4_lnxded` 0x80a662f..0x80a669d, the frame at 0x8372510).
    /// The height is 0.
    pub fn world_point(&self, cursor: [u8; 2]) -> [f32; 3] {
        let [across, down] = std::array::from_fn(|k| {
            (f32::from(cursor[k] as i8) + 128.0) / 255.0 * self.world_size[k]
        });
        let [cos, sin] = self.north;
        let [left, top] = self.upper_left;
        [
            across * sin + left - down * cos,
            top - across * cos - down * sin,
            0.0,
        ]
    }
}

/// `G_LogPrintf` (0x80b43c4) from the game: `text` queued with
/// `level.time` for `games_mp.log` (`engine::gamelog`).
pub fn log_printf(host: &mut crate::game::host::GameHost, text: String) {
    host.log_printf(text);
}

/// A client's `I_CleanStr`'d name and GUID, as `G_Say` and the weapon
/// pickup's log lines read them (`pers.netname`, `SV_GetGuid`).
pub fn log_client(host: &crate::game::host::GameHost, slot: usize) -> (String, String) {
    let name = host.client_names.get(slot).map_or("", String::as_str);
    let guid = match &host.ext {
        crate::game::host::HostExt::Cod4(c) => c.clients.get(slot).map(|c| c.guid.clone()),
        _ => None,
    };
    (crate::game::say::clean_name(name), guid.unwrap_or_default())
}

/// [`Compass`] off the level's configstrings, `None` before `setMiniMap`.
pub fn compass(host: &crate::game::host::GameHost) -> Option<Compass> {
    let yaw = crate::game::builtins::cvar::atof(&host.configstrings[CS_NORTHYAW]).to_radians();
    let north = [yaw.cos(), yaw.sin()];
    let mm = &host.configstrings[CS_MINIMAP];
    let rest = mm.rsplitn(5, ' ').collect::<Vec<_>>();
    if rest.len() < 5 {
        return None;
    }
    let n = |i: usize| rest[i].parse::<f32>().ok();
    let (ux, uy, lx, ly) = (n(3)?, n(2)?, n(1)?, n(0)?);
    let (dx, dy) = (lx - ux, ly - uy);
    Some(Compass {
        north,
        upper_left: [ux, uy],
        world_size: [
            dx * north[1] - dy * north[0],
            -dx * north[0] - dy * north[1],
        ],
    })
}

/// The seed a level's [`Cod4HostState::rand`] starts from unless the
/// server hands one in: retail's is the wall clock's milliseconds.
pub const GAME_SEED: u32 = 0x5eed;

/// `sv_fps`'s default and domain (`Dvar_RegisterInt` at 0x81735c8:
/// 20, 10..1000, no flags; cod4-server-frame.md 8).
pub const SV_FPS_DEFAULT: i32 = 20;
pub const SV_FPS_DOMAIN: std::ops::RangeInclusive<i32> = 10..=1000;

/// `sv_fps` as the server reads it: a value out of the domain, which the
/// console refuses, reads as the default.
pub fn sv_fps(cvars: &crate::cvars::Cvars) -> i32 {
    let fps = crate::game::builtins::cvar::atoi(cvars.get("sv_fps"));
    if SV_FPS_DOMAIN.contains(&fps) {
        fps
    } else {
        SV_FPS_DEFAULT
    }
}

/// The frame `sv_fps` gives: `1000 / sv_fps` in integer milliseconds, as
/// `SV_FrameInternal` divides it (0x81770c4), so 30 steps 33.
pub fn frame_ms(cvars: &crate::cvars::Cvars) -> i32 {
    1000 / sv_fps(cvars)
}

impl Cod4HostState {
    /// `rand()`: 0..2^31.
    pub fn rand(&mut self) -> i32 {
        self.rand.next_u31() as i32
    }
}

/// One client's fields beside the shared tables.
#[derive(Clone, Debug, Default)]
pub struct Cod4ClientState {
    /// `setRank`'s rank and prestige, the roster's.
    pub rank: u8,
    pub prestige: u8,
    /// The playerstate's `perks`, one bit a `PERKS` row
    /// (`builtins::cod4_client::PERKS`).
    pub perks: u32,
    /// Weapons, ammo and clips.
    pub loadout: weapons::Loadout,
    /// `ps.weapFlags` (`disableWeapons` sets 0x80).
    pub weap_flags: u32,
    /// `freezeControls`.
    pub frozen: bool,
    /// `ps.hudElemLastAssignedSoundID`, which `setPulseFX` steps.
    pub hud_sound_id: i32,
    /// `PMF_SHELLSHOCKED` as `shellShock` (on) or `stopShellShock` (off)
    /// left it, for the end frame to write.
    pub shock_pmf: Option<bool>,
    /// `setDepthOfField`'s six values, for the end frame to write.
    pub dof: Option<[f32; 6]>,
    /// `ps.locationSelectionInfo` as `beginLocationSelection` (or 0, the
    /// end) left it, for the end frame to write.
    pub location_selection: Option<u32>,
    /// `gclient_s.lastWeapon`: the weapon `G_ClientDoPerFrameNotifies`
    /// last announced with `weapon_change`.
    pub last_weapon: usize,
    /// `gclient_s.lastStand` and `lastStandTime`: Last Stand's down state
    /// and the time before which no damage lands.
    pub last_stand: bool,
    pub last_stand_time: i32,
    /// The grenade the playerstate held after its last cmd (`grenadeTimeLeft`
    /// not 0): the weapon and the time left, what `player_die` drops.
    pub held_grenade: Option<(u32, i32)>,
    /// `gclient_s.lastVoiceTime`: the level time a voice frame of this
    /// client's was last relayed (`G_BroadcastVoice`, cod4-voice.md 3),
    /// `None` before one; a spawn keeps it.
    pub last_voice_time: Option<i32>,
    /// `gclient_s.compassPingTime` (+0x3108): `pingPlayer`'s `eFlags`
    /// 0x400000 stays until this level time (cod4-hud.md 15.9).
    pub compass_ping_until: i32,
    /// The GUID the client's `getchallenge` named (`getGuid`, the log's
    /// lines); empty for a bot.
    pub guid: String,
}

impl Cod4ClientState {
    /// `ClientSpawn`'s memset of the playerstate: the weapons, perks and
    /// weapon flags go; the session's rank stays.
    pub fn spawned(&mut self) {
        self.loadout = weapons::Loadout::default();
        self.perks = 0;
        self.weap_flags = 0;
        self.hud_sound_id = 0;
        self.shock_pmf = None;
        self.last_stand = false;
        self.last_stand_time = 0;
        self.held_grenade = None;
    }
}

impl Default for Cod4HostState {
    fn default() -> Self {
        Cod4HostState {
            string_tables: HashMap::new(),
            stats: vec![vcod_cod4::net::stats::Stats::fresh(); MAX_CLIENTS],
            weapons: Vec::new(),
            table: weapons::Cod4Weapons::new(),
            weapon_files: HashMap::new(),
            model_bones: HashMap::new(),
            part_bits: HashMap::new(),
            script_structs: Vec::new(),
            clients: vec![Cod4ClientState::default(); MAX_CLIENTS],
            callback: Default::default(),
            damage_ops: Vec::new(),
            death_grenades: Vec::new(),
            items: Vec::new(),
            missiles: Vec::new(),
            bursts: Vec::new(),
            free_next: Vec::new(),
            corpse_source: vec![None; MAX_CLIENTS],
            death_anims: vec![Vec::new(); MAX_CLIENTS],
            pm_flags: vec![0; MAX_CLIENTS],
            view_angles: vec![[0.0; 3]; MAX_CLIENTS],
            clip: None,
            rand: vcod_common::rng::GlibcRand::new(GAME_SEED),
            fx: HashMap::new(),
            anim_clips: HashMap::new(),
            anim_names: Rc::new(Vec::new()),
            corpse_anims: HashMap::new(),
            model_tags: HashMap::new(),
            model_rigs: HashMap::new(),
            model_coll: HashMap::new(),
            velocity: vec![[0.0; 3]; MAX_CLIENTS],
            buttons: vec![0; MAX_CLIENTS],
            buttons_since: vec![0; MAX_CLIENTS],
            team_radar: [false; 3],
            detonations: Vec::new(),
            planes: HashMap::new(),
            helis: Vec::new(),
            physics: HashMap::new(),
            vehicle_shots: Vec::new(),
            attractors: [None; missile::MAX_ATTRACTORS],
            trace_bodies: Vec::new(),
            posed_bodies: Vec::new(),
            trace_brushes: Vec::new(),
        }
    }
}

/// Loads `gametype` on `map` from `zones` and runs the bootstrap: the
/// gametype's `main`, the map's, then `CodeCallback_StartGameType`, each to
/// its first wait. `overrides` is the probe directory [`Cod4Scripts`]
/// reads first. `fresh_mirror` starts 1.7's dvar mirror empty; the server
/// hands in its cvars with the engine's mirrored dvars already in it.
#[allow(clippy::too_many_arguments)]
pub fn load(
    zones: &Cod4Zones,
    install: &Install,
    map: &str,
    gametype: &str,
    cvars: crate::cvars::Cvars,
    fresh_mirror: bool,
    overrides: Option<PathBuf>,
    now_ms: i32,
) -> anyhow::Result<ScriptRuntime> {
    load_with(
        zones,
        install,
        map,
        gametype,
        cvars,
        fresh_mirror,
        overrides,
        now_ms,
        Carry::default(),
        None,
        GAME_SEED,
    )
}

/// The entity string with the clip map's inline models: each `cmodel_t`'s
/// bounds (`CM_ModelBounds`, what `SV_SetBrushModel` gives a brush
/// entity) and its brushes' planes, which a trigger's touch and
/// `isTouching` test (`SV_EntityContact`'s position test).
fn brush_models(entities: String, clip: Option<&vcod_cod4::collision::ClipMap>) -> LevelMap {
    let Some(clip) = clip else {
        return LevelMap::of_parts(entities, Vec::new(), Vec::new());
    };
    let bounds = clip
        .submodels
        .iter()
        .map(|m| (m.mins.to_array(), m.maxs.to_array()))
        .collect();
    let hulls = (0..clip.submodels.len())
        .map(|i| {
            clip.submodel_brush_planes(i)
                .into_iter()
                .map(|planes| crate::game::trigger::BrushHull { planes })
                .collect()
        })
        .collect();
    LevelMap::of_parts(entities, bounds, hulls)
}

/// [`load`] across a level boundary: `carry`'s `game[]`, `pers[]`, compiled
/// scripts and seed, and on a `fast_restart` the last level's
/// configstrings, which `SV_RestartGameProgs` keeps (cod4-map-cycle.md 2).
#[allow(clippy::too_many_arguments)]
pub fn load_with(
    zones: &Cod4Zones,
    install: &Install,
    map: &str,
    gametype: &str,
    mut cvars: crate::cvars::Cvars,
    fresh_mirror: bool,
    overrides: Option<PathBuf>,
    now_ms: i32,
    carried: Carry,
    configstrings: Option<Vec<String>>,
    seed: u32,
) -> anyhow::Result<ScriptRuntime> {
    let fs = match zones.iwds.get() {
        Some(fs) => fs.clone(),
        None => {
            let _p = crate::tick_profile::phase("console/iwds");
            let fs = Rc::new(vcod_common::pk3::Pk3Fs::open_iwd(
                &install.root().join("main"),
                None,
            )?);
            zones.iwds.get_or_init(|| fs.clone());
            fs
        }
    };
    let assets: Rc<dyn AssetSource> = zones.source.clone();
    cvars.set("g_gametype", gametype);
    cvars.set("mapname", map);
    // 1.7's mirror block: the engine's mirrored dvars, then the scripts'
    // `makeDvarServerInfo`.
    if fresh_mirror {
        cvars.remirror(&CS_LAYOUT_COD4, &[]);
    }
    let m = &zones.map;
    let brushes = crate::tick_profile::phase("console/brush models");
    let carry = Carry {
        map: Some(Rc::new(brush_models(
            m.entity_string.clone(),
            zones.clip.as_deref(),
        ))),
        area: Some(crate::area::AreaTree::new(
            m.mins,
            m.maxs,
            crate::game::entity::MAX_GENTITIES as usize,
        )),
        ..carried
    };
    drop(brushes);
    let _p = crate::tick_profile::phase("console/game init");
    let tables = zones.string_tables.clone();
    let weapon_files = zones.weapon_files.clone();
    let model_bones = zones.model_bones.clone();
    let model_tags = zones.model_tags.clone();
    let model_rigs = zones.model_rigs.clone();
    let model_coll = zones.model_coll.clone();
    let anim_clips = zones.anim_clips.clone();
    let clip = zones.clip.clone();
    ScriptRuntime::load_game_with(
        &crate::game::profile::COD4,
        Box::new(Cod4Scripts::new(assets.clone(), overrides)),
        fs,
        map,
        gametype,
        configstrings.unwrap_or_else(|| vec![String::new(); MAX_CONFIGSTRINGS]),
        cvars,
        zones.clip.clone().map(clip::level_world),
        Rc::new(crate::weapons::WeaponTable::empty()),
        now_ms,
        carry,
        move |host| {
            host.allocators = crate::configstrings::Allocators::with_constants(
                &CS_LAYOUT_COD4,
                vcod_cod4::net::constant_cs::CONSTANT_CONFIGSTRINGS,
            );
            host.assets = Some(assets);
            let c4 = host.cod4_mut();
            c4.string_tables = tables;
            c4.weapon_files = weapon_files;
            c4.model_bones = model_bones;
            c4.model_tags = model_tags;
            c4.model_rigs = model_rigs;
            c4.model_coll = model_coll;
            c4.anim_clips = anim_clips;
            c4.clip = clip;
            // `G_InitGame`'s `srand`, ahead of the spawn pass's draws.
            c4.rand = vcod_common::rng::GlibcRand::new(seed);
            // `defaultweapon_mp` is weapon 1 before any script runs, as the
            // capture's list opens with it.
            if let Err(e) = crate::game::builtins::cod4::register_weapon(host, "defaultweapon_mp") {
                log::warn!("cod4: defaultweapon_mp: {e:?}");
            }
        },
    )
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// The server's blocks end where the wire's do; they start one past
    /// the wire's base but for the status icons, head icons and menus,
    /// whose loops start on it.
    #[test]
    fn the_servers_configstring_blocks_match_the_wires() {
        let wire = vcod_cod4::net::wire::CS_LAYOUT_17;
        let from_base = [0, 1, 8];
        for (k, (ours, theirs)) in CS_LAYOUT_COD4.ranges.iter().zip(wire.ranges).enumerate() {
            let first = theirs.0 + usize::from(!from_base.contains(&k));
            assert_eq!(*ours, (first, theirs.1), "block {k}");
        }
        assert_eq!(CS_LAYOUT_COD4.hint_strings, wire.hint_strings);
        assert_eq!(
            MAX_CONFIGSTRINGS,
            vcod_cod4::net::parse::MAX_CONFIGSTRINGS as usize
        );
    }
}
