//! CoD4 1.7 sound in the client (docs/research/cod4-sound.md): the
//! zones' aliases as the [`AliasTable`] the audio system plays from,
//! their loaded sounds as `.wav` files on the search path's overlay, and
//! `CG_EntityEvent`'s sound arms over the events a watched session fires
//! (`play::cod4::Fired`).

use glam::Vec3;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use vcod_cod4::assets::Assets;
use vcod_cod4::map::stub_name;
use vcod_cod4::sound::{Alias, ChannelDef, SURFACES, SoundSet};

use crate::audio::alias::{AliasRow, AliasTable, Channel, MasterSlave};
use crate::audio::cod2::{PROBABILITY_SCALE, VfCurve};
use crate::audio::cues::{Cue, Source};
use crate::play::cod4::{Fired, ent_u32};

/// `ENTITYNUM_WORLD`, which retail plays positional one-shots on.
const ENTITYNUM_WORLD: u32 = 1022;
/// `CS_SOUNDALIASES`: an entity's `loopSound` and `EV_SOUND_ALIAS`'s parm
/// index the alias names from here (cod4-sound.md 4).
pub const CS_SOUND_ALIASES: u16 = 1342;

/// The weapon fields the event arms read, by weapon name.
#[derive(Clone, Debug, Default)]
pub struct WeaponAv {
    /// `SndAliasCustom` slots by field name (`fireSound`).
    pub sounds: HashMap<&'static str, String>,
    /// `bounceSound`, per surface type.
    pub bounce: Vec<Option<String>>,
    /// `impactType` (`WeaponDef` +312): 1 small bullet, 2 large, 3 AP,
    /// 4 shotgun, 5 grenade bounce, 6 grenade explode, 7 rocket explode,
    /// 8 dud.
    pub impact_type: u32,
    /// `FxEffectDef` slots by field name (`worldFlashEffect`).
    pub effects: HashMap<&'static str, String>,
    /// `clipOnly` (+1280) or `cancelAutoHolsterWhenEmpty` (+1288): EV_NOAMMO
    /// plays no dry fire (`iw3mp.exe` 0x43633b).
    pub quiet_empty: bool,
    /// The depth a round passes per surface type: `penetrateType`'s
    /// (`WeaponDef` +308) row of `info/bullet_penetration_mp`, all 0 for
    /// none (the penetration walk, `iw3mp.exe` 0x455c1c..0x455c65).
    pub penetration: [f32; 29],
    /// `explosionRadius` (`WeaponDef` +1364): an explode event's ragdoll
    /// push reaches this far (`iw3mp.exe` 0x436b2e, cod4-player-anims.md
    /// 10.1).
    pub explosion_radius: f32,
}

impl WeaponAv {
    fn sound(&self, field: &str) -> Option<&str> {
        self.sounds
            .get(field)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }
}

/// `WeaponDef.impactType`'s offset (cod4-sound.md 4).
const IMPACT_TYPE: usize = 312;

/// `WeaponDef.explosionRadius`'s offset (`weapon_file`'s field table).
const EXPLOSION_RADIUS: usize = 1364;

/// `WeaponDef.penetrateType`'s offset (`weapon_file`'s field table).
const PENETRATE_TYPE: usize = 308;

/// `BG_ParsePenetrationDepthTable` over `info/bullet_penetration_mp`'s
/// `BULLET_PEN_TABLE\small_bark\20\...`: a row per `penetrateType` (none,
/// small, medium, large) of depths per surface type, row 0 and surface 0
/// left 0. `None` off the wrong header.
pub fn penetration_table(text: &str) -> Option<[[f32; 29]; 4]> {
    let mut parts = text.split('\\');
    if parts.next()?.trim() != "BULLET_PEN_TABLE" {
        return None;
    }
    let mut keys = HashMap::new();
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        keys.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
    }
    let mut t = [[0.0; 29]; 4];
    for (row, kind) in ["small", "medium", "large"].iter().enumerate() {
        for (surf, name) in SURFACES.iter().enumerate().skip(1) {
            if let Some(v) = keys.get(&format!("{kind}_{name}")) {
                t[row + 1][surf] = v.parse().unwrap_or(0.0);
            }
        }
    }
    Some(t)
}

/// The weapons `zones` define, the first zone holding one winning.
pub fn weapons_from(zones: &[&Assets]) -> HashMap<String, WeaponAv> {
    let table = zones
        .iter()
        .flat_map(|a| &a.raw_files)
        .find(|r| r.name.eq_ignore_ascii_case("info/bullet_penetration_mp"))
        .and_then(|r| penetration_table(&String::from_utf8_lossy(&r.data)))
        .unwrap_or_default();
    let mut out = HashMap::new();
    for a in zones {
        for w in &a.weapons {
            if w.name.starts_with(',') || out.contains_key(&w.name) {
                continue;
            }
            let effects = w
                .effects
                .iter()
                .map(|(k, id)| (*k, stub_name(&a.fx[id.index()].name).to_string()))
                .collect();
            let impact_type = w
                .raw
                .get(IMPACT_TYPE..IMPACT_TYPE + 4)
                .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
            out.insert(
                w.name.clone(),
                WeaponAv {
                    sounds: w.sounds.clone(),
                    bounce: w.bounce_sounds.clone(),
                    impact_type,
                    effects,
                    quiet_empty: [1280, 1288]
                        .iter()
                        .any(|&o| w.raw.get(o..o + 4).is_some_and(|b| b != [0; 4])),
                    penetration: w
                        .raw
                        .get(PENETRATE_TYPE..PENETRATE_TYPE + 4)
                        .and_then(|b| table.get(u32::from_le_bytes(b.try_into().unwrap()) as usize))
                        .copied()
                        .unwrap_or_default(),
                    explosion_radius: w
                        .raw
                        .get(EXPLOSION_RADIUS..EXPLOSION_RADIUS + 4)
                        .map_or(0.0, |b| i32::from_le_bytes(b.try_into().unwrap()) as f32),
                },
            );
        }
    }
    out
}

/// CoD4's channel on vcod's: what is 2D plays flat, the rest by its kind
/// (`channels.def`'s `is3d` column; the restricted voice rules ride the
/// kind).
fn channel(c: Option<&ChannelDef>) -> Channel {
    let Some(c) = c else {
        return Channel::Auto;
    };
    match (c.is3d, c.name.as_str()) {
        (false, "menu") => Channel::Menu,
        (false, "music" | "musicnopause") => Channel::Music,
        (false, "announcer") => Channel::Announcer,
        (false, "shellshock") => Channel::Shellshock,
        // `auto2d` and kin stack on one entity (cod4-sound.md 3): the
        // promotion's `mp_level_up` plays on under the next kill's
        // `MP_hit_alert`.
        (false, _) if !c.restricted => Channel::Auto2d,
        (false, _) => Channel::Local,
        (true, "weapon") => Channel::Weapon,
        (true, "voice") => Channel::Voice,
        (true, "item") => Channel::Item,
        (true, "body" | "reload") => Channel::Body,
        (true, _) => Channel::Auto,
    }
}

/// A 2D row's level on a stereo output off its speaker map
/// (`SND_ApplyChannelMap`, cod4-sound.md 2): the map's stereo
/// source row for a stereo output, each front speaker at the larger of its
/// two levels, against the default map's 1. Every stock map's mono row is
/// the default's 0.5 a side, so the stereo row is the one that differs:
/// `amb_rear` (the ambients' `_sur` secondaries) plays at half. The LFE
/// and centre sends reach a multichannel output only.
pub fn stereo_speaker_gain(map: Option<&vcod_cod4::assets::sound::SpeakerMap>) -> f32 {
    let Some(m) = map else {
        return 1.0;
    };
    let rows = &m.maps[1][0];
    let level = |speaker: u32| {
        rows.iter()
            .find(|s| s.speaker == speaker)
            .map_or(1.0, |s| s.levels[0].max(s.levels[1]))
    };
    (level(0) + level(1)) / 2.0
}

