//! `vcod <map> --game cod4`: a stock CoD4 map in fly mode. The map comes
//! out of its zone (`vcod_cod4::map`), turns into the neutral world
//! (`vcod_cod4::mapworld`) and draws through the CoD2 world path, its
//! materials compiled by `vcod_cod4::materials`; images without zone
//! pixels load from `main/`'s iwds, which `fs` mounts. A watched session
//! (`play::cod4`) also asks for its entities' xmodels and weapons, out of
//! the map's zone and `common_mp`'s.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use vcod_cod4::assets::Assets;
use vcod_cod4::install::Install;
use vcod_cod4::map::{Map, stub_name};
use vcod_cod4::mapworld::{LightmapImages, to_mapworld};
use vcod_cod4::source::Cod4Source;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;
use vcod_common::mapworld::MapWorld;
use vcod_common::material::MaterialLib;
use vcod_common::pk3::Pk3Fs;

use crate::cod4_audio::Cod4Sounds;
use crate::cod4_fx::Cod4Effects;

/// A watched session's sounds and effects.
pub type Av = (Cod4Sounds, Cod4Effects);
/// Each entity model's surfaces' `XSurface.partBits`, by model name, in
/// its LOD's surface order.
pub type Parts = HashMap<String, Vec<[u32; 4]>>;
/// `(path, bytes)` files for the search path's overlay.
pub type OverlayFiles = Vec<(String, Vec<u8>)>;

/// A loaded CoD4 map: the world, its materials and where the camera starts.
pub struct Cod4Map {
    pub bsp: MapWorld,
    pub materials: MaterialLib,
    /// The first `mp_tdm_spawn`'s origin and yaw, else the bounds' centre.
    pub spawn: ([f32; 3], f32),
    /// The entity models asked for that a zone holds, by name, their
    /// materials in `materials`.
    pub models: HashMap<String, vcod_common::model::Model>,
    /// Each of `models`' surfaces' `partBits`, by model name: what an
    /// entity's `hidePart`s take off it (`Cod4Client::set_parts`).
    pub parts: Parts,
    /// The weapons asked for: world and projectile model names.
    pub weapons: HashMap<String, WeaponModels>,
    /// What a watched session's bodies are posed from: the player animtree,
    /// its clips and `models`. `None` in fly mode, or when no zone carries
    /// the tree.
    pub bodies: Option<vcod_cod4::anim::BodySource>,
    /// The sounds and effects (`cod4_audio`, `cod4_fx`): a watched
    /// session's, or fly mode's ([`FlyAv`]).
    pub av: Option<Av>,
    /// The map script's `ambientPlay` alias, which a server sends as
    /// configstring 821; fly mode plays it.
    pub ambient: Option<String>,
    /// The zones' loaded sounds as `.wav` files, for the search path's
    /// overlay (`cod4_audio::overlay_files`).
    pub sound_files: OverlayFiles,
    /// What a watched session predicts its own body through; `None` in
    /// fly mode.
    pub world: Option<Arc<crate::play::cod4_predict::Cod4World>>,
    /// The fog and vision sets (`cod4_look`): fly mode's from the map's
    /// script, a session's from its configstrings.
    pub look: crate::cod4_look::Cod4Look,
    /// What the HUD draws with, off the zones this load walked
    /// ([`hud_assets`]); `None` in fly mode.
    pub hud: Option<vcod_cod4::hud::HudAssets>,
}

/// What a weapon's item and missile draw, and its view weapon.
#[derive(Clone, Default)]
pub struct WeaponModels {
    pub world: Option<String>,
    pub projectile: Option<String>,
    pub view: Option<Arc<crate::cod4_viewmodel::ViewDef>>,
    /// A turret weapon's `animHorRotateInc` (`WeaponDef` +2004), the
    /// gunner's column step; 0 for the rest.
    pub rotate_inc: f32,
    /// A turret weapon's `vertViewJitter` and `horizViewJitter`
    /// (`WeaponDef` +2032, +2028), the gunner's view jitter under a held
    /// trigger.
    pub view_jitter: [f32; 2],
    /// A turret weapon's gun clips, `idleAnim` and `fireAnim`,
    /// which `CG_GetMG42Anims` makes its anims 1 and 2
    /// (cod4-client-view.md 13); `None` off a turret weapon.
    pub gun_anims: Option<[Arc<vcod_common::xanim::XAnim>; 2]>,
}

/// `WeaponDef.weapClass` (+304) and its turret value (`weapClass_t` 7).
const WEAP_CLASS: usize = 304;
const WEAPCLASS_TURRET: i32 = 7;