fn row(a: &Alias, channels: &[ChannelDef]) -> AliasRow {
    let (dist_min, dist_max) = a.dist;
    let c = channels.get(a.channel());
    let gain = if c.is_some_and(|c| !c.is3d) {
        stereo_speaker_gain(a.speaker_map.as_deref())
    } else {
        1.0
    };
    AliasRow {
        name: a.name.clone(),
        file: a.file.path().to_string(),
        sequence: i64::from(a.sequence),
        vol: (a.vol.0 * gain, a.vol.1 * gain),
        pitch: a.pitch,
        dist: (dist_min, dist_max.max(dist_min)),
        channel: channel(c),
        streamed: a.streamed(),
        looping: a.looping(),
        probability: ((a.probability * PROBABILITY_SCALE) as u32).max(1),
        master_slave: if a.master() {
            MasterSlave::Master
        } else if a.slave() {
            MasterSlave::Slave(a.slave_percentage)
        } else {
            MasterSlave::None
        },
        lod: (-1.0, -1.0),
        loadspec: String::new(),
        curve: a.curve.as_ref().map(|knots| {
            Arc::new(VfCurve {
                name: String::new(),
                knots: knots.clone(),
            })
        }),
        secondary: a.secondary.clone(),
        start_delay_ms: a.start_delay_ms.max(0) as u32,
        csv: String::new(),
        subtitle: None,
        no_wet: a.flags & vcod_cod4::sound::FLAG_NO_WET != 0,
    }
}

/// The aliases of a set of zones as the audio system's table.
pub fn table(set: &SoundSet) -> AliasTable {
    let mut t = AliasTable {
        skipped: set.skipped,
        ..AliasTable::default()
    };
    for (name, rows) in &set.aliases {
        let rows: Vec<AliasRow> = rows.iter().map(|a| row(a, &set.channels)).collect();
        if !rows.is_empty() {
            t.rows.insert(name.clone(), rows);
        }
    }
    t
}

/// The loaded sounds of `set` as `(path, .wav bytes)` for the search
/// path's overlay, at the `sound/` path an alias row's file reads;
/// those that do not convert are logged and left out.
pub fn overlay_files(set: &SoundSet) -> Vec<(String, Vec<u8>)> {
    set.loaded
        .iter()
        .filter_map(|(name, l)| match vcod_cod4::sound::wav(l) {
            Ok(w) => Some((format!("sound/{name}"), w)),
            Err(e) => {
                log::warn!("cod4: {e:#}");
                None
            }
        })
        .collect()
}

/// What a watched session plays sound from: the alias table, waiting for
/// the audio system, the alias names it holds and the weapons.
pub(crate) struct Cod4Sounds {
    table: Option<AliasTable>,
    names: HashSet<String>,
    pub(crate) weapons: Arc<HashMap<String, WeaponAv>>,
    /// Cues the events asked for whose alias the table has, and the names
    /// of those it does not.
    pub(crate) started: usize,
    pub(crate) missing: HashSet<String>,
}

impl Cod4Sounds {
    pub(crate) fn new(set: &SoundSet, weapons: Arc<HashMap<String, WeaponAv>>) -> Cod4Sounds {
        Cod4Sounds {
            table: Some(table(set)),
            names: set.aliases.keys().cloned().collect(),
            weapons,
            started: 0,
            missing: HashSet::new(),
        }
    }

    /// The table, once: the audio system takes it on the first frame.
    pub(crate) fn take_table(&mut self) -> Option<AliasTable> {
        self.table.take()
    }

    /// [`bullet_cues`], counted as [`Self::cues`] counts.
    pub(crate) fn bullet_cues(
        &mut self,
        f: &Fired,
        weapons: &[String],
        hit: &crate::cod4_fx::BulletHit,
        eye: Option<Vec3>,
    ) -> Vec<Cue> {
        let mut out = Vec::new();
        for c in bullet_cues(f, weapons, &self.weapons, hit, eye) {
            if self.has(&c.alias) {
                self.started += 1;
                out.push(c);
            } else {
                self.missing.insert(c.alias);
            }
        }
        out
    }

    pub(crate) fn has(&self, alias: &str) -> bool {
        self.names.contains(&alias.to_ascii_lowercase())
    }

    /// `f`'s cues, counted into [`Self::started`] and [`Self::missing`];
    /// `weapons` is the gamestate's weapon list.
    pub(crate) fn cues(
        &mut self,
        f: &Fired,
        weapons: &[String],
        configstring: &dyn Fn(u16) -> String,
    ) -> Vec<Cue> {
        let cues = event_cues(f, weapons, &self.weapons, configstring);
        let mut out = Vec::new();
        for c in cues {
            if self.has(&c.alias) {
                self.started += 1;
                out.push(c);
            } else {
                self.missing.insert(c.alias);
            }
        }
        out
    }
}

/// Where a fired event's voices go beyond its cues: the drawn
/// playerstate's client, whose own ring's sounds play on its entity, and
/// a shooter's drawn `tag_flash` (`None` the playerstate's view, else the
/// entity), where `CG_FireWeapon` starts the fire sound.
pub(crate) struct Placement<'a> {
    pub own: Option<u32>,
    pub flash: &'a dyn Fn(Option<u32>) -> Option<Vec3>,
}

/// One thing a fired event asks of the audio system.
#[derive(Debug, PartialEq)]
pub(crate) enum SoundOp {
    Play(Cue),
    /// `EV_SOUND_ALIAS_AS_MASTER`'s `SND_PlaySoundAliasAsMaster`.
    PlayAsMaster(Cue),
    /// `EV_STOPSOUNDS`'s `SND_StopSoundsOnEnt`.
    StopEntity(u32),
}

/// `f`'s cues placed as retail places them (cod4-sound.md 4): the
/// playerstate's own ring on the drawn client's entity (`CG_EntityEvent`
/// runs it on that client's `centity`), a shot's sound at the shooter's
/// `tag_flash`, kept at that offset from him (`CG_FireWeapon`:
/// `CG_DObjGetWorldTagPos`, the trajectory without a model), and the
/// master and stop arms as their own ops.
pub(crate) fn place(f: &Fired, cues: Vec<Cue>, at: &Placement) -> Vec<SoundOp> {
    if f.name == "EV_STOPSOUNDS" {
        return f
            .entity
            .or(at.own)
            .map(SoundOp::StopEntity)
            .into_iter()
            .collect();
    }
    let shot = matches!(
        f.name,
        "EV_FIRE_WEAPON" | "EV_FIRE_WEAPON_LASTSHOT" | "EV_FIRE_WEAPON_MG42"
    );
    let flash = shot.then(|| (at.flash)(f.entity)).flatten();
    cues.into_iter()
        .map(|mut c| {
            if let Source::Entity { num, pos } = c.source
                && f.entity.is_none()
                && num == 0
                && let Some(own) = at.own
            {
                c.source = Source::Entity { num: own, pos };
            }
            if let (Some(flash), Source::Entity { num, .. }) = (flash, c.source) {
                c.source = Source::Tag { num, pos: flash };
            }
            if f.name == "EV_SOUND_ALIAS_AS_MASTER" {
                SoundOp::PlayAsMaster(c)
            } else {
                SoundOp::Play(c)
            }
        })
        .collect()
}

/// Runs one of [`place`]'s ops on `audio`: the voice a play started.
pub(crate) fn play_op(
    audio: &mut crate::audio::AudioSystem,
    fs: &dyn vcod_common::asset_source::AssetSource,
    op: SoundOp,
) -> Option<crate::audio::voices::VoiceId> {
    match op {
        SoundOp::Play(c) => audio.play(fs, c),
        SoundOp::PlayAsMaster(c) => audio.play_as_master(fs, c),
        SoundOp::StopEntity(n) => {
            audio.stop_entity(n);
            None
        }
    }
}

/// The looping sounds of this frame (`CG_EntityEffects`'
/// `CG_AddEntityLoopSound`, `CG_AddClientSideSounds`) as the audio
/// system's `(key, alias, origin)` per sound entity: each entity of the
/// drawn snapshot with a `loopSound` plays configstring
/// [`CS_SOUND_ALIASES`] + it at its origin, and the map's createfx sounds
/// play on sound entities 1024 on. `pos_of` places an entity's loop
/// ([`loop_origin`], or its `trBase` where nothing is drawn).
pub fn loop_sounds(
    ents: &[vcod_cod4::net::delta::EntityState],
    configstring: &dyn Fn(u16) -> String,
    client: &[(Vec3, String)],
    pos_of: &dyn Fn(&vcod_cod4::net::delta::EntityState) -> Vec3,
) -> HashMap<u32, (i32, Option<String>, Vec3)> {
    let mut out = HashMap::new();
    for e in ents {
        let n = ent_u32(e, "loopSound");
        if n == 0 || e.etype() >= crate::play::cod4::ET_EVENTS {
            continue;
        }
        let alias = configstring(CS_SOUND_ALIASES + n as u16);
        let pos = pos_of(e);
        out.insert(e.number(), (n as i32, Some(alias), pos));
    }
    for (i, (pos, alias)) in client.iter().enumerate() {
        let num = vcod_cod4::fx::createfx::SOUND_ENTITY_BASE + i as u32;
        out.insert(num, (0, Some(alias.clone()), *pos));
    }
    out
}

/// `solid`'s brush model mark (`SOLID_BMODEL`).
const SOLID_BMODEL: u32 = 0xff_ffff;

/// Where `CG_AddEntityLoopSound` plays `e`'s loop (cod4-sound.md 4): its
/// drawn origin (`drawn`, else its `trBase`), and for a brush model
/// (`solid` 0xffffff) that plus its model's bounds' middle (`bounds` of
/// its `index`), unturned.
pub fn loop_origin(
    e: &vcod_cod4::net::delta::EntityState,
    drawn: Option<Vec3>,
    bounds: &dyn Fn(usize) -> Option<(Vec3, Vec3)>,
) -> Vec3 {
    let o = drawn.unwrap_or_else(|| crate::play::cod4::origin_of(e));
    if ent_u32(e, "solid") != SOLID_BMODEL {
        return o;
    }
    match bounds(ent_u32(e, "index") as usize) {
        Some((mins, maxs)) => o + (mins + maxs) * 0.5,
        None => o,
    }
}

/// A client-side bullet's sounds (cod4-sound.md 4): the impact sound at
/// the world hit (`CG_BulletHitEvent`), and `whizby` when the bullet
/// passed the listener at `eye` ([`whizby_at`]).
pub fn bullet_cues(
    f: &Fired,
    weapons: &[String],
    defs: &HashMap<String, WeaponAv>,
    hit: &crate::cod4_fx::BulletHit,
    eye: Option<Vec3>,
) -> Vec<Cue> {
    let at_point = |alias: String, pos: Vec3| Cue {
        alias,
        source: Source::Entity {
            num: ENTITYNUM_WORLD,
            pos,
        },
        delay_s: 0.0,
    };
    let mut out = Vec::new();
    let weapon = crate::play::cod4::weapon_name(weapons, f.weapon).and_then(|w| defs.get(w));
    // A player's box plays nothing here: the server's `EV_BULLET_HIT`
    // carries the flesh hit and its whizby (cod4-effects.md 12.2).
    if hit.body.is_some() {
        return out;
    }
    if hit.hit
        && let Some(a) = weapon.and_then(|w| impact_sound(w, hit.surf, 0))
    {
        out.push(at_point(a, hit.end));
    }
    // A penetrating round's further contacts: the `_exit` sounds on the far
    // sides (impact flag 4), the plain ones where it enters again.
    for p in hit.pierced.iter().flatten() {
        let flags = if p.exit { 4 } else { 0 };
        if let Some(a) = weapon.and_then(|w| impact_sound(w, p.surf, flags)) {
            out.push(at_point(a, p.at));
        }
    }
    if let Some(at) = eye.and_then(|eye| whizby_at(hit.muzzle, hit.end, eye)) {
        out.push(at_point("whizby".into(), at));
    }
    out
}

/// Where a round from `muzzle` to `end` is heard passing `eye`
/// (`iw3mp.exe` 0x457f40, cod4-effects.md 12): `eye` at least 64 units
/// down the shot and at least 64 short of its end (0x457fe4..0x458011,
/// the 64 at 0x70b128 and -64 at 0x70bd70), within 140 units of its line
/// (0x70bbe8), the sound 16 units back up the line from the nearest point.
fn whizby_at(muzzle: Vec3, end: Vec3, eye: Vec3) -> Option<Vec3> {
    let delta = end - muzzle;
    let dir = delta.normalize_or_zero();
    let dist = delta.dot(dir);
    let along = (eye - muzzle).dot(dir);
    if along < 64.0 || dist < along + 64.0 {
        return None;
    }
    let nearest = muzzle + dir * along;
    ((nearest - eye).length() <= 140.0).then(|| nearest - dir * 16.0)
}

/// `specialty_quieter`'s bit in `perks` (Dead Silence; `bg_perkNames`'
/// order, cod4-sound.md 4).
const PERK_QUIETER: u32 = 0x100;

/// `CG_RegisterSurfaceTypeSounds`' name: `<prefix>_<surface>`, the
/// surface clamped into the table.
fn surface(prefix: &str, surf: u32) -> String {
    format!(
        "{prefix}_{}",
        SURFACES[(surf as usize).min(SURFACES.len() - 1)]
    )
}