/// The little-endian int at `at` in a `WeaponDef`'s bytes, 0 past them.
fn i32_at(raw: &[u8], at: usize) -> i32 {
    raw.get(at..at + 4)
        .map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The little-endian float at `at` in a `WeaponDef`'s bytes, 0 past them.
fn float(raw: &[u8], at: usize) -> f32 {
    raw.get(at..at + 4)
        .map_or(0.0, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// What a session draws its entities with: xmodel and weapon names (the
/// gamestate's model configstrings and its weapon list).
#[derive(Clone, Debug, Default)]
pub struct EntityAssets {
    pub models: Vec<String>,
    pub weapons: Vec<String>,
}

/// The search path images load from: the install's `main/*.iwd`, and
/// under `fs_game` (`mods/<name>`) the level `map`'s `usermaps/<map>/`
/// and the mod's over them ([`Install::iwd_dirs`]).
pub fn open_fs(game_dir: &Path, fs_game: Option<&str>, map: Option<&str>) -> Result<Pk3Fs> {
    let dirs = match Install::at(game_dir) {
        Some(i) => i.with_fs_game(fs_game).with_map(map).iwd_dirs(),
        None => vec![game_dir.join("main")],
    };
    Pk3Fs::open_iwd_layers(&dirs[0], &dirs[1..])
        .with_context(|| format!("cannot open {}", dirs[0].display()))
}

/// Loads `zone/english/<map>.ff` (or a usermap's) from the install at
/// `game_dir`, with the entity assets `ents` names (none for fly mode).
/// Under `fs_game` its `mod.ff` loads too, and its assets win over the
/// map's and `common_mp`'s (cod4-fastfile.md 13).
///
/// A thread that set [`set_load_report`] hears how far the load has got.
pub fn load(
    game_dir: &Path,
    fs_game: Option<&str>,
    map: &str,
    ents: &EntityAssets,
) -> Result<Cod4Map> {
    load_from(game_dir, fs_game, map, ents, None)
}

/// The stock zones every level's load walks whatever its map:
/// `common_mp` and the [`SOUND_ZONES`]. A join walks them while it waits
/// for the gamestate ([`walk_stock`]).
pub struct StockZones {
    common: vcod_cod4::walk::Walked,
    sound: Vec<vcod_cod4::walk::Walked>,
}

/// [`StockZones`] off the install at `game_dir`, each zone on a thread
/// of its own.
pub fn walk_stock(game_dir: &Path) -> Result<StockZones> {
    let install = Install::at(game_dir)
        .with_context(|| format!("{} has no zone/ directory", game_dir.display()))?;
    let names: Vec<&str> = std::iter::once("common_mp").chain(SOUND_ZONES).collect();
    let mut walked = vcod_common::laps::par_map(&names, |name| {
        let path = install.zone("english", name);
        Zone::open_path(&path)
            .and_then(|z| walk(z, WalkOptions { keep_bulk: true }))
            .with_context(|| path.display().to_string())
    })
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    let sound = walked.split_off(1);
    let common = walked.pop().expect("common_mp walked");
    Ok(StockZones { common, sound })
}

/// [`load`] over `stock` walked already, or with none walked beside the
/// map's zone.
pub fn load_from(
    game_dir: &Path,
    fs_game: Option<&str>,
    map: &str,
    ents: &EntityAssets,
    stock: Option<StockZones>,
) -> Result<Cod4Map> {
    let install = Install::at(game_dir)
        .with_context(|| format!("{} has no zone/ directory", game_dir.display()))?
        .with_fs_game(fs_game)
        .with_map(Some(map));
    let mut laps = vcod_common::laps::Laps::new(format!("cod4 map {map}"));
    report_load(0.05);
    let (map_zone, stock) = std::thread::scope(|s| {
        let stock = match stock {
            Some(z) => Ok(z),
            None => vcod_common::laps::joined(s.spawn(|| walk_stock(game_dir))),
        };
        (Map::load(&install, "english", map), stock)
    });
    let (mut m, walked) = map_zone?;
    let StockZones { common, sound } = stock?;
    laps.lap("map and stock zones");
    report_load(0.6);
    let ambient = map_ambient(&walked.assets, map);
    let mod_zone = install
        .mod_zone()
        .map(|path| {
            Zone::open_path(&path)
                .and_then(|z| walk(z, WalkOptions { keep_bulk: true }))
                .with_context(|| path.display().to_string())
        })
        .transpose()?;
    if let Some(z) = &mod_zone {
        log::info!(
            "cod4: fs_game {}: mod.ff, {} menus, {} materials",
            fs_game.unwrap_or_default(),
            z.assets.menus.len(),
            z.assets.materials.len()
        );
    }
    // Highest priority first: the mod's, the map's, `common_mp`'s.
    let zones: Vec<&Assets> = mod_zone
        .iter()
        .map(|z| &z.assets)
        .chain([&walked.assets, &common.assets])
        .collect();
    // stub technique sets' keys for 1.7's material order (cod4-world.md 12)
    m.resolve_material_orders(&zones);
    let fly = ents.models.is_empty() && ents.weapons.is_empty();
    let hud = (!fly).then(|| {
        let stock = std::iter::once(("common_mp", &common.assets))
            .chain(SOUND_ZONES.into_iter().zip(sound.iter().map(|z| &z.assets)));
        hud_assets(stock, &walked.assets, mod_zone.as_ref().map(|z| &z.assets))
    });
    laps.lap("hud assets");
    // the maps after the first release carry their vision set in their zone
    let mut visions = vcod_cod4::look::visions(&common.assets.raw_files);
    visions.extend(vcod_cod4::look::visions(&walked.assets.raw_files));
    let look = if fly {
        let fog = vcod_cod4::look::map_fog(&walked.assets.raw_files, map);
        log::info!("{map}: script fog {fog:?}");
        crate::cod4_look::Cod4Look::fly(visions, map, fog)
    } else {
        crate::cod4_look::Cod4Look::new(visions)
    };
    let (models, parts, weapons, bodies, av, sound_files, world) = if fly {
        // Fly mode: no entities, but the map's effects and sounds.
        let (av, files) = sound_and_fx(&mut m, &zones, map, &sound)?;
        (
            HashMap::new(),
            Parts::new(),
            HashMap::new(),
            None,
            Some(av),
            files,
            None,
        )
    } else {
        let (models, weapons, parts) = entity_models(&mut m, &zones, ents);
        laps.lap("entity models");
        let bodies = vcod_cod4::anim::BodySource::from_zones(&zones, models.clone())
            .inspect(|b| log::info!("cod4: {} player clips", b.clip_count()))
            .inspect_err(|e| log::warn!("cod4: player anims: {e:#}, players stay unposed"))
            .ok();
        laps.lap("player anims");
        let (av, files) = sound_and_fx(&mut m, &zones, map, &sound)?;
        laps.lap("sound and fx");
        let world = crate::play::cod4_predict::Cod4World::from_assets(&walked.assets, &zones)
            .inspect_err(|e| log::warn!("cod4: clip map: {e:#}, no prediction"))
            .ok()
            .map(|mut w| {
                if let Ok(fs) = open_fs(game_dir, fs_game, Some(map)) {
                    if fs_game.is_some() {
                        let n = w.overlay_weapon_files(&fs);
                        log::info!("cod4: {n} weapon files read off the search path");
                    }
                    if !w.read_ragdoll(&fs) {
                        log::warn!("cod4: no ragdoll.cfg, corpses keep their death anims");
                    }
                }
                Arc::new(w)
            });
        (models, parts, weapons, bodies, Some(av), files, world)
    };
    // the entity and effect materials added since: a model's stub set
    // left unresolved read prepass key 3, no `floatZ`, and kept the
    // entity out of the depth prepass (cod4-client-perf.md 37)
    m.resolve_material_orders(&zones);
    laps.lap("clip map");
    report_load(0.85);
    // the static models' light grid samples trace through the clip map
    let clip = match &world {
        Some(w) => Some(w.clip.clone()),
        None => vcod_cod4::collision::ClipMap::from_assets(&walked.assets)
            .inspect_err(|e| log::warn!("cod4: clip map: {e:#}, grid corners untraced"))
            .ok()
            .map(Arc::new),
    };
    // each light def's attenuation image, by def name
    let light_def_images: HashMap<String, String> = walked
        .assets
        .light_defs
        .iter()
        .filter_map(|d| {
            let img = &walked.assets.images[d.image?.index()];
            Some((d.name.clone(), img.name.trim_start_matches(',').to_string()))
        })
        .collect();
    let mut src = Cod4Source::open(&install)?;
    src.add_zone_flagged(Arc::new(walked.assets), vcod_cod4::install::zone_flags::MAP);
    if let Some(z) = mod_zone {
        src.add_zone_flagged(Arc::new(z.assets), vcod_cod4::install::zone_flags::MOD);
    }
    // vcod draws no loading screen yet; the zone's assets still win.
    load_zone(&install, map, &mut src);
    laps.lap("load zone");
    let lightmaps = m
        .lightmaps
        .iter()
        .enumerate()
        .map(|(i, [p, s])| {
            let image = |n: &Option<String>| {
                let n = n
                    .as_deref()
                    .with_context(|| format!("lightmap {i} lacks an image"))?;
                src.image(n)
            };
            Ok(LightmapImages {
                primary: image(p)?,
                secondary: image(s)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    laps.lap("lightmap images");
    let (mut bsp, built) = to_mapworld(&m, &lightmaps)?;
    laps.lap("mapworld");
    let blocked = |a: [f32; 3], b: [f32; 3]| {
        clip.as_ref().is_some_and(|c| {
            let z = glam::Vec3::ZERO;
            c.trace(a.into(), b.into(), z, z, vcod_cod4::lightgrid::SIGHT_MASK)
                .fraction
                < 1.0
        })
    };
    bsp.model_lighting = vcod_cod4::mapworld::model_lighting(&m, &blocked);
    let entity_clip = clip.clone();
    bsp.entity_lighting = vcod_cod4::mapworld::entity_lighter(
        &m,
        Arc::new(move |a: [f32; 3], b: [f32; 3]| {
            entity_clip.as_ref().is_some_and(|c| {
                let z = glam::Vec3::ZERO;
                c.trace(a.into(), b.into(), z, z, vcod_cod4::lightgrid::SIGHT_MASK)
                    .fraction
                    < 1.0
            })
        }),
    );
    laps.lap("lighting");
    bsp.reflection_probes = reflection_probes(&src, &m);
    bsp.model_lights = vcod_cod4::mapworld::model_lights(&m, &|def| {
        let name = light_def_images.get(def)?;
        attenuation_row(&src, name)
            .inspect_err(|e| log::warn!("cod4: light def {def}'s attenuation {name}: {e:#}"))
            .ok()
    });
    laps.lap("probes, lights");
    laps.finish();
    log::info!(
        "{map}: {} soups ({} static model), {} lightmap pages, {} cells",
        bsp.soups.len(),
        built.static_soups,
        built.tiles,
        bsp.cells.len()
    );
    let spawn = m
        .entities
        .iter()
        .find(|e| {
            e.iter()
                .any(|(k, v)| k == "classname" && v == "mp_tdm_spawn")
        })
        .and_then(|e| {
            let get = |key: &str| e.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
            let origin = vcod_common::bsp::parse_vec3(get("origin")?)?;
            let yaw = get("angles")
                .and_then(|a| a.split_whitespace().nth(1)?.parse().ok())
                .unwrap_or(0.0);
            Some((origin, yaw))
        })
        .unwrap_or_else(|| ([0, 1, 2].map(|k| (m.mins[k] + m.maxs[k]) * 0.5), 0.0));
    Ok(Cod4Map {
        bsp,
        materials: vcod_cod4::materials::material_lib(&m),
        spawn,
        models,
        parts,
        weapons,
        bodies,
        av,
        ambient,
        sound_files,
        world,
        look,
        hud,
    })
}

/// [`vcod_cod4::hud::HudAssets::load`]'s assets off zones walked already:
/// `stock` by name in its `HUD_ZONES` order, the map's, then the mod's.
/// The HUD reads no bulk data, so zones walked keeping it give the same
/// (`hud_assets_match_their_own_walk`).
fn hud_assets<'a>(
    stock: impl Iterator<Item = (&'a str, &'a Assets)>,
    map: &Assets,
    mod_zone: Option<&Assets>,
) -> vcod_cod4::hud::HudAssets {
    let stock: HashMap<&str, &Assets> = stock.collect();
    let mut out = vcod_cod4::hud::HudAssets::default();
    for name in vcod_cod4::hud::HUD_ZONES {
        match stock.get(name) {
            Some(a) => out.add(a),
            None => log::warn!("cod4 hud: {name} was not walked"),
        }
    }
    out.add(map);
    if let Some(m) = mod_zone {
        out.add(m);
    }
    out
}

/// [`load_from`] with the level's HUD zones off its walks
/// (`Cod4Hud::read_level`), as `Cod4Client::prewarm` would read them
/// after it.
pub fn load_with_hud(
    game_dir: &Path,
    fs_game: Option<&str>,
    map: &str,
    ents: &EntityAssets,
    stock: Option<StockZones>,
) -> Result<(Cod4Map, crate::cod4_hud::LevelZones)> {
    let mut m = load_from(game_dir, fs_game, map, ents, stock)?;
    let t0 = std::time::Instant::now();
    let z =
        crate::cod4_hud::Cod4Hud::read_level(&game_dir.join("main"), map, fs_game, m.hud.take());
    vcod_common::laps::span(t0, || "cod4 hud".into());
    Ok((m, z))
}

/// What [`load`] tells of its progress.
type LoadReport = Box<dyn Fn(f32)>;

thread_local! {
    static LOAD_REPORT: std::cell::RefCell<Option<LoadReport>> =
        const { std::cell::RefCell::new(None) };
}

/// What this thread's [`load`] calls with the fraction it has loaded
/// (the loading screen's bar, `cod4_loading`).
pub fn set_load_report(f: Option<LoadReport>) {
    LOAD_REPORT.with(|r| *r.borrow_mut() = f);
}

fn report_load(f: f32) {
    LOAD_REPORT.with(|r| {
        if let Some(r) = &*r.borrow() {
            r(f);
        }
    });
}

/// `<map>_load.ff`'s `$levelbriefing` image name, the loading screen's
/// levelshot (`loadscreen_<map>` on the stock maps), without walking it
/// into a source.
pub fn levelshot(game_dir: &Path, map: &str) -> Option<String> {
    let install = Install::at(game_dir)?;
    let f = install.zone_file("english", &format!("{map}_load"))?;
    let walked = Zone::open_path(&f.path)
        .and_then(|z| walk(z, WalkOptions::default()))
        .ok()?;
    let a = &walked.assets;
    let m = a
        .materials
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case("$levelbriefing"))?;
    vcod_cod4::hud::colour_image(a, m)
}

/// The load screen load: `<map>_load.ff` (stock, or a usermap's) walked into
/// `src` with the load flags, over every other zone (cod4-fastfile.md 13).
/// Returns its `$levelbriefing` loading screen's image name once that
/// decodes; `None` when the map has no such zone or the image does not
/// decode.
fn load_zone(install: &Install, map: &str, src: &mut Cod4Source) -> Option<String> {
    let f = install.zone_file("english", &format!("{map}_load"))?;
    let walked = Zone::open_path(&f.path)
        .and_then(|z| walk(z, WalkOptions::default()))
        .inspect_err(|e| log::warn!("cod4: {}: {e:#}", f.path.display()))
        .ok()?;
    src.add_zone_flagged(
        Arc::new(walked.assets),
        vcod_cod4::install::zone_flags::LOAD,
    );
    let name = src.material_image("$levelbriefing")?;
    let img = src
        .image(&name)
        .inspect_err(|e| log::warn!("cod4: {map}'s loading screen {name}: {e:#}"))
        .ok()?;
    log::info!(
        "cod4: {map}_load: loading screen {name} {}x{}",
        img.width,
        img.height
    );
    Some(name)
}

/// A light def's attenuation image's first row, RGBA (cod4-world.md 3.5).
fn attenuation_row(src: &Cod4Source, name: &str) -> Result<Vec<[u8; 4]>> {
    let img = src.image(name)?;
    let vcod_common::assets::ImageData::Rgba8(px) = img.data else {
        anyhow::bail!("a compressed attenuation image");
    };
    Ok(px
        .as_chunks::<4>()
        .0
        .iter()
        .take(img.width as usize)
        .copied()
        .collect())
}

/// The map's reflection probes as cubes, in its probe order; one that
/// does not decode reads black (`cod4-world.md` 9.7).
fn reflection_probes(src: &Cod4Source, m: &Map) -> Vec<vcod_common::mapworld::ProbeCube> {
    m.reflection_probes
        .iter()
        .map(|name| {
            let faces = name
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("a probe without an image"))
                .and_then(|n| src.zone_cube(n));
            match faces {
                Ok(faces) => vcod_common::mapworld::ProbeCube {
                    size: faces[0][0].width,
                    faces: faces
                        .into_iter()
                        .map(|f| {
                            f.into_iter()
                                .map(|l| match l.data {
                                    vcod_common::assets::ImageData::Rgba8(px) => px,
                                    _ => unreachable!("decode_zone_cube decodes RGBA"),
                                })
                                .collect()
                        })
                        .collect(),
                },
                Err(e) => {
                    log::warn!("cod4: reflection probe {name:?}: {e:#}");
                    vcod_common::mapworld::ProbeCube::default()
                }
            }
        })
        .collect()
}

/// The map script's `ambientPlay` alias (`maps/mp/<map>.gsc`, a rawfile
/// of the map's zone: mp_crash's `ambient_crash`).
fn map_ambient(a: &Assets, map: &str) -> Option<String> {
    a.raw_files
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(&format!("maps/mp/{map}.gsc")))
        .and_then(|r| crate::play::cod2_sound::script_ambient(&String::from_utf8_lossy(&r.data)))
}

/// The level time fly mode starts the createfx ambience at: a level a
/// minute old, so the smoke and dust stand as a joining client sees them
/// rather than growing from nothing.
const FLY_LEVEL_MS: i32 = 60_000;

/// Fly mode's sounds and effects, a session's without a server: the map's
/// createfx effects and their sounds start on the first frame, the looping
/// elements run, and the map script's `ambientPlay` alias plays
/// (cod4-effects.md 11, cod4-sound.md).
pub struct FlyAv {
    sounds: Cod4Sounds,
    effects: Cod4Effects,
    ambient: Option<String>,
    started: bool,
    /// The map's vision set, which the frame hands the renderer.
    pub look: crate::cod4_look::Cod4Look,
}

impl FlyAv {
    pub fn new(
        (sounds, effects): Av,
        ambient: Option<String>,
        look: crate::cod4_look::Cod4Look,
    ) -> FlyAv {
        FlyAv {
            sounds,
            effects,
            ambient,
            started: false,
            look,
        }
    }

    /// One frame at fx clock `now` (seconds): the first hands the alias
    /// table and the effects to their systems and starts the ambience,
    /// settled as at [`FLY_LEVEL_MS`].
    pub fn frame(
        &mut self,
        fs: &Pk3Fs,
        audio: &mut crate::audio::AudioSystem,
        fx: &mut crate::fx::sim::FxSystem,
        now: f64,
    ) {
        if !self.started {
            self.started = true;
            if let Some(t) = self.sounds.take_table() {
                audio.set_cod4_aliases(t);
            }
            if let Some(all) = self.effects.take_effects() {
                for (k, v) in all {
                    fx.insert(&k, v);
                }
            }
            audio.set_ambient(fs, self.ambient.as_deref());
        }
        let mut sounds = self.effects.start_ambience(fx, fs, (FLY_LEVEL_MS, now));
        sounds.extend(self.effects.run_loops(fx, fs, now));
        audio.play_fx(fs, sounds);
        let loops = crate::cod4_audio::loop_sounds(
            &[],
            &|_| String::new(),
            &self.effects.client_sounds,
            &crate::play::cod4::origin_of,
        );
        audio.set_loop_aliases(fs, &loops);
    }
}

/// The zones a 1.7 client loads ahead of an MP map that carry sound
/// (`channels.def`, the shared aliases and their loaded sounds;
/// cod4-handshake.md 1).
const SOUND_ZONES: [&str; 3] = [
    "code_post_gfx_mp",
    "localized_code_post_gfx_mp",
    "localized_common_mp",
];

/// The front end's sound zones: the code zones, whose aliases hold
/// `mouse_click`, `mouse_over` and `music_mainmenu_mp`, then `ui_mp`
/// (cod4-front-end.md 8).
const FRONT_SOUND_ZONES: [&str; 3] = ["code_post_gfx_mp", "localized_code_post_gfx_mp", "ui_mp"];

/// The front end's sound aliases and loaded sounds, under `fs_game` its
/// `mod.ff` last, as the menus' `play` and `soundName` read them.
pub(crate) fn front_sounds(
    root: &Path,
    fs_game: Option<&str>,
) -> Result<vcod_cod4::sound::SoundSet> {
    let install = Install::at(root)
        .with_context(|| format!("{} is no CoD4 install", root.display()))?
        .with_fs_game(fs_game);
    let mut set = vcod_cod4::sound::SoundSet::default();
    let mod_zone = install.mod_zone().map(|_| "mod");
    for name in FRONT_SOUND_ZONES.into_iter().chain(mod_zone) {
        let path = install.zone_path("english", name);
        let z = Zone::open_path(&path)
            .and_then(|z| walk(z, WalkOptions { keep_bulk: true }))
            .with_context(|| path.display().to_string())?;
        set.add(&z.assets);
    }
    Ok(set)
}

/// `cod2_tracer::COD4`'s material.
const TRACER_MATERIAL: &str = crate::fx::cod2_tracer::COD4.material;

/// A watched session's sounds and effects out of `zones` (the map's and
/// `common_mp`) and [`SOUND_ZONES`]: the effects' materials join the
/// map's, the loaded sounds come back as overlay files.
fn sound_and_fx(
    m: &mut Map,
    zones: &[&Assets],
    map: &str,
    sound_zones: &[vcod_cod4::walk::Walked],
) -> Result<(Av, OverlayFiles)> {
    let mut set = vcod_cod4::sound::SoundSet::default();
    for z in sound_zones {
        set.add(&z.assets);
        // The tracer's beam (`CG_RegisterGraphics`' `gfx_tracer`,
        // cod4-effects.md 12) lives in `localized_common_mp`.
        if let Some(i) = z
            .assets
            .materials
            .iter()
            .position(|mt| mt.name == TRACER_MATERIAL)
        {
            m.add_material(&z.assets, vcod_cod4::assets::Id::new(i));
        }
    }
    // The map's own lists last: a later zone's list replaces an earlier's.
    for a in zones.iter().rev() {
        set.add(a);
    }
    let files = crate::cod4_audio::overlay_files(&set);
    let weapons = Arc::new(crate::cod4_audio::weapons_from(zones));
    let sounds = Cod4Sounds::new(&set, weapons.clone());
    let effects = Cod4Effects::new(zones, weapons, map);
    // Every effect's materials, whole in a zone, for the fx draw.
    for a in zones {
        for fx in a.fx.iter().filter(|f| !f.name.starts_with(',')) {
            for e in &fx.elems {
                for v in &e.visuals {
                    use vcod_cod4::assets::fx::FxVisual;
                    if let FxVisual::Material(Some(id)) | FxVisual::Mark([Some(id), _]) = v
                        && !a.materials[id.index()].name.starts_with(',')
                    {
                        m.add_material(a, *id);
                    }
                }
            }
        }
    }
    log::info!(
        "cod4: {} sound aliases, {} loaded sounds",
        set.aliases.len(),
        files.len()
    );
    Ok(((sounds, effects), files))
}

/// [`load_bodies`]' entity models, weapons and bodies.
#[cfg(test)]
pub(crate) type Bodies = (
    HashMap<String, vcod_common::model::Model>,
    HashMap<String, WeaponModels>,
    vcod_cod4::anim::BodySource,
);

/// A watched session's bodies without the world: the map's zone and
/// `common_mp`'s entity models, weapons and [`vcod_cod4::anim::BodySource`].
/// The replay gate's load; [`load`] builds the same beside the world.
#[cfg(test)]
pub fn load_bodies(game_dir: &Path, map: &str, ents: &EntityAssets) -> Result<Bodies> {
    let install = Install::at(game_dir)
        .with_context(|| format!("{} has no zone/ directory", game_dir.display()))?;
    let (mut m, walked) = Map::load(&install, "english", map)?;
    let path = install.zone("english", "common_mp");
    let common = Zone::open_path(&path)
        .and_then(|z| walk(z, WalkOptions { keep_bulk: true }))
        .with_context(|| path.display().to_string())?;
    let zones = [&walked.assets, &common.assets];
    let (models, weapons, _) = entity_models(&mut m, &zones, ents);
    let src = vcod_cod4::anim::BodySource::from_zones(&zones, models.clone())?;
    Ok((models, weapons, src))
}

/// A watched session's sounds and effects without the world, as [`load`]
/// builds them beside it. The replay gate's load.
#[cfg(test)]
pub fn load_av(game_dir: &Path, map: &str) -> Result<(Av, OverlayFiles)> {
    let install = Install::at(game_dir)
        .with_context(|| format!("{} has no zone/ directory", game_dir.display()))?;
    let (mut m, walked) = Map::load(&install, "english", map)?;
    let StockZones { common, sound } = walk_stock(game_dir)?;
    sound_and_fx(&mut m, &[&walked.assets, &common.assets], map, &sound)
}

/// Fly mode's sounds and effects without the world, as [`load`] builds
/// them. The gate's load.
#[cfg(test)]
pub fn load_fly_av(game_dir: &Path, map: &str) -> Result<(FlyAv, OverlayFiles)> {
    let install = Install::at(game_dir)
        .with_context(|| format!("{} has no zone/ directory", game_dir.display()))?;
    let (mut m, walked) = Map::load(&install, "english", map)?;
    let StockZones { common, sound } = walk_stock(game_dir)?;
    let (av, files) = sound_and_fx(&mut m, &[&walked.assets, &common.assets], map, &sound)?;
    let look = crate::cod4_look::Cod4Look::default();
    Ok((
        FlyAv::new(av, map_ambient(&walked.assets, map), look),
        files,
    ))
}

/// A watched session's prediction world without the rest: the map's clip
/// map and the weapons of its zone and `common_mp`. The replay gate's load.
#[cfg(test)]
pub fn load_world(game_dir: &Path, map: &str) -> Result<crate::play::cod4_predict::Cod4World> {
    let install = Install::at(game_dir)
        .with_context(|| format!("{} has no zone/ directory", game_dir.display()))?;
    let (_, walked) = Map::load(&install, "english", map)?;
    let path = install.zone("english", "common_mp");
    let common = Zone::open_path(&path)
        .and_then(|z| walk(z, WalkOptions { keep_bulk: false }))
        .with_context(|| path.display().to_string())?;
    let mut w = crate::play::cod4_predict::Cod4World::from_assets(
        &walked.assets,
        &[&walked.assets, &common.assets],
    )?;
    if let Ok(fs) = open_fs(game_dir, None, Some(map)) {
        w.read_ragdoll(&fs);
    }
    Ok(w)
}

/// `ents`' models and weapons out of `zones` (the first holding one
/// whole, not as a stub, wins), and the entity models' surfaces'
/// `partBits`; each model's materials join the map's.
fn entity_models(
    m: &mut Map,
    zones: &[&Assets],
    ents: &EntityAssets,
) -> (
    HashMap<String, vcod_common::model::Model>,
    HashMap<String, WeaponModels>,
    Parts,
) {
    let mut models: HashMap<String, vcod_common::model::Model> = HashMap::new();
    let mut parts = Parts::new();
    let mut weapons = HashMap::new();
    let mut clips = HashMap::new();
    let model_name = |a: &Assets, id: vcod_cod4::assets::Id<_>| {
        let x: &vcod_cod4::assets::model::XModel = &a.xmodels[id.index()];
        stub_name(&x.name).to_string()
    };
    for w in &ents.weapons {
        let Some((a, def)) = zones
            .iter()
            .find_map(|a| Some((*a, a.weapons.iter().find(|d| &d.name == w)?)))
        else {
            continue;
        };
        let world = def
            .model_lists
            .get("worldModel")
            .and_then(|l| l.first().copied().flatten())
            .map(|id| model_name(a, id));
        let projectile = def
            .models
            .get("projectileModel")
            .map(|&id| model_name(a, id));
        let view = view_def(a, def, zones, &mut clips);
        let gun_anims = (i32_at(&def.raw, WEAP_CLASS) == WEAPCLASS_TURRET)
            .then(|| {
                let mut anim = |n: usize| {
                    let name = def.xanims.get(n).cloned().flatten()?;
                    decode_clip(zones, &mut clips, &name)
                };
                Some([anim(1)?, anim(3)?])
            })
            .flatten();
        weapons.insert(
            w.clone(),
            WeaponModels {
                world: world.clone(),
                projectile: projectile.clone(),
                view,
                rotate_inc: float(&def.raw, 2004),
                view_jitter: [float(&def.raw, 2032), float(&def.raw, 2028)],
                gun_anims,
            },
        );
    }
    let wanted = ents.models.iter().cloned().chain(
        weapons
            .values()
            .flat_map(|w| {
                let (hands, knife) = w
                    .view
                    .as_ref()
                    .map_or((None, None), |v| (v.hands.clone(), v.knife.clone()));
                [w.world.clone(), w.projectile.clone(), hands, knife]
            })
            .flatten(),
    );
    // Each view gun under its `hideTags`, keyed by `ViewDef::gun`.
    for v in weapons.values().filter_map(|w| w.view.as_ref()) {
        if models.contains_key(&v.gun) {
            continue;
        }
        let found = zones.iter().find_map(|a| {
            let x = a.xmodels.iter().find(|x| x.name == v.gun_model)?;
            Some((*a, x))
        });
        let Some((a, x)) = found else {
            log::debug!("cod4: no zone holds xmodel {}", v.gun_model);
            continue;
        };
        let hide = vcod_cod4::anim::hide_part_bits(a, x, &v.hide_tags);
        let skinned = vcod_cod4::anim::model_hiding(a, x, hide, |id| {
            let i = add_whole_material(m, zones, a, id) as usize;
            m.materials[i].name.clone()
        });
        match skinned {
            Ok(Some(model)) => {
                models.insert(v.gun.clone(), model);
            }
            Ok(None) => {}
            Err(e) => log::warn!("cod4: xmodel {}: {e:#}", v.gun_model),
        }
    }
    for name in wanted {
        if name.is_empty() || models.contains_key(&name) {
            continue;
        }
        let found = zones.iter().find_map(|a| {
            let x = a.xmodels.iter().find(|x| x.name == name)?;
            Some((*a, x))
        });
        let Some((a, x)) = found else {
            log::debug!("cod4: no zone holds xmodel {name}");
            continue;
        };
        let skinned = vcod_cod4::anim::model_parts(a, x, [0; 4], |id| {
            let i = add_whole_material(m, zones, a, id) as usize;
            m.materials[i].name.clone()
        });
        match skinned {
            Ok(Some((model, bits))) => {
                if bits.iter().any(|b| *b != [0; 4]) {
                    parts.insert(name.clone(), bits);
                }
                models.insert(name, model);
            }
            Ok(None) => {}
            Err(e) => log::warn!("cod4: xmodel {name}: {e:#}"),
        }
    }
    log::info!(
        "cod4: {} entity models, {} weapons",
        models.len(),
        weapons.len()
    );
    (models, weapons, parts)
}

/// Zone `a`'s material `id` in `m`, taken whole from the first of `zones`
/// that holds it when `a` has only its stub: the map zone's own
/// `weapon_saw_mg_setup` names `,mc/mtl_weapon_saw`, whose techset and
/// images only `common_mp` carries, and the stub drew black.
fn add_whole_material(
    m: &mut Map,
    zones: &[&Assets],
    a: &Assets,
    id: vcod_cod4::assets::Id<vcod_cod4::assets::material::Material>,
) -> u32 {
    if let Some(name) = a.materials[id.index()].name.strip_prefix(',') {
        for z in zones {
            if let Some(i) = z.materials.iter().position(|x| x.name == name) {
                return m.add_material(z, vcod_cod4::assets::Id::new(i));
            }
        }
    }
    m.add_material(a, id)
}

/// A clip decoded once per name (lowercase), `None` where no zone holds
/// it whole or it does not decode.
type Clips = HashMap<String, Option<Arc<vcod_common::xanim::XAnim>>>;

/// Clip `n` out of the first of `zones` holding it whole, decoded once.
fn decode_clip(
    zones: &[&Assets],
    clips: &mut Clips,
    n: &str,
) -> Option<Arc<vcod_common::xanim::XAnim>> {
    clips
        .entry(n.to_ascii_lowercase())
        .or_insert_with(|| {
            let found = zones.iter().find_map(|z| {
                let x = z.xanims.iter().find(|x| x.name.eq_ignore_ascii_case(n))?;
                Some((*z, x))
            });
            let (z, x) = found?;
            vcod_cod4::anim::clip(z, x)
                .inspect_err(|e| log::warn!("cod4 weapon anim {n}: {e:#}"))
                .ok()
                .map(Arc::new)
        })
        .clone()
}

/// `def`'s view weapon out of `a` (the zone holding the weapon): its
/// `gunModel[0]` and `handModel` names and its 33 view clips out of
/// `zones` (`CG_RegisterWeapon`, `CG_CreateWeaponViewModelXAnim`); `None`
/// without a gun, hands or idle.
fn view_def(
    a: &Assets,
    def: &vcod_cod4::assets::weapon::Weapon,
    zones: &[&Assets],
    clips: &mut Clips,
) -> Option<Arc<crate::cod4_viewmodel::ViewDef>> {
    let name = |id: vcod_cod4::assets::Id<vcod_cod4::assets::model::XModel>| {
        stub_name(&a.xmodels[id.index()].name).to_string()
    };
    let gun = name(
        def.model_lists
            .get("gunModel")?
            .first()
            .copied()
            .flatten()?,
    );
    let hands = def.models.get("handModel").map(|&id| name(id));
    let knife = def.models.get("knifeModel").map(|&id| name(id));
    let hide_tags = def.hide_tags(a);
    let gun_model = gun.clone();
    // A gun that hides parts is its own model in the session's map.
    let gun = match hide_tags.as_slice() {
        [] => gun,
        t => format!("{gun}#{}", t.join(",")),
    };
    let anim_name = |n: usize| {
        def.xanims
            .get(n)
            .cloned()
            .flatten()
            .filter(|s| !s.is_empty())
    };
    let idle = anim_name(1)?;
    let mut clip = |n: &str| decode_clip(zones, clips, n);
    let anims = (0..33)
        .map(|n| match n {
            0 => None,
            _ => clip(&anim_name(n).unwrap_or_else(|| idle.clone())),
        })
        .collect();
    Some(Arc::new(crate::cod4_viewmodel::ViewDef {
        gun,
        gun_model,
        hide_tags,
        hands,
        knife,
        anims,
        clip_name: def.strings.get("clipName").cloned().unwrap_or_default(),
        numbers: crate::cod4_viewmodel::ViewNumbers::from_raw(&def.raw),
        note_sounds: note_sounds(a, &def.raw),
    }))
}

/// `notetrackSoundMap`: 16 key and 16 value script strings at
/// `WeaponDef` +232 and +264 (the notetrack sound step, `iw3mp.exe`
/// 0x454a70), up to the first empty key; a pair with no value is left out.
fn note_sounds(a: &Assets, raw: &[u8]) -> Vec<(String, String)> {
    let at = |o: usize| {
        raw.get(o..o + 2)
            .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    };
    (0..16)
        .map(|k| (at(232 + 2 * k), at(264 + 2 * k)))
        .take_while(|&(key, _)| key != 0)
        .filter(|&(_, value)| value != 0)
        .map(|(key, value)| {
            let s = |i| vcod_cod4::anim::script_string(a, i).to_string();
            (s(key).to_ascii_lowercase(), s(value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// A session's load builds the HUD's assets off the zones it walked
    /// for the map (keeping bulk data) as the HUD's own walk of them
    /// (dropping it) reads them. Vacuous without `COD4_DIR`.
    #[test]
    fn hud_assets_match_their_own_walk() {
        use std::collections::HashMap;
        use std::fmt::Debug;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let ents = super::EntityAssets {
            models: vec!["body_mp_usmc_cqb".into()],
            weapons: Vec::new(),
        };
        let m = super::load(install.root(), None, "mp_crash", &ents).unwrap();
        let shared = m.hud.expect("a session's load builds the HUD's assets");
        let own = vcod_cod4::hud::HudAssets::load(&install, "mp_crash").unwrap();
        fn sorted<K: Debug, V: Debug>(m: &HashMap<K, V>) -> Vec<String> {
            let mut v: Vec<String> = m.iter().map(|kv| format!("{kv:?}")).collect();
            v.sort();
            v
        }
        let keys = |m: &HashMap<String, _>| {
            let mut k: Vec<String> = m.keys().cloned().collect();
            k.sort();
            k
        };
        assert!(!own.images.is_empty() && !own.localize.is_empty());
        assert_eq!(shared.images, own.images);
        assert_eq!(shared.localize, own.localize);
        assert_eq!(shared.tables, own.tables);
        assert_eq!(shared.gametypes, own.gametypes);
        assert_eq!(shared.gametype_order, own.gametype_order);
        assert_eq!(sorted(&shared.kill_icons), sorted(&own.kill_icons));
        assert_eq!(sorted(&shared.windows), sorted(&own.windows));
        assert_eq!(sorted(&shared.compass), sorted(&own.compass));
        assert_eq!(sorted(&shared.rank_icons), sorted(&own.rank_icons));
        assert_eq!(sorted(&shared.rank_levels), sorted(&own.rank_levels));
        assert_eq!(sorted(&shared.weapon_hud), sorted(&own.weapon_hud));
        assert_eq!(sorted(&shared.weapon_info), sorted(&own.weapon_info));
        assert_eq!(keys(&shared.script_menus), keys(&own.script_menus));
        for (a, b) in [
            (format!("{:?}", shared.fonts), format!("{:?}", own.fonts)),
            (format!("{:?}", shared.stance), format!("{:?}", own.stance)),
            (
                format!("{:?}", shared.killcam),
                format!("{:?}", own.killcam),
            ),
            (format!("{:?}", shared.arenas), format!("{:?}", own.arenas)),
        ] {
            assert_eq!(a, b);
        }
    }

    /// Fly mode under `fs_game mods/ModWarfare`, the install's stock
    /// example mod: the map loads with `mod.ff` beside it and its
    /// `_load` zone's loading screen, and the search path reads the mod's
    /// iwd over `main/`'s (cod4-fastfile.md 13).
    /// Vacuous without `COD4_DIR` or the mod.
    #[test]
    fn cod4_fly_loads_under_a_mod() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let g = Some("mods/ModWarfare");
        let Some(dir) = install.clone().with_fs_game(g).game_dir() else {
            return;
        };
        let fs = super::open_fs(install.root(), g, None).unwrap();
        let stock = super::open_fs(install.root(), None, None).unwrap();
        let mod_iwd = vcod_common::pk3::Pk3Fs::open_iwd(&dir, None).unwrap();
        let logo = "images/logo_cod2.iwi";
        assert_eq!(fs.read(logo), mod_iwd.read(logo));
        assert_ne!(stock.read(logo), mod_iwd.read(logo));
        let m = super::load(install.root(), g, "mp_crash", &Default::default()).unwrap();
        assert!(!m.bsp.soups.is_empty());
        // `mp_crash_load.ff`'s `$levelbriefing` decodes off the iwds.
        let i = install.with_fs_game(g);
        let mut src = vcod_cod4::source::Cod4Source::open(&i).unwrap();
        let shot = super::load_zone(&i, "mp_crash", &mut src);
        assert_eq!(shot.as_deref(), Some("loadscreen_mp_crash"));
    }

    /// Fly mode over mp_crash plays what a session on it does (C4-19's
    /// createfx ambience, cod4-effects.md 11): the map script's
    /// `ambient_crash` reaches the mixer, and the 36 createfx effects
    /// stand settled on the first frames and keep running. Vacuous
    /// without `COD4_DIR`.
    #[test]
    fn cod4_fly_mode_runs_the_maps_effects_and_ambient() {
        use crate::audio::{AudioOpts, AudioSystem};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let (mut av, files) = super::load_fly_av(install.root(), "mp_crash").unwrap();
        assert_eq!(av.ambient.as_deref(), Some("ambient_crash"));
        let mut fs = super::open_fs(install.root(), None, None).unwrap();
        for (p, w) in files {
            fs.overlay(&p, w);
        }
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 44100,
            },
        );
        a.on_gamestate("mp_crash");
        let mut fx = crate::fx::sim::FxSystem::new();
        let mut now = 100.0f64;
        let mut heard = std::collections::HashSet::new();
        let mut parts = Vec::new();
        for i in 0..180 {
            av.frame(&fs, &mut a, &mut fx, now);
            fx.step(1.0 / 60.0, now as f32, None);
            for (row, gain) in a.mix() {
                if gain > 0.01 {
                    heard.insert(row.name.clone());
                }
            }
            a.step(&Default::default(), &Default::default(), None);
            if i % 60 == 0 {
                parts.push(fx.counts().0);
            }
            now += 1.0 / 60.0;
        }
        println!("heard {heard:?}, particles {parts:?}");
        assert!(heard.contains("ambient_crash"), "{heard:?}");
        assert!(av.effects.loopers() > 50, "{}", av.effects.loopers());
        assert!(parts.iter().all(|&p| p > 300), "{parts:?}");
    }
}