/// `CG_EntityEvent`'s sound arms (the alias names
/// `CG_RegisterSounds` builds, iw3mp.exe 0x43e140) for one
/// fired event. The playerstate's own ring is the player's view and
/// takes the `_plr` aliases.
pub fn event_cues(
    f: &Fired,
    weapons: &[String],
    defs: &HashMap<String, WeaponAv>,
    configstring: &dyn Fn(u16) -> String,
) -> Vec<Cue> {
    let plr = f.entity.is_none();
    let num = f.entity.unwrap_or(0);
    let on_ent = |alias: String| Cue {
        alias,
        source: Source::Entity { num, pos: f.origin },
        delay_s: 0.0,
    };
    let at_point = |alias: String, pos: Vec3| Cue {
        alias,
        source: Source::Entity {
            num: ENTITYNUM_WORLD,
            pos,
        },
        delay_s: 0.0,
    };
    let weapon = crate::play::cod4::weapon_name(weapons, f.weapon).and_then(|w| defs.get(w));
    // The player's view plays the `Player` field alone: an empty one is
    // silent (EV_RELOAD at `iw3mp.exe` 0x43625b, EV_NOAMMO at 0x43633b).
    // A stock gun's reload has none; its view clip's notetracks sound it
    // (`cod4_viewmodel::Cod4ViewFrame::sounds`).
    let pick = |npc: &str, player: &str| -> Option<String> {
        let w = weapon?;
        w.sound(if plr { player } else { npc }).map(str::to_string)
    };
    let plr_tag = if plr { "_plr" } else { "" };
    // Dead Silence (`specialty_quieter`, perk bit 8) on the entity's client
    // takes the `q` set: `CG_EntityEvent` adds 29 to the surface index
    // (`iw3mp.exe` 0x435de9..0x435dfc) into tables `CG_RegisterSounds`
    // fills from `qstep_*`, `qland*` and `qgear_rattle_*` (0x43e370..).
    let q = if f.perks & PERK_QUIETER != 0 { "q" } else { "" };
    let surf_of = |s: &str| -> String {
        // `step_run` / `step_run_plr`, `land` / `land_plr`.
        format!("{q}{s}{plr_tag}")
    };
    let state_u32 = |name: &str| f.state.as_ref().map_or(0, |e| ent_u32(e, name));
    let mut out = Vec::new();
    match f.id {
        // EV_LANDING_DEFAULT..EV_LANDING_PAINTEDMETAL, then the pain ones.
        77..=105 => out.push(on_ent(surface(&surf_of("land"), f.id - 77))),
        106..=134 => {
            out.push(on_ent(surface(&surf_of("land"), f.id - 106)));
            out.push(on_ent("land_damage".into()));
        }
        _ => {}
    }
    let gear = |kind: &str| {
        if plr {
            format!("{q}gear_rattle_plr_{kind}")
        } else {
            format!("{q}gear_rattle_{kind}")
        }
    };
    match f.name {
        "EV_FOOTSTEP_SPRINT" => {
            out.push(on_ent(surface(&surf_of("step_sprint"), f.parm)));
            out.push(on_ent(gear("sprint")));
        }
        "EV_FOOTSTEP_RUN" | "EV_JUMP" => {
            out.push(on_ent(surface(&surf_of("step_run"), f.parm)));
            out.push(on_ent(gear("run")));
        }
        "EV_FOOTSTEP_WALK" => {
            out.push(on_ent(surface(&surf_of("step_walk"), f.parm)));
            out.push(on_ent(gear("walk")));
        }
        "EV_FOOTSTEP_PRONE" => {
            out.push(on_ent(surface(&surf_of("step_prone"), f.parm)));
            out.push(on_ent(gear("walk")));
        }
        "EV_FOLIAGE_SOUND" => out.push(on_ent("movement_foliage".into())),
        // A script `playSound` on the entity, at its `trBase`.
        "EV_SOUND_ALIAS" | "EV_SOUND_ALIAS_AS_MASTER" if f.parm != 0 => {
            let alias = configstring(CS_SOUND_ALIASES + f.parm as u16);
            if !alias.is_empty() {
                out.push(on_ent(alias));
            }
        }
        // `CG_FireWeapon`: `fireSoundPlayer` over `fireSound`
        // when set, and on the last shot `lastShotSoundPlayer`, else
        // `lastShotSound`, over both.
        "EV_FIRE_WEAPON" | "EV_FIRE_WEAPON_LASTSHOT" | "EV_FIRE_WEAPON_MG42" => {
            let either = |npc: &str, player: &str| {
                let w = weapon?;
                plr.then(|| w.sound(player))
                    .flatten()
                    .or_else(|| w.sound(npc))
                    .map(str::to_string)
            };
            let last = (f.name == "EV_FIRE_WEAPON_LASTSHOT")
                .then(|| either("lastShotSound", "lastShotSoundPlayer"))
                .flatten();
            if let Some(a) = last.or_else(|| either("fireSound", "fireSoundPlayer")) {
                out.push(on_ent(a));
            }
        }
        "EV_RELOAD" => out.extend(pick("reloadSound", "reloadSoundPlayer").map(on_ent)),
        "EV_RELOAD_FROM_EMPTY" => {
            out.extend(pick("reloadEmptySound", "reloadEmptySoundPlayer").map(on_ent))
        }
        "EV_RELOAD_START" => {
            out.extend(pick("reloadStartSound", "reloadStartSoundPlayer").map(on_ent))
        }
        "EV_RELOAD_END" => out.extend(pick("reloadEndSound", "reloadEndSoundPlayer").map(on_ent)),
        "EV_RAISE_WEAPON" => out.extend(pick("raiseSound", "raiseSoundPlayer").map(on_ent)),
        "EV_FIRST_RAISE_WEAPON" => {
            out.extend(pick("firstRaiseSound", "firstRaiseSoundPlayer").map(on_ent))
        }
        "EV_PUTAWAY_WEAPON" => out.extend(pick("putawaySound", "putawaySoundPlayer").map(on_ent)),
        "EV_WEAPON_ALT" => out.extend(pick("altSwitchSound", "altSwitchSoundPlayer").map(on_ent)),
        "EV_PULLBACK_WEAPON" => {
            out.extend(pick("pullbackSound", "pullbackSoundPlayer").map(on_ent))
        }
        "EV_RECHAMBER_WEAPON" => {
            out.extend(pick("rechamberSound", "rechamberSoundPlayer").map(on_ent))
        }
        "EV_MELEE_SWIPE" => {
            out.extend(pick("meleeSwipeSound", "meleeSwipeSoundPlayer").map(on_ent))
        }
        // A swing's contact (`FireWeaponMelee`'s event entity at it): on a
        // body the struck entity plays the knife's `melee_knife_hit_body`
        // (the knife bit in the parm) or the weapon's `meleeHitSound`, else
        // `melee_hit`; off the world the point plays
        // `melee_knife_hit_other`, `meleeMissSound` or `melee_hit_other`
        // (`CG_EntityEvent`; the four names `CG_RegisterSounds`
        // registers, iw3mp.exe 0x6cba98..0x6cbacc).
        "EV_MELEE_HIT" | "EV_MELEE_MISS" => {
            let hit = f.name == "EV_MELEE_HIT";
            let own = weapon.and_then(|w| {
                w.sound(if hit {
                    "meleeHitSound"
                } else {
                    "meleeMissSound"
                })
            });
            let alias = match (f.parm != 0, own, hit) {
                (true, _, true) => "melee_knife_hit_body",
                (true, _, false) => "melee_knife_hit_other",
                (false, Some(a), _) => a,
                (false, None, true) => "melee_hit",
                (false, None, false) => "melee_hit_other",
            };
            let other = state_u32("otherEntityNum");
            out.push(Cue {
                alias: alias.to_string(),
                source: Source::Entity {
                    num: if hit { other } else { ENTITYNUM_WORLD },
                    pos: f.origin,
                },
                delay_s: 0.0,
            });
        }
        "EV_NOAMMO" if !weapon.is_some_and(|w| w.quiet_empty) => {
            out.extend(pick("emptyFireSound", "emptyFireSoundPlayer").map(on_ent))
        }
        "EV_DETONATE" => out.extend(pick("detonateSound", "detonateSoundPlayer").map(on_ent)),
        "EV_ITEM_PICKUP" | "EV_AMMO_PICKUP" => {
            let item = crate::play::cod4::weapon_name(weapons, f.parm).and_then(|w| defs.get(w));
            let (npc, player) = if f.name == "EV_ITEM_PICKUP" {
                ("pickupSound", "pickupSoundPlayer")
            } else {
                ("ammoPickupSound", "ammoPickupSoundPlayer")
            };
            if let Some(w) = item {
                let s = if plr { w.sound(player) } else { None };
                out.extend(s.or_else(|| w.sound(npc)).map(|s| on_ent(s.to_string())));
            }
        }
        "EV_BULLET_HIT" => {
            let surf = state_u32("surfType");
            let flags = state_u32("un1");
            if let Some(a) = weapon.and_then(|w| impact_sound(w, surf, flags)) {
                out.push(at_point(a, f.origin));
            }
        }
        "EV_BULLET_HIT_CLIENT_SMALL" => out.push(at_point(
            surface("bullet_small", state_u32("surfType")),
            f.origin,
        )),
        "EV_BULLET_HIT_CLIENT_LARGE" => out.push(at_point(
            surface("bullet_large", state_u32("surfType")),
            f.origin,
        )),
        "EV_GRENADE_BOUNCE" => {
            let surf = state_u32("surfType") as usize;
            if let Some(Some(a)) = weapon.and_then(|w| w.bounce.get(surf)) {
                out.push(at_point(a.clone(), f.origin));
            }
        }
        "EV_GRENADE_EXPLODE"
        | "EV_ROCKET_EXPLODE"
        | "EV_ROCKET_EXPLODE_NOMARKS"
        | "EV_CUSTOM_EXPLODE"
        | "EV_CUSTOM_EXPLODE_NOMARKS" => {
            let surf = state_u32("surfType");
            if let Some(a) = weapon.and_then(|w| impact_sound(w, surf, 0)) {
                out.push(at_point(a, f.origin));
            }
            if let Some(a) = weapon.and_then(|w| w.sound("projExplosionSound")) {
                out.push(at_point(a.to_string(), f.origin));
            }
        }
        "EV_FLASHBANG_EXPLODE" => {
            if let Some(a) = weapon.and_then(|w| w.sound("projExplosionSound")) {
                out.push(at_point(a.to_string(), f.origin));
            }
        }
        _ => {}
    }
    out
}

/// `CG_ImpactEffectForWeapon`'s sound: by
/// the weapon's impact type, the exit sound under impact
/// flag 4.
fn impact_sound(w: &WeaponAv, surf: u32, flags: u32) -> Option<String> {
    let exit = flags & 4 != 0;
    let pre = match (w.impact_type, exit) {
        (1, false) => "bullet_small",
        (1, true) => "bullet_small_exit",
        (2, false) => "bullet_large",
        (2, true) => "bullet_large_exit",
        (3, false) => "bullet_ap",
        (3, true) => "bullet_ap_exit",
        (4, false) => "bulletspray_small",
        (4, true) => "bulletspray_small_exit",
        (5, _) => return w.bounce.get(surf as usize).cloned().flatten(),
        (6, _) => "grenade_explode",
        (7, _) => "rocket_explode",
        (8, _) => return w.sound("projDudSound").map(str::to_string),
        _ => return None,
    };
    Some(surface(pre, surf))
}

/// Configstring `CS_AMBIENT` (821): `n\<alias>\t\<start time>`; the alias.
pub fn ambient_alias(cs: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(cs);
    let mut it = s.trim_start_matches('\\').split('\\');
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        if k == "n" && !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};

    /// The whizby window: 64 clear of both ends, 140 from the line.
    #[test]
    fn a_round_whizzes_past_only_mid_flight() {
        let (m, e) = (Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0));
        let at = whizby_at(m, e, Vec3::new(500.0, 100.0, 0.0)).unwrap();
        assert!(at.abs_diff_eq(Vec3::new(484.0, 0.0, 0.0), 1e-3), "{at}");
        assert!(whizby_at(m, e, Vec3::new(500.0, 141.0, 0.0)).is_none());
        assert!(whizby_at(m, e, Vec3::new(63.0, 10.0, 0.0)).is_none());
        assert!(whizby_at(m, e, Vec3::new(936.0, 10.0, 0.0)).is_some());
        assert!(whizby_at(m, e, Vec3::new(937.0, 10.0, 0.0)).is_none());
        assert!(whizby_at(m, e, Vec3::new(1030.0, 10.0, 0.0)).is_none());
    }

    /// The stock maps' stereo rows (cod4-sound.md 2): the default's and
    /// `amb_front`'s pass a stereo source whole, `amb_rear`'s at half a
    /// side; no map is the default.
    #[test]
    fn a_speaker_map_scales_a_2d_stereo_row() {
        use vcod_cod4::assets::sound::{SpeakerLevels, SpeakerMap};
        let lv = |speaker, l: f32, r: f32| SpeakerLevels {
            speaker,
            levels: [l, r],
        };
        let map = |l: f32| SpeakerMap {
            name: String::new(),
            is_default: false,
            maps: [
                [vec![lv(0, 0.5, 0.0), lv(1, 0.5, 0.0)], vec![]],
                [vec![lv(0, l, 0.0), lv(1, 0.0, l)], vec![]],
            ],
        };
        assert_eq!(stereo_speaker_gain(None), 1.0);
        assert_eq!(stereo_speaker_gain(Some(&map(1.0))), 1.0);
        assert_eq!(stereo_speaker_gain(Some(&map(0.5))), 0.5);
    }

    #[test]
    fn the_ambient_configstring_names_its_alias() {
        assert_eq!(
            ambient_alias(b"n\\ambient_crash\\t\\0").as_deref(),
            Some("ambient_crash")
        );
        assert_eq!(ambient_alias(b""), None);
    }

    /// The zones' loaded sounds decode from the overlay as the audio
    /// system's bank reads them, and so does the map's streamed ambient
    /// (`COD4_DIR`; vacuous without it).
    #[test]
    fn loaded_and_streamed_sounds_decode() {
        use crate::audio::bank::SoundBank;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let (_, files) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let mut fs = crate::cod4_fly::open_fs(install.root(), None, None).unwrap();
        let n = files.len();
        let names: Vec<String> = files.iter().map(|(p, _)| p.clone()).collect();
        for (p, w) in files {
            fs.overlay(&p, w);
        }
        let mut bank = SoundBank::default();
        for p in &names {
            let f = p.strip_prefix("sound/").unwrap();
            assert!(bank.get(&fs, f).is_some(), "{p}");
        }
        assert_eq!(bank.failures, 0);
        assert!(n > 700, "{n}");
        let amb = SoundBank::read_stream(&fs, "ambient/amb_middle_east1v7_lr.mp3").unwrap();
        let d = kira::sound::static_sound::StaticSoundData::from_cursor(std::io::Cursor::new(amb));
        assert!(
            d.is_ok_and(|d| d.frames.len() > 44100),
            "the ambient decodes"
        );
    }

    /// The kill capture through `cod4_client`'s sound path into a silent
    /// audio system set up as `main` sets one up for a CoD4 run (the
    /// `.iwd` search path with the overlay, a CoD4 shell's master volume
    /// cvar, the listener on the drawn eye): the ambient and the shots
    /// reach the mixer as voices with a gain and decoded, non-silent
    /// frames. A master read off an unregistered cvar left every gain 0.
    #[test]
    fn kill_capture_sounds_reach_the_mixer_audible() {
        use crate::audio::{AudioOpts, AudioSystem, bank, volume_cvar};
        use crate::play::cod4::CS_AMBIENT;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/net/play-1.7-kill-target.txt"
        );
        let mut s =
            crate::play::cod4::Cod4Session::open_capture(&std::fs::read_to_string(path).unwrap())
                .unwrap();
        let map = s.map_name().unwrap();
        let ((mut sounds, _), files) = crate::cod4_fly::load_av(install.root(), &map).unwrap();
        let mut fs = crate::cod4_fly::open_fs(install.root(), None, None).unwrap();
        for (p, w) in files {
            fs.overlay(&p, w);
        }
        let shell = crate::console::shell::Shell::new();
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: shell.cvar_f32("mss_volume"),
                rate: bank::mixer_rate(shell.cvar_f32("mss_khz") as i32),
            },
        );
        a.on_gamestate(&map);
        // Each alias the mixer held: its loudest gain and its frames' peak.
        let mut heard: HashMap<String, (f32, f32)> = HashMap::new();
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            a.set_master_volume(shell.cvar_f32(volume_cvar(&shell)));
            let fired = s.take_fired();
            if let Some(scene) = s.scene() {
                let (f, r, u) =
                    crate::camera::basis(scene.angles.y.to_radians(), -scene.angles.x.to_radians());
                a.set_listener(scene.eye, f, r, u, u32::MAX);
            }
            let Some(gs) = s.gamestate() else { continue };
            if let Some(t) = sounds.take_table() {
                a.set_cod4_aliases(t);
            }
            a.set_ambient(&fs, ambient_alias(gs.configstring(CS_AMBIENT)).as_deref());
            let weapons = crate::play::cod4::weapon_list(gs);
            let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
            for f in &fired {
                for c in sounds.cues(f, &weapons, &cs) {
                    a.play(&fs, c);
                }
            }
            for (row, gain) in a.mix() {
                let e = heard
                    .entry(row.name.clone())
                    .or_insert_with(|| (0.0, a.decoded_peak(&fs, &row).unwrap_or(0.0)));
                e.0 = e.0.max(gain);
            }
            a.step(&HashMap::new(), &HashMap::new(), None);
        }
        let audible = |k: &str| heard.get(k).is_some_and(|&(g, p)| g > 0.01 && p > 0.01);
        assert!(audible("ambient_crash"), "{heard:?}");
        assert!(audible("weap_m16_fire_npc"), "{heard:?}");
        assert!(audible("weap_m16_fire_plr"), "{heard:?}");
    }

    /// `place`: our own ring's steps on our client's entity, not 0's; a
    /// shot at the shooter's `tag_flash`, held at that offset; the master
    /// and stop arms (cod4-sound.md 4).
    #[test]
    fn events_place_their_voices_as_the_cgame_does() {
        let f = Fired {
            id: 73,
            name: "EV_FOOTSTEP_RUN",
            entity: None,
            server_time: 0,
            parm: 22,
            origin: Vec3::X,
            state: None,
            weapon: 0,
            perks: 0,
            aim: None,
        };
        let flash = |e: Option<u32>| Some(Vec3::splat(e.map_or(1.0, |n| n as f32)));
        let at = Placement {
            own: Some(3),
            flash: &flash,
        };
        let cue = |num, pos| Cue {
            alias: "a".into(),
            source: Source::Entity { num, pos },
            delay_s: 0.0,
        };
        let one = |f: &Fired, c: Cue| place(f, vec![c], &at).pop().unwrap();
        assert_eq!(
            one(&f, cue(0, Vec3::X)),
            SoundOp::Play(cue(3, Vec3::X)),
            "our own step"
        );
        let shot = Fired {
            name: "EV_FIRE_WEAPON",
            entity: Some(7),
            ..f.clone()
        };
        let tagged = |num, pos| Cue {
            source: Source::Tag { num, pos },
            ..cue(0, Vec3::ZERO)
        };
        assert_eq!(
            one(&shot, cue(7, Vec3::X)),
            SoundOp::Play(tagged(7, Vec3::splat(7.0)))
        );
        let own_shot = Fired {
            entity: None,
            ..shot.clone()
        };
        assert_eq!(
            one(&own_shot, cue(0, Vec3::X)),
            SoundOp::Play(tagged(3, Vec3::ONE)),
            "our view's flash"
        );
        let master = Fired {
            name: "EV_SOUND_ALIAS_AS_MASTER",
            entity: Some(9),
            ..f.clone()
        };
        assert_eq!(
            one(&master, cue(9, Vec3::X)),
            SoundOp::PlayAsMaster(cue(9, Vec3::X))
        );
        let stop = Fired {
            name: "EV_STOPSOUNDS",
            entity: Some(9),
            ..f.clone()
        };
        assert_eq!(place(&stop, vec![], &at), [SoundOp::StopEntity(9)]);
    }

    /// `CG_AddEntityLoopSound`: a brush model's loop at its origin plus its
    /// bounds' middle, anything else's at its drawn origin.
    #[test]
    fn a_brush_models_loop_plays_at_its_middle() {
        use vcod_cod4::net::delta::EntityState;
        let mut e = EntityState::default();
        e.0[1] = 6;
        let set = |e: &mut EntityState, name: &str, v: u32| {
            let t = EntityState::table(e.etype());
            let w = t.iter().find(|f| f.name == name).unwrap().word();
            e.0[w] = v;
        };
        set(&mut e, "lerp.pos.trBase[0]", 100f32.to_bits());
        set(&mut e, "index", 2);
        let bounds =
            |i: usize| (i == 2).then_some((Vec3::new(-8.0, 0.0, 0.0), Vec3::new(8.0, 64.0, 32.0)));
        assert_eq!(loop_origin(&e, None, &bounds), Vec3::new(100.0, 0.0, 0.0));
        assert_eq!(loop_origin(&e, Some(Vec3::Z), &bounds), Vec3::Z);
        set(&mut e, "solid", SOLID_BMODEL);
        assert_eq!(loop_origin(&e, None, &bounds), Vec3::new(100.0, 32.0, 16.0));
    }

    #[test]
    fn a_footstep_names_its_surface() {
        let f = Fired {
            id: 73,
            name: "EV_FOOTSTEP_RUN",
            entity: Some(2),
            server_time: 0,
            parm: 22,
            origin: Vec3::ZERO,
            state: None,
            weapon: 0,
            perks: 0,
            aim: None,
        };
        let cues = event_cues(&f, &[], &HashMap::new(), &|_| String::new());
        let names: Vec<&str> = cues.iter().map(|c| c.alias.as_str()).collect();
        assert_eq!(names, ["step_run_asphalt", "gear_rattle_run"]);
        let own = Fired { entity: None, ..f };
        assert_eq!(
            event_cues(&own, &[], &HashMap::new(), &|_| String::new())[0].alias,
            "step_run_plr_asphalt"
        );
        // Dead Silence on the stepping client: the `q` set, its own view too.
        let quiet = Fired {
            perks: PERK_QUIETER,
            ..f.clone()
        };
        let names: Vec<String> = event_cues(&quiet, &[], &HashMap::new(), &|_| String::new())
            .into_iter()
            .map(|c| c.alias)
            .collect();
        assert_eq!(names, ["qstep_run_asphalt", "qgear_rattle_run"]);
        let land = Fired {
            id: 77 + 22,
            name: "EV_LANDING_ASPHALT",
            entity: None,
            ..quiet
        };
        assert_eq!(
            event_cues(&land, &[], &HashMap::new(), &|_| String::new())[0].alias,
            "qland_plr_asphalt"
        );
    }

    /// A knife swing's contact: `EV_MELEE_HIT` with the knife bit plays
    /// `melee_knife_hit_body` on the struck body (`otherEntityNum`),
    /// `EV_MELEE_MISS` `melee_knife_hit_other` at the point; both names are
    /// in the zones' alias table (`COD4_DIR`).
    #[test]
    fn a_knife_contact_plays_its_hit_sound_on_the_body() {
        use vcod_cod4::net::delta::EntityState;
        let etype = crate::play::cod4::ET_EVENTS + 35;
        let mut e = EntityState::default();
        e.0[0] = 120;
        e.0[1] = etype;
        let other = EntityState::table(etype)
            .iter()
            .find(|f| f.name == "otherEntityNum")
            .unwrap()
            .word();
        e.0[other] = 3;
        let f = Fired {
            id: 35,
            name: "EV_MELEE_HIT",
            entity: Some(120),
            server_time: 0,
            parm: 1,
            origin: Vec3::new(1.0, 2.0, 3.0),
            state: Some(e),
            weapon: 0,
            perks: 0,
            aim: None,
        };
        let cues = event_cues(&f, &[], &HashMap::new(), &|_| String::new());
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].alias, "melee_knife_hit_body");
        assert!(matches!(cues[0].source, Source::Entity { num: 3, .. }));
        let miss = Fired {
            id: 36,
            name: "EV_MELEE_MISS",
            ..f.clone()
        };
        let cues = event_cues(&miss, &[], &HashMap::new(), &|_| String::new());
        assert_eq!(cues[0].alias, "melee_knife_hit_other");
        assert!(matches!(
            cues[0].source,
            Source::Entity {
                num: ENTITYNUM_WORLD,
                ..
            }
        ));
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let ((sounds, _), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        for a in [
            "melee_knife_hit_body",
            "melee_knife_hit_other",
            "melee_hit",
            "melee_hit_other",
        ] {
            assert!(sounds.has(a), "{a}");
        }
    }

    /// `CG_AddEntityLoopSound` and `CG_AddClientSideSounds`: an entity's
    /// `loopSound` names configstring 1342 + it, the createfx sounds play
    /// on sound entities 1024 on; `EV_SOUND_ALIAS` plays 1342 + its parm.
    #[test]
    fn loop_sounds_and_sound_aliases_read_their_configstrings() {
        use vcod_cod4::net::delta::EntityState;
        let cs = |n: u16| match n {
            1347 => "emt_generator".to_string(),
            1344 => "scr_alarm".to_string(),
            _ => String::new(),
        };
        let mut e = EntityState::default();
        e.0[0] = 70;
        e.0[1] = crate::play::cod4::ET_SCRIPTMOVER;
        let word = |name: &str| {
            EntityState::table(crate::play::cod4::ET_SCRIPTMOVER)
                .iter()
                .find(|f| f.name == name)
                .unwrap()
                .word()
        };
        e.0[word("loopSound")] = 5;
        e.0[word("lerp.pos.trBase[0]")] = 64f32.to_bits();
        let mut quiet = e;
        quiet.0[0] = 71;
        quiet.0[word("loopSound")] = 0;
        let client = [(Vec3::new(1.0, 2.0, 3.0), "emt_wind".to_string())];
        let loops = loop_sounds(&[e, quiet], &cs, &client, &crate::play::cod4::origin_of);
        assert_eq!(loops.len(), 2);
        assert_eq!(
            loops[&70],
            (5, Some("emt_generator".into()), Vec3::new(64.0, 0.0, 0.0))
        );
        assert_eq!(loops[&1024].1.as_deref(), Some("emt_wind"));
        let f = Fired {
            id: 1,
            name: "EV_SOUND_ALIAS",
            entity: Some(70),
            server_time: 0,
            parm: 2,
            origin: Vec3::ZERO,
            state: None,
            weapon: 0,
            perks: 0,
            aim: None,
        };
        let cues = event_cues(&f, &[], &HashMap::new(), &cs);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].alias, "scr_alarm");
    }

    /// The promotion's `mp_level_up` and the next kill's `MP_hit_alert`
    /// both ride `auto2d`, which `channels.def` leaves unrestricted: on
    /// our own entity the alert plays beside the sting, not over it.
    #[test]
    fn an_unrestricted_2d_cue_does_not_cut_the_last() {
        use crate::audio::{AudioOpts, AudioSystem};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let ((mut sounds, _), files) =
            crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let mut fs = crate::cod4_fly::open_fs(install.root(), None, None).unwrap();
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
        a.set_cod4_aliases(sounds.take_table().unwrap());
        assert!(a.play_local(&fs, "mp_level_up").is_some());
        assert!(a.play_local(&fs, "MP_hit_alert").is_some());
        let playing: HashSet<String> = a
            .mix()
            .into_iter()
            .map(|(r, _)| r.name.to_ascii_lowercase())
            .collect();
        assert!(playing.contains("mp_level_up"), "{playing:?}");
        assert!(playing.contains("mp_hit_alert"), "{playing:?}");
    }

    /// vcod-server's `war` on mp_crash with the probe gsc
    /// `crates/cod4/tests/fixtures/gsc/loop_stop` and us joined, played
    /// through `Cod4Client::pump` into a silent mixer that keeps its loops
    /// as a device would: four `script_origin`s loop beside us, then one
    /// is deleted (freed), one hidden (it leaves the snapshot), one stops
    /// its loop, and each one's voices, its secondaries' too, stop on that
    /// frame's reap, the deleted first; the fourth's ring on. 1.7's server
    /// has no `stopSounds`, so no stock path sends `EV_STOPSOUNDS`
    /// (cod4-sound.md 4). Needs `COD4_DIR` and an adapter with BC
    /// textures (lavapipe will do).
    #[test]
    fn cod4_live_entity_loops_stop_when_their_entity_goes() {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let root = install.root().to_path_buf();
        let _slot = crate::renderer::pipeline_check::live_session_slot();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.set_script_overrides(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../cod4/tests/fixtures/gsc/loop_stop"
            )
            .into(),
        );
        sv.load(Some(&install)).unwrap();
        let Some(frames) = run_held(&mut sv, move |addr| hear_loops(&root, addr), |_, _, _| true)
        else {
            return;
        };
        // The frame each group was last heard on.
        let last = |prefix: &str| {
            frames
                .iter()
                .rposition(|f| f.iter().any(|n| n.starts_with(prefix)))
        };
        let freed = last("mp_cobra_helicopter").expect("the freed loop never played");
        let hidden = last("mp_hind_helicopter").expect("the hidden loop never played");
        let stopped = last("veh_mig29_").expect("the stopped loop never played");
        let end = frames.len() - 1;
        assert!(freed < hidden && hidden < stopped && stopped < end);
        // The kept loop and its secondary, and nothing else but the
        // map's ambient, at the end.
        let mut at_end: Vec<&str> = frames[end]
            .iter()
            .map(String::as_str)
            .filter(|n| !n.starts_with("ambient_"))
            .collect();
        at_end.sort_unstable();
        assert_eq!(
            at_end,
            [
                "cobra_helicopter_dying_layer",
                "cobra_helicopter_dying_loop"
            ]
        );
        // The chain went with its primary, in the same frame.
        assert!(frames[stopped].iter().any(|n| n == "veh_mig29_dist_loop"));
    }

    /// vcod-server's `war` on mp_crash with the probe gsc
    /// `crates/cod4/tests/fixtures/gsc/sound_turn`: a `script_origin`
    /// beside us loops a sound and turns a full circle about its yaw. A
    /// voice started 100 units off it, stepped on `Cod4Client`'s live
    /// positions and angles, swings round with it at its start offset in
    /// the entity's axes (`SND_GetCurrent3DPosition`, iw3mp.exe 0x5c28d0;
    /// cod4-sound.md 4). Needs `COD4_DIR` and an adapter with BC textures.
    #[test]
    fn cod4_live_voice_turns_with_its_entity() {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let root = install.root().to_path_buf();
        let _slot = crate::renderer::pipeline_check::live_session_slot();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.set_script_overrides(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../cod4/tests/fixtures/gsc/sound_turn"
            )
            .into(),
        );
        sv.load(Some(&install)).unwrap();
        let Some(frames) = run_held(&mut sv, move |addr| hear_turn(&root, addr), |_, _, _| true)
        else {
            return;
        };
        assert!(frames.len() > 20, "{} frames", frames.len());
        let (yaw0, off0) = frames[0];
        let heading = |v: Vec3| v.y.atan2(v.x).to_degrees();
        let diff = |a: f32, b: f32| (a - b + 540.0).rem_euclid(360.0) - 180.0;
        let mut turned = 0.0f32;
        for &(yaw, off) in &frames {
            assert!((off.length() - 100.0).abs() < 0.5, "{off}");
            // The offset turns as the entity does, from where it started.
            let want = diff(heading(off0) + (yaw - yaw0), heading(off));
            assert!(want.abs() < 1.0, "yaw {yaw} offset {off}");
            turned = turned.max(diff(yaw, yaw0).abs());
        }
        assert!(turned > 90.0, "the entity turned only {turned}");
    }

    /// The client's half of [`cod4_live_voice_turns_with_its_entity`]: from
    /// the loop's first frame, the entity's live yaw and a voice's offset
    /// from it each frame; `None` without an adapter.
    fn hear_turn(root: &std::path::Path, addr: std::net::SocketAddr) -> Option<Vec<(f32, Vec3)>> {
        use crate::audio::{AudioOpts, AudioSystem, bank};
        use crate::play::cod4::Cod4Session;
        use crate::renderer::Renderer;
        use std::time::Duration;
        let join = crate::play::cod4_menus::Join {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s =
            Cod4Session::connect(addr, "joiner", vcod_cod4::net::conn::Cp::Bare, join).unwrap();
        let t0 = clk_now();
        while s.spawn().is_none() && clk_since(t0) < Duration::from_secs(60) {
            s.step(clk_since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            clk_sleep(Duration::from_millis(16));
        }
        assert!(s.spawn().is_some(), "never spawned");
        let map = crate::cod4_fly::load(root, None, "mp_crash", &s.entity_assets()).unwrap();
        let mut fs = crate::cod4_fly::open_fs(root, None, None).unwrap();
        for (p, w) in &map.sound_files {
            fs.overlay(p, w.clone());
        }
        let Some((mut r, _)) = Renderer::headless_sized(&fs, false, 1, (320, 192)) else {
            s.disconnect();
            return None;
        };
        r.set_material_lib(map.materials.clone());
        r.load_world(&map.bsp, &fs).unwrap();
        let shell = crate::console::shell::Shell::new();
        let localized = vcod_common::localize::Localized::load(&fs);
        let ui = crate::frontend::Ui::without_menus(&fs);
        let opts = || AudioOpts {
            enabled: false,
            volume: 1.0,
            rate: bank::mixer_rate(44),
        };
        let mut audio = AudioSystem::with_aliases(&fs, opts(), Default::default());
        audio.on_gamestate("mp_crash");
        // A second mixer the client's reap leaves alone, holding the probe
        // voice.
        let ((mut sounds, _), _) = crate::cod4_fly::load_av(root, "mp_crash").unwrap();
        let mut probe = AudioSystem::with_aliases(&fs, opts(), Default::default());
        probe.on_gamestate("mp_crash");
        probe.set_cod4_aliases(sounds.take_table().unwrap());
        probe.keep_silent_loops();
        let mut fx = crate::fx::sim::FxSystem::new();
        let mut c = crate::cod4_client::Cod4Client::new(
            s,
            map.models,
            map.parts,
            map.weapons,
            map.bodies,
            map.av,
            map.world,
            map.look,
        );
        let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
        let mut voice = None;
        let mut frames = Vec::new();
        let start = clk_now();
        let mut last = start;
        // Past the turn's end (6 s after the spawn) and a second.
        while clk_since(start) < Duration::from_secs(8) {
            let now = clk_now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            let mut cx = crate::cod2_client::Cod2Frame {
                fs: &fs,
                shell: &shell,
                localized: &localized,
                audio: &mut audio,
                fx: &mut fx,
                ui: &ui,
                r: &mut r,
                console_open: false,
                time: clk_since(start).as_secs_f32(),
                dt,
            };
            c.pump(&mut cx, &mut cam);
            c.session.take_commands();
            let positions = c.entity_positions();
            let angles = c.entity_angles();
            let ent = c.session.client().snapshots.last().and_then(|sn| {
                let gs = c.session.gamestate()?;
                let cs = |n: u16| String::from_utf8_lossy(gs.configstring(n)).into_owned();
                let loops = loop_sounds(&sn.entities, &cs, &[], &crate::play::cod4::origin_of);
                loops
                    .into_iter()
                    .find(|(_, (_, a, _))| a.as_deref() == Some("mp_cobra_helicopter"))
                    .map(|(n, _)| n)
            });
            if let Some(n) = ent
                && let (Some(&o), Some(&a)) = (positions.get(&n), angles.get(&n))
            {
                probe.set_listener(o, Vec3::X, -Vec3::Y, Vec3::Z, u32::MAX);
                let id = *voice.get_or_insert_with(|| {
                    let cue = Cue {
                        alias: "mp_cobra_helicopter".into(),
                        source: Source::Tag {
                            num: n,
                            pos: o + Vec3::Y * 100.0,
                        },
                        delay_s: 0.0,
                    };
                    probe.play(&fs, cue).expect("the probe voice never started")
                });
                probe.step(&positions, &angles, None);
                let at = probe.voice_source(id).unwrap().pos();
                frames.push((a.y, at - o));
            }
            clk_sleep(Duration::from_millis(16));
        }
        c.session.disconnect();
        Some(frames)
    }

    /// The client's half of
    /// [`cod4_live_entity_loops_stop_when_their_entity_goes`]: each frame's
    /// looping voices by alias, from the probe's loops on; `None` without
    /// an adapter.
    fn hear_loops(root: &std::path::Path, addr: std::net::SocketAddr) -> Option<Vec<Vec<String>>> {
        use crate::audio::{AudioOpts, AudioSystem, bank};
        use crate::play::cod4::Cod4Session;
        use crate::renderer::Renderer;
        use std::time::Duration;
        let join = crate::play::cod4_menus::Join {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s =
            Cod4Session::connect(addr, "joiner", vcod_cod4::net::conn::Cp::Bare, join).unwrap();
        let t0 = clk_now();
        while s.spawn().is_none() && clk_since(t0) < Duration::from_secs(60) {
            s.step(clk_since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            clk_sleep(Duration::from_millis(16));
        }
        assert!(s.spawn().is_some(), "never spawned");
        let map = crate::cod4_fly::load(root, None, "mp_crash", &s.entity_assets()).unwrap();
        let mut fs = crate::cod4_fly::open_fs(root, None, None).unwrap();
        for (p, w) in &map.sound_files {
            fs.overlay(p, w.clone());
        }
        let Some((mut r, _)) = Renderer::headless_sized(&fs, false, 1, (320, 192)) else {
            s.disconnect();
            return None;
        };
        r.set_material_lib(map.materials.clone());
        r.load_world(&map.bsp, &fs).unwrap();
        let shell = crate::console::shell::Shell::new();
        let localized = vcod_common::localize::Localized::load(&fs);
        let ui = crate::frontend::Ui::without_menus(&fs);
        let mut audio = AudioSystem::with_aliases(
            &fs,
            AudioOpts {
                enabled: false,
                volume: shell.cvar_f32("mss_volume"),
                rate: bank::mixer_rate(shell.cvar_f32("mss_khz") as i32),
            },
            Default::default(),
        );
        audio.on_gamestate("mp_crash");
        audio.keep_silent_loops();
        let mut fx = crate::fx::sim::FxSystem::new();
        let mut c = crate::cod4_client::Cod4Client::new(
            s,
            map.models,
            map.parts,
            map.weapons,
            map.bodies,
            map.av,
            map.world,
            map.look,
        );
        let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
        let mut frames: Vec<Vec<String>> = Vec::new();
        let start = clk_now();
        let mut last = start;
        // Past the probe's last step (9 s after the spawn) and a second.
        while clk_since(start) < Duration::from_secs(12) {
            let now = clk_now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            let mut cx = crate::cod2_client::Cod2Frame {
                fs: &fs,
                shell: &shell,
                localized: &localized,
                audio: &mut audio,
                fx: &mut fx,
                ui: &ui,
                r: &mut r,
                console_open: false,
                time: clk_since(start).as_secs_f32(),
                dt,
            };
            c.pump(&mut cx, &mut cam);
            c.session.take_commands();
            let (f, rt, u) = crate::camera::basis(cam.yaw, cam.pitch);
            audio.set_listener(cam.pos, f, rt, u, u32::MAX);
            let looping: Vec<String> = audio
                .mix()
                .into_iter()
                .filter(|(row, _)| row.looping)
                .map(|(row, _)| row.name)
                .collect();
            audio.step(&c.entity_positions(), &HashMap::new(), None);
            if !frames.is_empty() || looping.iter().any(|n| n == "mp_cobra_helicopter") {
                frames.push(looping);
            }
            clk_sleep(Duration::from_millis(16));
        }
        c.session.disconnect();
        Some(frames)
    }
}
