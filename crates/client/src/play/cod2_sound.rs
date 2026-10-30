//! The sounds a CoD2 connection plays (docs/research/cod2-sound.md 7):
//! [`Cod2Sounds::cues`] is `CG_EntityEvent`'s sound arms (the whizby
//! included) over the events `play::cod2_events::EventTracker` found,
//! [`Cod2Sounds::loops`] the entities' loop and projectile sounds,
//! [`Cod2Sounds::blends`] the sound blend entities, [`ambient`]
//! configstring 3, and [`sound_command`] `CG_ServerCommand`'s `s`, `o`,
//! `p`, `q`, `r` and `D`.

use std::collections::HashMap;

use glam::Vec3;
use vcod_cod2::net::events as ev;
use vcod_cod2::net::{ConfigStrings, Frame, SnapshotView};
use vcod_common::pk3::Pk3Fs;

use crate::audio::blend::BlendSource;
use crate::audio::cues::{Cue, Source};
use crate::play::cod2_events::{Event2, surface_name};

/// `ps.pm_flags` bits under which the cgame plays the `_plr` (first
/// person) aliases for the playerstate's own entity (`CG_EntityEvent`,
/// Mac 0x1e4650).
pub(crate) const PMF_FIRST_PERSON: i32 = 0xc00000;
/// The configstring `EV_SOUND_ALIAS`'s parm indexes from (Mac 0x1e4650:
/// `CL_GetConfigString(parm + 0x24e)`).
const CS_SOUND_ALIASES: usize = 0x24e;
/// The `s` command's highest index (`CG_ServerCommand`, Mac 0x1e2ea4:
/// `index - 1 < 0x100`).
const MAX_LOCAL_SOUND: usize = 0x100;
/// `CG_WhizbySound` (Mac 0x1d99ca): the shot's closest point to the eye
/// must be this far from both ends (`FLOAT_00326344`), within
/// [`WHIZBY_RADIUS`] of the eye (`FLOAT_0032662c`), and the sound plays
/// [`WHIZBY_BACK`] along the shot from it (`FLOAT_00326630`).
const WHIZBY_MARGIN: f32 = 64.0;
const WHIZBY_RADIUS: f32 = 140.0;
const WHIZBY_BACK: f32 = -16.0;
/// `CG_CalcMuzzlePoint` (Mac 0x1d7f72)'s heights over a client's origin
/// when his model has no flash tag: prone (`eFlags` 0x8), crouched (0x4),
/// standing (`FLOAT_00326374`, `FLOAT_003262f4`, `FLOAT_00326380`).
const MUZZLE_PRONE: f32 = 11.0;
const MUZZLE_CROUCH: f32 = 40.0;
const MUZZLE_STAND: f32 = 60.0;
const EF_PRONE: i32 = 0x8;
const EF_CROUCHING: i32 = 0x4;
/// `CG_Missile` (Mac 0x1646da) plays no `projectileSound` under this
/// `eFlags` bit.
const EF_NODRAW: i32 = 0x20;

/// A server command `CG_ServerCommand` (Mac 0x1e2ea4) turns into sound.
#[derive(Clone, Debug, PartialEq)]
pub enum SoundCommand {
    /// `s <index>`, `playLocalSound`: configstring 590 + index on the
    /// viewer (`CG_PlayClientSoundAliasByName`, Mac 0x144a64).
    Local(String),
    /// `o <alias>`, `musicPlay`.
    Music(String),
    /// `p <fade ms>`, `musicStop`.
    MusicStop(i64),
    /// `q <volume> <ms>`, `soundFade`.
    Fade(f32, i32),
    /// `r <priority> "<room>" <dry> <wet> <fade s>`, `setReverb`
    /// (`PlayerCmd_SetReverb`, Mac 0x1c1960): the fade in ms.
    Reverb {
        prio: usize,
        room: String,
        dry: f32,
        wet: f32,
        fade_ms: i32,
    },
    /// `D <priority> <fade s>`, `deactivateReverb`
    /// (`CG_DeactivateReverbCmd`, Mac 0x1e2a88): the fade in ms.
    ReverbOff { prio: usize, fade_ms: i32 },
}

/// Seconds on the wire to ms: `floorf(s * 1000 + 0.5)`, negative to 0
/// (`CG_ServerCommand`'s `r` arm, Mac 0x1e2ea4).
fn wire_ms(s: &str) -> i32 {
    let v = (atof(s) * 1000.0 + 0.5).floor() as i32;
    v.max(0)
}

/// C's `atof` over a server command's word: the longest leading number.
fn atof(s: &str) -> f32 {
    let s = s.trim_start();
    (1..=s.len())
        .rev()
        .filter(|&n| s.is_char_boundary(n))
        .find_map(|n| s[..n].parse::<f32>().ok())
        .unwrap_or(0.0)
}

/// `CG_ServerCommand`'s sound arms, keyed like the rest on the first
/// letter of the first word. `s` reads the configstring when the command
/// runs; a bad index or argument count prints and plays nothing, as an
/// empty slot does.
pub fn sound_command(text: &str, cs: &ConfigStrings) -> Option<SoundCommand> {
    let words = crate::console::shell::tokenize(text);
    let arg = |i: usize| words.get(i).map_or("", String::as_str);
    let atoi = |w: &str| {
        let t = w.trim_start();
        let end = t
            .char_indices()
            .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
            .map_or(t.len(), |(i, _)| i);
        t[..end].parse::<i64>().unwrap_or(0)
    };
    match words.first()?.bytes().next()? {
        b's' => {
            let idx = atoi(arg(1));
            if words.len() != 2 || !(1..=MAX_LOCAL_SOUND as i64).contains(&idx) {
                log::debug!("cod2: CG_LocalSound refused {text:?}");
                return None;
            }
            let alias = cs.get(CS_SOUND_ALIASES + idx as usize);
            (!alias.is_empty()).then(|| SoundCommand::Local(alias.to_string()))
        }
        b'o' => Some(SoundCommand::Music(arg(1).to_string())),
        b'p' => Some(SoundCommand::MusicStop(atoi(arg(1)))),
        b'q' => Some(SoundCommand::Fade(
            arg(1).trim().parse().unwrap_or(0.0),
            atoi(arg(2)) as i32,
        )),
        b'r' if words.len() == 6 => Some(SoundCommand::Reverb {
            prio: atoi(arg(1)).clamp(0, 3) as usize,
            room: arg(2).to_string(),
            dry: atof(arg(3)),
            wet: atof(arg(4)),
            fade_ms: wire_ms(arg(5)),
        }),
        b'D' if words.len() == 3 => Some(SoundCommand::ReverbOff {
            prio: atoi(arg(1)).clamp(0, 3) as usize,
            fade_ms: wire_ms(arg(2)),
        }),
        b'r' | b'D' => {
            log::debug!("cod2: reverb command refused {text:?}");
            None
        }
        _ => None,
    }
}

/// `CG_WhizbySound` (Mac 0x1d99ca): where a round from `muzzle` to `hit`
/// is heard passing `view`, `None` when it passes too far or too near
/// either end.
pub fn whizby(muzzle: Vec3, hit: Vec3, view: Vec3) -> Option<Vec3> {
    let shot = hit - muzzle;
    let dir = shot.normalize_or_zero();
    let along = (view - muzzle).dot(dir);
    if along < WHIZBY_MARGIN || along + WHIZBY_MARGIN > dir.dot(shot) {
        return None;
    }
    let closest = muzzle + dir * along;
    (closest.distance(view) <= WHIZBY_RADIUS).then(|| closest + dir * WHIZBY_BACK)
}

/// `CG_CalcMuzzlePoint` (Mac 0x1d7f72) for `shooter`: the view for the
/// first-person body, else the body's origin raised to its stance's
/// height (retail's fallback when the model has no `tag_flash`; vcod
/// poses no tag here). `None` for a shooter not in the snapshot.
pub(crate) fn muzzle_point(snap: &SnapshotView, shooter: i32, view: Vec3) -> Option<Vec3> {
    if snap.ps.pm_flags & PMF_FIRST_PERSON != 0 && shooter == snap.ps.client_num {
        return Some(view);
    }
    let e = snap.entity(u32::try_from(shooter).ok()?)?;
    let origin = e.pos.evaluate(snap.server_time);
    if shooter >= 64 {
        return Some(origin);
    }
    let up = match e.eflags {
        f if f & EF_PRONE != 0 => MUZZLE_PRONE,
        f if f & EF_CROUCHING != 0 => MUZZLE_CROUCH,
        _ => MUZZLE_STAND,
    };
    Some(origin + Vec3::Z * up)
}

/// The `*Sound` keys of the weapon files, by 1-based weapon index; `None`
/// when the file did not read.
#[derive(Default)]
pub struct Cod2Sounds {
    weapons: HashMap<i32, Option<HashMap<String, String>>>,
    /// The drawn bodies' world `tag_flash` by entity
    /// (`cod2_scene::Cod2Draw::muzzles`), where a fire sound and a
    /// whizby start.
    pub muzzles: HashMap<u32, Vec3>,
}

impl Cod2Sounds {
    /// Forget the weapon files: a new gamestate's list may differ.
    pub fn clear(&mut self) {
        self.weapons.clear();
    }

    /// Weapon `index`'s file under `weapons/mp/`, read once.
    fn weapon(
        &mut self,
        fs: Option<&Pk3Fs>,
        cs: &ConfigStrings,
        index: i32,
    ) -> Option<&HashMap<String, String>> {
        if index <= 0 {
            return None;
        }
        let fs = fs?;
        self.weapons
            .entry(index)
            .or_insert_with(|| {
                let name = cs.weapon_name(index)?;
                let text = fs.read(&format!("weapons/mp/{name}"))?;
                Some(vcod_common::xmodel::parse_weapon(&String::from_utf8_lossy(
                    &text,
                )))
            })
            .as_ref()
    }

    /// A weapon key's alias, `None` when blank.
    fn key(
        &mut self,
        fs: Option<&Pk3Fs>,
        cs: &ConfigStrings,
        index: i32,
        key: &str,
    ) -> Option<String> {
        self.weapon(fs, cs, index)?
            .get(key)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    /// The first of `keys` set in the weapon file.
    fn first_key(
        &mut self,
        fs: Option<&Pk3Fs>,
        cs: &ConfigStrings,
        index: i32,
        keys: &[&str],
    ) -> Option<String> {
        keys.iter().find_map(|k| self.key(fs, cs, index, k))
    }

    /// `CG_EntityEvent`'s sounds (Mac 0x1e4650) for `e` on the snapshot
    /// `snap`, heard at `view` (the whizby's eye). `fs` reads the weapon
    /// files; without it the weapon sounds are skipped. An alias the table
    /// lacks is left for the audio system to count as a miss, as
    /// `Com_FindSoundAlias` leaves a null.
    pub fn cues(
        &mut self,
        e: &Event2,
        snap: &SnapshotView,
        cs: &ConfigStrings,
        fs: Option<&Pk3Fs>,
        view: Vec3,
    ) -> Vec<Cue> {
        let on_ent = |alias: String| Cue {
            alias,
            source: Source::Entity {
                num: e.entity,
                pos: e.pos,
            },
            delay_s: 0.0,
        };
        let at_point = |alias: String| Cue {
            alias,
            source: Source::Point(e.pos),
            delay_s: 0.0,
        };
        // The `_plr` aliases: the playerstate's own body seen first person.
        let plr = snap.ps.pm_flags & PMF_FIRST_PERSON != 0 && e.entity == snap.ps.client_num as u32;
        let p = if plr { "_plr" } else { "" };
        let block = |base: i32| {
            (base..base + ev::SURFACE_EVENTS)
                .contains(&e.event)
                .then(|| surface_name(e.event - base))
        };
        let w = e.weapon;
        let mut out = Vec::new();
        if let Some(s) = block(ev::EV_FOOTSTEP_RUN) {
            out.push(on_ent(format!("step_run{p}_{s}")));
            out.push(on_ent(format!("gear_rattle{p}_run")));
        } else if let Some(s) = block(ev::EV_FOOTSTEP_WALK) {
            out.push(on_ent(format!("step_walk{p}_{s}")));
            out.push(on_ent(format!("gear_rattle{p}_walk")));
        } else if let Some(s) = block(ev::EV_FOOTSTEP_PRONE) {
            out.push(on_ent(format!("step_prone{p}_{s}")));
            out.push(on_ent(format!("gear_rattle{p}_walk")));
        } else if let Some(s) = block(ev::EV_JUMP) {
            out.push(on_ent(format!("step_run{p}_{s}")));
            out.push(on_ent(format!("gear_rattle{p}_run")));
        } else if let Some(s) = block(ev::EV_LANDING) {
            out.push(on_ent(format!("land{p}_{s}")));
        } else if let Some(s) = block(ev::EV_LANDING_PAIN) {
            out.push(on_ent(format!("land{p}_{s}")));
            out.push(on_ent("land_damage".into()));
        }
        if !out.is_empty() {
            return out;
        }
        let pick = |me: &mut Self, keys: &[&str]| me.first_key(fs, cs, w, keys);
        let alias = match e.event {
            ev::EV_FOLIAGE_SOUND => Some("movement_foliage".to_string()),
            // The item index is the weapon's (`CG_RegisterWeapon`).
            ev::EV_ITEM_PICKUP => Some(
                self.key(fs, cs, e.parm, "pickupSound")
                    .unwrap_or_else(|| "weap_pickup".into()),
            ),
            ev::EV_AMMO_PICKUP => Some(
                self.key(fs, cs, e.parm, "ammoPickupSound")
                    .unwrap_or_else(|| "weap_ammo_pickup".into()),
            ),
            ev::EV_NOAMMO => {
                let clip_only = self.key(fs, cs, w, "clipOnly").is_some_and(|v| v != "0");
                (!clip_only).then(|| "player_out_of_ammo".to_string())
            }
            ev::EV_RELOAD if plr => pick(
                self,
                &[
                    "reloadSoundPlayer",
                    "reloadEmptySoundPlayer",
                    "reloadSound",
                    "reloadEmptySound",
                ],
            ),
            ev::EV_RELOAD => pick(self, &["reloadSound", "reloadEmptySound"]),
            ev::EV_RELOAD_FROM_EMPTY if plr => pick(
                self,
                &[
                    "reloadEmptySoundPlayer",
                    "reloadSoundPlayer",
                    "reloadEmptySound",
                    "reloadSound",
                ],
            ),
            ev::EV_RELOAD_FROM_EMPTY => pick(self, &["reloadEmptySound", "reloadSound"]),
            ev::EV_RELOAD_START if plr => {
                pick(self, &["reloadStartSoundPlayer", "reloadStartSound"])
            }
            ev::EV_RELOAD_START => pick(self, &["reloadStartSound"]),
            ev::EV_RELOAD_END if plr => pick(self, &["reloadEndSoundPlayer", "reloadEndSound"]),
            ev::EV_RELOAD_END => pick(self, &["reloadEndSound"]),
            ev::EV_RAISE_WEAPON => {
                Some(pick(self, &["raiseSound"]).unwrap_or_else(|| "weap_raise".into()))
            }
            ev::EV_PUTAWAY_WEAPON => {
                Some(pick(self, &["putawaySound"]).unwrap_or_else(|| "weap_putaway".into()))
            }
            ev::EV_WEAPON_ALT => pick(self, &["altSwitchSound"]),
            ev::EV_PULLBACK_WEAPON => pick(self, &["pullbackSound"]),
            ev::EV_RECHAMBER_WEAPON if plr => {
                pick(self, &["rechamberSoundPlayer", "rechamberSound"])
            }
            ev::EV_RECHAMBER_WEAPON => pick(self, &["rechamberSound"]),
            ev::EV_MELEE_SWIPE => pick(self, &["meleeSwipeSound"]),
            _ => None,
        };
        if let Some(a) = alias {
            return vec![on_ent(a)];
        }
        let fire = |me: &mut Self, last: bool| {
            let fire = match plr {
                true => pick(me, &["fireSoundPlayer", "fireSound"]),
                false => pick(me, &["fireSound"]),
            };
            let last_shot = match plr {
                true => pick(me, &["lastShotSoundPlayer", "lastShotSound"]),
                false => pick(me, &["lastShotSound"]),
            };
            if last { last_shot.or(fire) } else { fire }
        };
        let surf = surface_name(e.surf);
        let explosion = |me: &mut Self| pick(me, &["projExplosionSound"]);
        // `CG_FireWeapon` (Mac 0x1d823c) plays on the entity from its world
        // `tag_flash`, carried with it; the first-person body's is the
        // view weapon's, and a body without the tag plays at its
        // trajectory, the event's own position.
        let flash = match self.muzzles.get(&e.entity) {
            Some(&pos) if !plr => Source::Tag { num: e.entity, pos },
            _ => on_ent(String::new()).source,
        };
        let at_flash = |alias: String| Cue {
            alias,
            source: flash,
            delay_s: 0.0,
        };
        match e.event {
            ev::EV_FIRE_WEAPON | ev::EV_FIRE_WEAPONB | ev::EV_FIRE_WEAPON_MG42 => {
                out.extend(fire(self, false).map(at_flash));
            }
            ev::EV_FIRE_WEAPON_LASTSHOT => out.extend(fire(self, true).map(at_flash)),
            ev::EV_FIRE_QUADBARREL_1 | ev::EV_FIRE_QUADBARREL_2 => {
                for _ in 0..2 {
                    out.extend(fire(self, false).map(at_flash));
                }
            }
            ev::EV_MELEE_HIT => out.push(Cue {
                source: Source::Entity {
                    num: e.other_entity as u32,
                    pos: e.pos,
                },
                ..on_ent("melee_hit".into())
            }),
            ev::EV_MELEE_MISS => out.push(Cue {
                source: Source::Entity {
                    num: e.other_entity as u32,
                    pos: e.pos,
                },
                ..on_ent("melee_hit_other".into())
            }),
            ev::EV_SOUND_ALIAS | ev::EV_SOUND_ALIAS_AS_MASTER => {
                let name = cs.get(CS_SOUND_ALIASES + e.parm as usize);
                if !name.is_empty() {
                    out.push(on_ent(name.to_string()));
                }
            }
            // `CG_BulletHitEvent` (Mac 0x1db1f4) and
            // `CG_BulletHitClientEvent` (Mac 0x1d9b78): a shotgun's spray,
            // a small or large round; a client hit's large row is read
            // against 183, so 186 plays the spray (cod2-sound.md 6).
            // Both then whizz past the eye off the shooter's muzzle
            // (`cg_tracerChance` above 0, its default 0.4).
            ev::EV_SHOTGUN_HIT
            | ev::EV_BULLET_HIT_SMALL
            | ev::EV_BULLET_HIT_LARGE
            | ev::EV_BULLET_HIT_CLIENT_SMALL
            | ev::EV_BULLET_HIT_CLIENT_LARGE => {
                let base = match e.event {
                    ev::EV_BULLET_HIT_SMALL | ev::EV_BULLET_HIT_CLIENT_SMALL => "bullet_small",
                    ev::EV_BULLET_HIT_LARGE => "bullet_large",
                    _ => "bulletspray_small",
                };
                out.push(at_point(format!("{base}_{surf}")));
                // The shooter's drawn `tag_flash`, else `CG_CalcMuzzlePoint`'s
                // view or stance point.
                let first = snap.ps.pm_flags & PMF_FIRST_PERSON != 0
                    && e.other_entity == snap.ps.client_num;
                let muzzle = u32::try_from(e.other_entity)
                    .ok()
                    .filter(|_| !first)
                    .and_then(|n| self.muzzles.get(&n).copied())
                    .or_else(|| muzzle_point(snap, e.other_entity, view));
                if let Some(at) = muzzle.and_then(|m| whizby(m, e.pos, view)) {
                    out.push(Cue {
                        source: Source::Point(at),
                        ..at_point("whizby".into())
                    });
                }
            }
            ev::EV_GRENADE_BOUNCE => out.push(at_point(format!("grenade_bounce_{surf}"))),
            ev::EV_GRENADE_EXPLODE => {
                out.push(at_point(format!("grenade_explode_{surf}")));
                out.extend(explosion(self).map(at_point));
            }
            ev::EV_ROCKET_EXPLODE | ev::EV_ROCKET_EXPLODE_NOMARKS => {
                out.push(at_point(format!("rocket_explode_{surf}")));
                out.extend(explosion(self).map(at_point));
            }
            // Played unless the entity carries eFlags 0x10000 and the
            // blast is 50 ms old or more.
            ev::EV_CUSTOM_EXPLODE if e.eflags & 0x10000 == 0 || snap.server_time - e.time < 50 => {
                out.extend(explosion(self).map(at_point));
            }
            ev::EV_GRENADE_SUICIDE => out.push(at_point("grenade_explode_default".into())),
            _ => {}
        }
        out
    }
}

/// One looping voice per entity: a key whose change restarts it, the
/// alias (`None` when it names none) and where it plays.
pub type LoopSounds = HashMap<u32, (i32, Option<String>, Vec3)>;

impl Cod2Sounds {
    /// The loops `frame`'s entities hold at the render time:
    /// `CG_EntityEffects` (Mac 0x163dd0) plays every entity's `loopSound`
    /// (configstring 590 + it) at its origin each frame, and `CG_Missile`
    /// (Mac 0x1646da) a missile's weapon's `projectileSound` once its
    /// `es.time` is reached, unless `eFlags` 0x20. The engine's looping
    /// dedupe keeps one voice each. Returns the loop sounds keyed by
    /// index and the missiles' keyed by weapon.
    pub fn loops(&mut self, frame: &Frame, fs: Option<&Pk3Fs>) -> (LoopSounds, LoopSounds) {
        let cs = frame.configstrings;
        let (mut loops, mut missiles) = (HashMap::new(), HashMap::new());
        for e in &frame.snap.entities {
            let idx = e.raw.i32("loopSound");
            let missile = e.etype == ev::ET_MISSILE
                && e.eflags & EF_NODRAW == 0
                && e.raw.i32("time") <= frame.time_ms
                && e.weapon > 0;
            if idx <= 0 && !missile {
                continue;
            }
            let pos = frame.entity_origin(e);
            if idx > 0 {
                let alias = cs.get(CS_SOUND_ALIASES + idx as usize);
                let alias = (!alias.is_empty()).then(|| alias.to_string());
                loops.insert(e.number, (idx, alias, pos));
            }
            if missile {
                let alias = self.key(fs, cs, e.weapon, "projectileSound");
                missiles.insert(e.number, (e.weapon, alias, pos));
            }
        }
        (loops, missiles)
    }
}

impl Cod2Sounds {
    /// `CG_SoundBlend` (Mac 0x16351c) for every sound blend entity
    /// (`eType` 7) of `frame`: the aliases at configstring 590 plus
    /// `eventParms[0]` and `eventParms[1]` (either 0 plays nothing),
    /// crossfaded by `leanf` lerped from the older snapshot to the newer,
    /// at the entity's drawn origin. The engine plays the pair as one
    /// voice through `SND_PlayBlendedSoundAliases` (Mac 0x4c1d0), which
    /// [`crate::audio::AudioSystem::set_sound_blends`] models. No stock
    /// 1.3 server makes one: the game has no script call that spawns it.
    pub fn blends(frame: &Frame) -> Vec<BlendSource> {
        let cs = frame.configstrings;
        let mut out = Vec::new();
        for e in &frame.snap.entities {
            if e.etype != ev::ET_SOUND_BLEND {
                continue;
            }
            let alias = |field: &str| {
                let i = usize::try_from(e.raw.i32(field)).ok().filter(|&i| i > 0)?;
                let a = cs.get(CS_SOUND_ALIASES + i);
                (!a.is_empty()).then(|| a.to_string())
            };
            let (Some(a), Some(b)) = (alias("eventParms[0]"), alias("eventParms[1]")) else {
                continue;
            };
            let next = e.raw.f32("leanf");
            let cur = frame
                .prev
                .and_then(|p| p.entity(e.number))
                .map_or(next, |p| p.raw.f32("leanf"));
            out.push(BlendSource {
                entity: e.number,
                aliases: [a, b],
                blend: cur + (next - cur) * frame.frac,
                pitch_scale: 1.0,
                pos: frame.entity_origin(e),
            });
        }
        out
    }
}

/// Configstring 3 as `CG_StartAmbient` (Mac 0x143de6) reads it at server
/// time `now`: the text, the alias `n` (`None` when blank) and the ms left
/// to `t`, 0 when past or before the first snapshot.
pub fn ambient(cs: &ConfigStrings, now: i32) -> (String, Option<String>, i32) {
    let text = cs.get(3);
    let t = vcod_cod2::net::oob::info_value_for_key(text, "t")
        .map_or(0, |v| v.trim().parse::<i32>().unwrap_or(0));
    let fade = if now == 0 { 0 } else { (t - now).max(0) };
    (text.to_string(), ambient_alias(cs), fade)
}

/// Configstring 3, `n\<alias>\t\<start ms>` (`Scr_AmbientPlay`, Mac
/// 0x19c512): the ambient alias, `None` when blank.
pub fn ambient_alias(cs: &ConfigStrings) -> Option<String> {
    vcod_cod2::net::oob::info_value_for_key(cs.get(3), "n")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The alias a map script's first `ambientPlay("...")` call names: fly
/// mode runs no script, so this stands in for configstring 3. `//`
/// comments are skipped.
pub fn script_ambient(gsc: &str) -> Option<String> {
    gsc.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .find_map(|l| {
            let at = l.to_ascii_lowercase().find("ambientplay")?;
            let rest = l[at + "ambientplay".len()..]
                .trim_start()
                .strip_prefix('(')?;
            let rest = rest.trim_start().strip_prefix('"')?;
            Some(rest[..rest.find('"')?].to_string())
        })
        .filter(|a| !a.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::cod2_events::EventTracker;
    use vcod_cod2::net::capture::{Capture, Dir};
    use vcod_cod2::net::delta::EntityState2;
    use vcod_cod2::net::oob::{self, Reply};
    use vcod_cod2::net::{Client, NetView};

    const FIXTURE: &[u8] = include_bytes!("../../../cod2/tests/fixtures/mp_toujane-spectate.bin");

    #[test]
    fn a_map_script_names_its_ambient() {
        let gsc = "main()\n{\n\t// ambientPlay(\"old\");\n\tmaps\\mp\\_load::main();\n\tambientPlay(\"ambient_france\");\n}\n";
        assert_eq!(script_ambient(gsc).as_deref(), Some("ambient_france"));
        assert_eq!(script_ambient("main() {}"), None);
    }

    /// The retail capture (`crates/cod2/tests/replay.rs`) through the
    /// receive path, each snapshot walked by the tracker as it becomes
    /// current, every cue the dispatch starts listed with its snapshot's
    /// time and entity, and the events by name.
    #[allow(clippy::type_complexity)]
    fn replay(fs: Option<&Pk3Fs>) -> (NetView, Vec<(i32, u32, String)>, Vec<(u32, &'static str)>) {
        let cap = Capture::decode(FIXTURE).unwrap();
        let mut challenge = None;
        let mut client: Option<Client> = None;
        let mut view = NetView::default();
        let mut tracker = EventTracker::default();
        let mut sounds = Cod2Sounds::default();
        let mut events = Vec::new();
        let mut cues = Vec::new();
        for (dir, p) in &cap.packets {
            if let Some(pl) = oob::payload(p) {
                match (dir, oob::parse_reply(pl)) {
                    (Dir::FromServer, Reply::Challenge(c)) => challenge = Some(c),
                    (Dir::ToServer, Reply::Other(text)) => {
                        if let Some(q) = text.strip_prefix("connect \"").and_then(|ui| {
                            oob::info_value_for_key(ui.trim_end_matches('"'), "qport")
                        }) {
                            client = Some(Client::new(q.parse().unwrap(), challenge.unwrap()));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if *dir != Dir::FromServer {
                continue;
            }
            let cl = client.as_mut().unwrap();
            let Some(parsed) = cl.receive(p).unwrap() else {
                continue;
            };
            view.update(&cl.parser, &parsed);
            // The cgame's current snapshot is the older of the two.
            let Some(cur) = view.prev.as_ref().or(view.snap.as_ref()) else {
                continue;
            };
            for e in tracker.transition(cur) {
                for c in sounds.cues(&e, cur, &view.configstrings, fs, Vec3::from(cur.ps.eye())) {
                    cues.push((cur.server_time, e.entity, c.alias));
                }
                events.push((e.entity, ev::event_name(e.event).unwrap_or("?")));
            }
        }
        (view, cues, events)
    }

    /// The other client runs on mp_toujane's dirt: one step from his
    /// entity's ring, with its gear rattle; the level's ambient is
    /// `ambientPlay`'s.
    #[test]
    fn retail_toujane_capture_plays_the_other_players_steps() {
        let (view, cues, events) = replay(None);
        assert_eq!(
            cues,
            [
                (16050, 0, "step_run_dirt".to_string()),
                (16050, 0, "gear_rattle_run".to_string()),
            ]
        );
        assert_eq!(events, [(0, "EV_FOOTSTEP_RUN_DIRT")]);
        assert_eq!(
            ambient_alias(&view.configstrings).as_deref(),
            Some("ambient_africa")
        );
    }

    /// The bullet hits by what `CG_BulletHitEvent` and
    /// `CG_BulletHitClientEvent` play (cod2-efx.md 6.2, cod2-sound.md 6).
    #[test]
    fn bullet_hits_play_their_rows_aliases() {
        let snap = vcod_cod2::net::SnapshotView {
            message_num: 1,
            server_time: 100,
            snap_flags: 0,
            ps: vcod_cod2::net::PlayerView::new(&Default::default(), 0),
            entities: Vec::new(),
            clients: Vec::new(),
        };
        let cs = ConfigStrings::default();
        let mut sounds = Cod2Sounds::default();
        let mut alias = |event: i32| {
            let e = Event2 {
                entity: 70,
                event,
                parm: 0,
                etype: ev::ET_EVENTS + event,
                eflags: 0,
                weapon: 0,
                surf: 5,
                other_entity: 0,
                time: 0,
                scale: 0,
                angles2: [0.0; 2],
                pos: Vec3::ZERO,
                angles: Vec3::ZERO,
                from_ps: false,
            };
            let c = sounds.cues(&e, &snap, &cs, None, Vec3::ZERO);
            assert_eq!(c.len(), 1);
            c[0].alias.clone()
        };
        assert_eq!(alias(ev::EV_SHOTGUN_HIT), "bulletspray_small_concrete");
        assert_eq!(alias(ev::EV_BULLET_HIT_SMALL), "bullet_small_concrete");
        assert_eq!(alias(ev::EV_BULLET_HIT_LARGE), "bullet_large_concrete");
        assert_eq!(
            alias(ev::EV_BULLET_HIT_CLIENT_SMALL),
            "bullet_small_concrete"
        );
        assert_eq!(
            alias(ev::EV_BULLET_HIT_CLIENT_LARGE),
            "bulletspray_small_concrete"
        );
    }

    /// With an install: every alias the dispatch names for a kar98k's
    /// shots, reloads and steps, a frag's bounce and blast and the hits,
    /// on the capture's last snapshot, is in mp_toujane's table, and its
    /// files decode.
    #[test]
    fn stock_events_name_aliases_the_table_holds() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let (view, cues, _) = replay(Some(&fs));
        assert_eq!(cues.len(), 2);
        let cs = &view.configstrings;
        let cur = view.prev.as_ref().unwrap();
        let weapon = |name: &str| {
            (1..64)
                .find(|&i| cs.weapon_name(i) == Some(name))
                .unwrap_or_else(|| panic!("{name} not in {:?}", cs.get(7)))
        };
        let table = crate::audio::cod2::load(&fs).for_map("mp_toujane");
        let mut bank = crate::audio::bank::SoundBank::at_rate(0);
        let mut sounds = Cod2Sounds::default();
        let at = |event: i32, weapon: i32, surf: i32| Event2 {
            entity: 0,
            event,
            parm: 0,
            etype: ev::ET_PLAYER,
            eflags: 0,
            weapon,
            surf,
            other_entity: 1,
            time: cur.server_time,
            scale: 0,
            angles2: [0.0; 2],
            pos: Vec3::ZERO,
            angles: Vec3::ZERO,
            from_ps: false,
        };
        let kar = weapon("kar98k_mp");
        let frag = weapon("frag_grenade_german_mp");
        let dirt = 6;
        let mut heard = Vec::new();
        for e in [
            at(ev::EV_FIRE_WEAPON, kar, 0),
            at(ev::EV_RECHAMBER_WEAPON, kar, 0),
            at(ev::EV_RELOAD, kar, 0),
            at(ev::EV_RAISE_WEAPON, kar, 0),
            at(ev::EV_FOOTSTEP_PRONE + dirt, kar, 0),
            at(ev::EV_LANDING_PAIN + dirt, kar, 0),
            at(ev::EV_GRENADE_BOUNCE, frag, dirt),
            at(ev::EV_GRENADE_EXPLODE, frag, dirt),
            at(ev::EV_BULLET_HIT_SMALL, kar, dirt),
            at(ev::EV_BULLET_HIT_LARGE, kar, dirt),
            at(ev::EV_SHOTGUN_HIT, kar, dirt),
            at(ev::EV_BULLET_HIT_CLIENT_SMALL, kar, 7),
        ] {
            let c = sounds.cues(&e, cur, cs, Some(&fs), Vec3::ZERO);
            assert!(!c.is_empty(), "{:?}", ev::event_name(e.event));
            heard.extend(c.into_iter().map(|c| c.alias));
        }
        for alias in &heard {
            let rows = table
                .get(alias)
                .unwrap_or_else(|| panic!("{alias} not loaded"));
            for r in rows.iter().filter(|r| !r.streamed) {
                assert!(bank.get(&fs, &r.file).is_some(), "{alias}: {}", r.file);
            }
        }
        assert!(heard.contains(&"weap_kar98k_fire".to_string()), "{heard:?}");
        let fire = &table.get("weap_kar98k_fire").unwrap()[0];
        assert_eq!(
            fire.curve.as_ref().map(|c| c.name.as_str()),
            Some("weapon2")
        );
        // The level's ambient streams.
        let amb = &table.get("ambient_africa").unwrap()[0];
        assert!(amb.streamed && amb.looping, "{amb:?}");
        assert!(fs.source_archive(&format!("sound/{}", amb.file)).is_some());
    }

    fn table(slots: &[(usize, &str)]) -> ConfigStrings {
        let gs = vcod_cod2::net::Gamestate2 {
            server_command_sequence: 0,
            configstrings: Default::default(),
            baselines: Default::default(),
            client_num: 0,
            checksum_feed: 0,
        };
        let mut cs = ConfigStrings::from_gamestate(&gs);
        for (slot, text) in slots {
            cs.apply_command(format!("d {slot} {text}").as_bytes());
        }
        cs
    }

    fn snap_of(ents: &[EntityState2], t: i32) -> SnapshotView {
        SnapshotView {
            message_num: t,
            server_time: t,
            snap_flags: 0,
            ps: vcod_cod2::net::PlayerView::new(&Default::default(), 0),
            entities: ents.iter().map(vcod_cod2::net::EntityView::new).collect(),
            clients: Vec::new(),
        }
    }

    fn set(es: &mut EntityState2, k: &str, v: u32) {
        let i = vcod_cod2::net::delta::field_index(&vcod_cod2::net::fields::ENTITY_STATE_FIELDS, k);
        es.f[i] = v;
    }

    fn put(es: &mut EntityState2, at: Vec3) {
        for (i, v) in at.to_array().into_iter().enumerate() {
            set(es, &format!("pos.trBase[{i}]"), v.to_bits());
        }
    }

    fn event(entity: u32, event: i32, surf: i32) -> Event2 {
        Event2 {
            entity,
            event,
            parm: 0,
            etype: ev::ET_PLAYER,
            eflags: 0,
            weapon: 0,
            surf,
            other_entity: 9,
            time: 0,
            scale: 0,
            angles2: [0.0; 2],
            pos: Vec3::ZERO,
            angles: Vec3::ZERO,
            from_ps: false,
        }
    }

    /// Every arm that reads no weapon file plays `CG_RegisterSounds`' name
    /// for the `cgs` slot `CG_EntityEvent` reads (Mac 0x1e4650,
    /// 0x144cf8), third person and as the first-person body.
    #[test]
    fn each_event_plays_retails_alias() {
        let cs = table(&[(CS_SOUND_ALIASES + 5, "MP_bomb_plant")]);
        let third = snap_of(&[], 100);
        let mut first = snap_of(&[], 100);
        first.ps.pm_flags = PMF_FIRST_PERSON;
        first.ps.client_num = 3;
        let mut sounds = Cod2Sounds::default();
        let mut heard = |e: Event2, snap: &SnapshotView| -> Vec<(String, u32)> {
            sounds
                .cues(&e, snap, &cs, None, Vec3::ZERO)
                .into_iter()
                .map(|c| {
                    let num = match c.source {
                        Source::Entity { num, .. } => num,
                        _ => u32::MAX,
                    };
                    (c.alias, num)
                })
                .collect()
        };
        let on = |names: &[&str], n: u32| -> Vec<(String, u32)> {
            names.iter().map(|a| (a.to_string(), n)).collect()
        };
        let world = u32::MAX;
        // (event, surface, third person, first person)
        let cases: &[(i32, i32, &[&str], &[&str])] = &[
            (
                ev::EV_FOOTSTEP_RUN + 6,
                0,
                &["step_run_dirt", "gear_rattle_run"],
                &["step_run_plr_dirt", "gear_rattle_plr_run"],
            ),
            (
                ev::EV_FOOTSTEP_WALK + 21,
                0,
                &["step_walk_wood", "gear_rattle_walk"],
                &["step_walk_plr_wood", "gear_rattle_plr_walk"],
            ),
            (
                ev::EV_FOOTSTEP_PRONE + 10,
                0,
                &["step_prone_grass", "gear_rattle_walk"],
                &["step_prone_plr_grass", "gear_rattle_plr_walk"],
            ),
            (
                ev::EV_JUMP + 19,
                0,
                &["step_run_snow", "gear_rattle_run"],
                &["step_run_plr_snow", "gear_rattle_plr_run"],
            ),
            (ev::EV_LANDING + 13, 0, &["land_metal"], &["land_plr_metal"]),
            (
                ev::EV_LANDING_PAIN + 22,
                0,
                &["land_asphalt", "land_damage"],
                &["land_plr_asphalt", "land_damage"],
            ),
            (
                ev::EV_FOLIAGE_SOUND,
                0,
                &["movement_foliage"],
                &["movement_foliage"],
            ),
            // No weapon file: the registered fallbacks.
            (ev::EV_ITEM_PICKUP, 0, &["weap_pickup"], &["weap_pickup"]),
            (
                ev::EV_AMMO_PICKUP,
                0,
                &["weap_ammo_pickup"],
                &["weap_ammo_pickup"],
            ),
            (
                ev::EV_NOAMMO,
                0,
                &["player_out_of_ammo"],
                &["player_out_of_ammo"],
            ),
            (ev::EV_RAISE_WEAPON, 0, &["weap_raise"], &["weap_raise"]),
            (
                ev::EV_PUTAWAY_WEAPON,
                0,
                &["weap_putaway"],
                &["weap_putaway"],
            ),
        ];
        for &(ev_num, surf, third_names, first_names) in cases {
            let name = ev::event_name(ev_num);
            assert_eq!(
                heard(event(3, ev_num, surf), &third),
                on(third_names, 3),
                "{name:?}"
            );
            assert_eq!(
                heard(event(3, ev_num, surf), &first),
                on(first_names, 3),
                "{name:?} plr"
            );
        }
        // Another body under a first-person view keeps the plain aliases.
        assert_eq!(
            heard(event(4, ev::EV_LANDING + 13, 0), &first),
            on(&["land_metal"], 4)
        );
        // The melee pair on `otherEntityNum`.
        assert_eq!(
            heard(event(3, ev::EV_MELEE_HIT, 0), &third),
            on(&["melee_hit"], 9)
        );
        assert_eq!(
            heard(event(3, ev::EV_MELEE_MISS, 0), &third),
            on(&["melee_hit_other"], 9)
        );
        // `playSound`'s configstring.
        let mut e = event(70, ev::EV_SOUND_ALIAS, 0);
        e.parm = 5;
        assert_eq!(heard(e, &third), on(&["MP_bomb_plant"], 70));
        // The surface-keyed blasts at the point.
        for (ev_num, surf, alias) in [
            (ev::EV_GRENADE_BOUNCE, 17, "grenade_bounce_rock"),
            (ev::EV_GRENADE_EXPLODE, 1, "grenade_explode_bark"),
            (ev::EV_ROCKET_EXPLODE, 20, "rocket_explode_water"),
            (ev::EV_ROCKET_EXPLODE_NOMARKS, 2, "rocket_explode_brick"),
            (ev::EV_GRENADE_SUICIDE, 9, "grenade_explode_default"),
        ] {
            assert_eq!(heard(event(70, ev_num, surf), &third), on(&[alias], world));
        }
        // A custom explode plays only its weapon's sound: none here.
        assert!(heard(event(70, ev::EV_CUSTOM_EXPLODE, 0), &third).is_empty());
    }

    /// `CG_WhizbySound`'s window: 64 units clear of both ends, 140 from
    /// the eye, heard 16 back toward the shooter.
    #[test]
    fn a_round_whizzes_past_only_close_and_mid_flight() {
        let muzzle = Vec3::ZERO;
        let hit = Vec3::new(1000.0, 0.0, 0.0);
        let at = whizby(muzzle, hit, Vec3::new(500.0, 100.0, 0.0)).unwrap();
        assert!(at.abs_diff_eq(Vec3::new(484.0, 0.0, 0.0), 1e-3), "{at}");
        assert!(whizby(muzzle, hit, Vec3::new(500.0, 141.0, 0.0)).is_none());
        assert!(whizby(muzzle, hit, Vec3::new(63.0, 10.0, 0.0)).is_none());
        assert!(whizby(muzzle, hit, Vec3::new(64.0, 10.0, 0.0)).is_some());
        assert!(whizby(muzzle, hit, Vec3::new(937.0, 10.0, 0.0)).is_none());
        assert!(whizby(muzzle, muzzle, Vec3::ZERO).is_none());
        // Through the event: a standing shooter's round past the eye.
        let mut shooter = EntityState2::null(2);
        put(&mut shooter, Vec3::new(0.0, 0.0, -60.0));
        let snap = snap_of(&[shooter], 100);
        let mut e = event(80, ev::EV_BULLET_HIT_SMALL, 5);
        e.etype = ev::ET_EVENTS + e.event;
        e.other_entity = 2;
        e.pos = hit;
        let mut sounds = Cod2Sounds::default();
        let cs = ConfigStrings::default();
        let c = sounds.cues(&e, &snap, &cs, None, Vec3::new(500.0, 100.0, 0.0));
        let got: Vec<_> = c
            .iter()
            .map(|c| (c.alias.as_str(), c.source.pos()))
            .collect();
        assert_eq!(
            got,
            [
                ("bullet_small_concrete", hit),
                ("whizby", Vec3::new(484.0, 0.0, 0.0))
            ]
        );
        // The first-person shooter's own round starts at the eye: none.
        let mut own = snap_of(&[], 100);
        own.ps.pm_flags = PMF_FIRST_PERSON;
        own.ps.client_num = 2;
        let c = sounds.cues(&e, &own, &cs, None, Vec3::new(500.0, 100.0, 0.0));
        assert_eq!(c.len(), 1);
    }

    /// `CG_ServerCommand`'s sound letters.
    #[test]
    fn sound_server_commands_read_as_retail() {
        let cs = table(&[(CS_SOUND_ALIASES + 3, "MP_announcer_allies_win")]);
        assert_eq!(
            sound_command("s 3", &cs),
            Some(SoundCommand::Local("MP_announcer_allies_win".into()))
        );
        assert_eq!(sound_command("s 4", &cs), None);
        assert_eq!(sound_command("s 0", &cs), None);
        assert_eq!(sound_command("s 257", &cs), None);
        assert_eq!(sound_command("s 3 1", &cs), None);
        assert_eq!(
            sound_command("o mus_victory", &cs),
            Some(SoundCommand::Music("mus_victory".into()))
        );
        assert_eq!(
            sound_command("p 2000", &cs),
            Some(SoundCommand::MusicStop(2000))
        );
        assert_eq!(sound_command("p", &cs), Some(SoundCommand::MusicStop(0)));
        assert_eq!(
            sound_command("q 0.500000 1500\n", &cs),
            Some(SoundCommand::Fade(0.5, 1500))
        );
        assert_eq!(sound_command("b 0 0", &cs), None);
    }

    /// An entity's `loopSound` and a flying missile's `projectileSound`
    /// loop at the entity; a missile not yet launched or hidden is quiet.
    #[test]
    fn loop_and_projectile_sounds_follow_their_entities() {
        let cs = table(&[(CS_SOUND_ALIASES + 7, "bomb_tick")]);
        let mut bomb = EntityState2::null(80);
        set(&mut bomb, "loopSound", 7);
        put(&mut bomb, Vec3::new(1.0, 2.0, 3.0));
        let mut nade = EntityState2::null(81);
        set(&mut nade, "eType", ev::ET_MISSILE as u32);
        set(&mut nade, "weapon", 4);
        set(&mut nade, "time", 50);
        let mut late = nade;
        late.number = 82;
        set(&mut late, "time", 500);
        let mut hidden = nade;
        hidden.number = 83;
        set(&mut hidden, "eFlags", EF_NODRAW as u32);
        let snap = snap_of(&[bomb, nade, late, hidden], 100);
        let frame = Frame {
            snap: &snap,
            prev: None,
            time_ms: 100,
            frac: 1.0,
            configstrings: &cs,
        };
        let (loops, missiles) = Cod2Sounds::default().loops(&frame, None);
        assert_eq!(
            loops.get(&80),
            Some(&(7, Some("bomb_tick".to_string()), Vec3::new(1.0, 2.0, 3.0)))
        );
        assert_eq!(loops.len(), 1);
        // No weapon file without an install: the key with no alias.
        assert_eq!(missiles.keys().copied().collect::<Vec<_>>(), [81]);
        assert_eq!(missiles[&81].0, 4);
    }

    /// With an install: the weapon arms pick each key, its `Player` twin
    /// for the first-person body, and fall back as `CG_EntityEvent` and
    /// `CG_FireWeapon` do (Mac 0x1e4650, 0x1d823c).
    #[test]
    fn stock_weapon_events_pick_retails_keys() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let names = [
            "bar_mp",
            "mosin_nagant_mp",
            "mosin_nagant_sniper_mp",
            "kar98k_mp",
            "frag_grenade_german_mp",
        ];
        let cs = table(&[(vcod_cod2::net::protocol::CS_WEAPONS, &names.join(" "))]);
        let third = snap_of(&[], 100);
        let mut first = snap_of(&[], 100);
        first.ps.pm_flags = PMF_FIRST_PERSON;
        first.ps.client_num = 3;
        let mut sounds = Cod2Sounds::default();
        let mut heard = |ev_num: i32, weapon: i32, snap: &SnapshotView| -> Vec<String> {
            let mut e = event(3, ev_num, 6);
            e.weapon = weapon;
            e.parm = weapon;
            sounds
                .cues(&e, snap, &cs, Some(&fs), Vec3::ZERO)
                .into_iter()
                .map(|c| c.alias)
                .collect()
        };
        let (bar, nagant, sniper, kar, frag) = (1, 2, 3, 4, 5);
        let one = |a: &str| vec![a.to_string()];
        assert_eq!(
            heard(ev::EV_RELOAD, bar, &third),
            one("weap_bar_reload_not_empty_npc")
        );
        assert_eq!(
            heard(ev::EV_RELOAD, bar, &first),
            one("weap_bar_reload_not_empty")
        );
        assert_eq!(
            heard(ev::EV_RELOAD_FROM_EMPTY, bar, &third),
            one("weap_bar_reload_npc")
        );
        assert_eq!(
            heard(ev::EV_RELOAD_FROM_EMPTY, bar, &first),
            one("weap_bar_reload")
        );
        // The nagant has no empty reload: both reloads play its one.
        assert_eq!(
            heard(ev::EV_RELOAD_FROM_EMPTY, nagant, &third),
            one("weap_nagant_reload_npc")
        );
        assert_eq!(
            heard(ev::EV_RELOAD_FROM_EMPTY, nagant, &first),
            one("weap_nagant_reload")
        );
        assert_eq!(
            heard(ev::EV_RECHAMBER_WEAPON, nagant, &third),
            one("weap_nagant_rechamber_npc")
        );
        assert_eq!(
            heard(ev::EV_RECHAMBER_WEAPON, nagant, &first),
            one("weap_nagant_rechamber")
        );
        assert_eq!(
            heard(ev::EV_RAISE_WEAPON, bar, &third),
            one("weap_bar_raise")
        );
        assert_eq!(
            heard(ev::EV_RAISE_WEAPON, nagant, &third),
            one("weap_raise")
        );
        assert_eq!(
            heard(ev::EV_PUTAWAY_WEAPON, nagant, &third),
            one("weap_putaway")
        );
        assert_eq!(
            heard(ev::EV_RELOAD_START, sniper, &third),
            one("weap_kar98k_sniper_reload_start_npc")
        );
        assert_eq!(
            heard(ev::EV_RELOAD_END, sniper, &first),
            one("weap_kar98k_sniper_reload_end")
        );
        assert_eq!(
            heard(ev::EV_ITEM_PICKUP, bar, &third),
            one("weap_bar_pickup")
        );
        assert_eq!(
            heard(ev::EV_AMMO_PICKUP, bar, &third),
            one("weap_bar_ammopickup")
        );
        assert_eq!(
            heard(ev::EV_AMMO_PICKUP, nagant, &third),
            one("weap_ammo_pickup")
        );
        assert_eq!(
            heard(ev::EV_FIRE_WEAPON, nagant, &third),
            one("weap_nagant_fire")
        );
        assert_eq!(
            heard(ev::EV_FIRE_WEAPON, nagant, &first),
            one("weap_nagant_fire_plr")
        );
        assert_eq!(
            heard(ev::EV_MELEE_SWIPE, nagant, &third),
            one("melee_swing_large")
        );
        // A clip-only weapon's empty click is silent.
        assert!(heard(ev::EV_NOAMMO, frag, &third).is_empty());
        assert_eq!(heard(ev::EV_NOAMMO, kar, &third), one("player_out_of_ammo"));
        // The stock frag names no `projExplosionSound`: the surface's alone.
        assert_eq!(
            heard(ev::EV_GRENADE_EXPLODE, frag, &third),
            one("grenade_explode_dirt")
        );
    }

    /// The weapon keys `CG_RegisterWeapon` (Mac 0x1d85cc) looks up as
    /// aliases. `loopFireSound`, `stopFireSound` and their twins (def
    /// +0xb0..+0xbc) are skipped: the 1.3 MP cgame plays neither.
    const WEAPON_ALIAS_KEYS: [&str; 27] = [
        "pickupSound",
        "ammoPickupSound",
        "projectileSound",
        "pullbackSound",
        "fireSound",
        "fireSoundPlayer",
        "lastShotSound",
        "lastShotSoundPlayer",
        "meleeSwipeSound",
        "rechamberSound",
        "rechamberSoundPlayer",
        "reloadSound",
        "reloadSoundPlayer",
        "reloadEmptySound",
        "reloadEmptySoundPlayer",
        "reloadStartSound",
        "reloadStartSoundPlayer",
        "reloadEndSound",
        "reloadEndSoundPlayer",
        "raiseSound",
        "altSwitchSound",
        "putawaySound",
        "noteTrackSoundA",
        "noteTrackSoundB",
        "noteTrackSoundC",
        "noteTrackSoundD",
        "projExplosionSound",
    ];

    /// The per-surface bases and fixed names of `CG_RegisterSounds` (Mac
    /// 0x144cf8).
    const SURFACE_BASES: [&str; 14] = [
        "grenade_bounce",
        "grenade_explode",
        "rocket_explode",
        "bullet_small",
        "bulletspray_small",
        "bullet_large",
        "step_run",
        "step_run_plr",
        "step_walk",
        "step_walk_plr",
        "step_prone",
        "step_prone_plr",
        "land",
        "land_plr",
    ];
    const FIXED_NAMES: [&str; 21] = [
        "player_out_of_ammo",
        "land_damage",
        "gear_rattle_run",
        "gear_rattle_plr_run",
        "gear_rattle_walk",
        "gear_rattle_plr_walk",
        "movement_foliage",
        "whizby",
        "melee_swing_large",
        "melee_swing_small",
        "melee_hit",
        "melee_hit_other",
        "weap_sniper_heartbeat",
        "weap_sniper_breathin",
        "weap_sniper_breathout",
        "weap_sniper_breathgasp",
        "weap_offhand_select",
        "weap_raise",
        "weap_putaway",
        "weap_pickup",
        "weap_ammo_pickup",
    ];

    /// The string literals a gsc hands the sound builtins, directly or
    /// through the variables the stock scripts route them by.
    fn gsc_sound_literals(text: &str) -> Vec<String> {
        const CALLS: [&str; 7] = [
            "playsound",
            "playsoundasmaster",
            "playlocalsound",
            "playloopsound",
            "playsoundonplayers",
            "ambientplay",
            "musicplay",
        ];
        const VARS: [&str; 9] = [
            "soundalias",
            "sound",
            "alliedsound",
            "axissound",
            "friendlyalias",
            "enemyalias",
            "scr_sound",
            "ambient_track",
            "sounds",
        ];
        let lit = |rest: &str| -> Option<String> {
            let rest = rest.trim_start().strip_prefix('"')?;
            Some(rest[..rest.find('"')?].to_string())
        };
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.split("//").next().unwrap_or("");
            let low = line.to_ascii_lowercase();
            for call in CALLS {
                let mut from = 0;
                while let Some(at) = low[from..].find(call) {
                    let at = from + at;
                    from = at + call.len();
                    let before = low[..at].chars().next_back();
                    if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                        continue;
                    }
                    if let Some(a) = low[from..]
                        .trim_start()
                        .strip_prefix('(')
                        .and_then(|_| {
                            line[from..].trim_start()[1..]
                                .trim_start()
                                .strip_prefix('"')
                        })
                        .and_then(|r| r.find('"').map(|e| r[..e].to_string()))
                    {
                        out.push(a);
                    }
                }
            }
            if let Some((lhs, rhs)) = line.split_once('=')
                && !rhs.starts_with('=')
                && !lhs.ends_with(['!', '<', '>', '='])
            {
                let lhs = lhs.trim().to_ascii_lowercase();
                let name = lhs.split('[').next().unwrap_or("");
                let name = name.rsplit(['.', ' ', '\t']).next().unwrap_or("");
                if VARS.iter().any(|v| name == *v || name.ends_with(v))
                    && let Some(a) = lit(rhs)
                {
                    out.push(a);
                }
            }
        }
        out
    }

    #[test]
    fn gsc_sound_literals_find_calls_and_routed_names() {
        let gsc = "\tplaySoundOnPlayers(\"MP_announcer_allies_win\", \"allies\");\n\
                   \tself playLocalSound( \"MP_hit_alert\" );\n\
                   \tsoundalias = \"US_mp_cmd_followme\";\n\
                   \tattacksounds[\"american\"] = \"US_mp_cmd_movein\";\n\
                   \tif(sound == \"x\") wait 1;\n\
                   \tself myplaysound(\"no\");\n";
        assert_eq!(
            gsc_sound_literals(gsc),
            [
                "MP_announcer_allies_win",
                "MP_hit_alert",
                "US_mp_cmd_followme",
                "US_mp_cmd_movein"
            ]
        );
    }

    /// With an install, the census of every alias the stock content names:
    /// `CG_RegisterSounds`' names, every stock weapon file's alias keys and
    /// every sound literal of the stock `maps/mp` scripts, each loaded on
    /// every stock map it can play on with all its rows' columns read and
    /// its loaded files decoding.
    #[test]
    fn stock_content_names_aliases_the_table_holds() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let all = crate::audio::cod2::load(&fs);
        assert!(all.errors.is_empty(), "{:#?}", all.errors);
        let mut maps: Vec<String> = fs
            .list_prefix("maps/mp/")
            .into_iter()
            .filter_map(|p| Some(p.strip_suffix(".d3dbsp")?["maps/mp/".len()..].to_string()))
            .collect();
        maps.sort();
        let tables: Vec<_> = maps.iter().map(|m| (m.clone(), all.for_map(m))).collect();
        let mut bank = crate::audio::bank::SoundBank::at_rate(0);
        let mut decoded = std::collections::HashSet::new();
        // Each name on each map it plays on: missing ones listed by map.
        let mut check = |name: &str, on: &dyn Fn(&str) -> bool| -> Vec<String> {
            let mut gone = Vec::new();
            for (map, t) in tables.iter().filter(|(m, _)| on(m)) {
                match t.get(name) {
                    None => gone.push(map.clone()),
                    Some(rows) => {
                        for r in rows.iter().filter(|r| !r.streamed) {
                            if decoded.insert(r.file.to_ascii_lowercase()) {
                                assert!(bank.get(&fs, &r.file).is_some(), "{name}: {}", r.file);
                            }
                        }
                    }
                }
            }
            gone
        };
        let every = |_: &str| true;
        // `CG_RegisterSounds`: retail leaves a missing one null, silent.
        let mut cgame_missing = Vec::new();
        let names = SURFACE_BASES
            .iter()
            .flat_map(|b| crate::play::cod2_events::SURFACES.map(|s| format!("{b}_{s}")))
            .chain(FIXED_NAMES.iter().map(|s| s.to_string()));
        for name in names {
            let gone = check(&name, &every);
            if !gone.is_empty() {
                assert_eq!(gone.len(), maps.len(), "{name} on some maps only: {gone:?}");
                cgame_missing.push(name);
            }
        }
        assert!(cgame_missing.is_empty(), "{cgame_missing:?}");
        // Every stock weapon's alias keys.
        let weapons: Vec<String> = fs
            .list_prefix("weapons/mp/")
            .into_iter()
            .filter_map(|p| {
                p.rsplit('/')
                    .next()
                    .filter(|n| !n.is_empty())
                    .map(str::to_string)
            })
            .collect();
        assert!(weapons.len() >= 40, "{weapons:?}");
        let mut keys_named = 0;
        for w in &weapons {
            let def = vcod_cod2::weapon::load(&fs, w).unwrap().def;
            for key in WEAPON_ALIAS_KEYS {
                let alias = def.str(key);
                if alias.is_empty() {
                    continue;
                }
                assert!(
                    !key.starts_with("noteTrackSound"),
                    "{w}: {key} {alias}: a notetrack sound ships"
                );
                keys_named += 1;
                let gone = check(alias, &every);
                assert!(gone.is_empty(), "{w}: {key} {alias} not on {gone:?}");
            }
        }
        // 266 on the 1.3 install.
        assert!(keys_named > 200, "{keys_named}");
        // The scripts: a map's own on its map, the rest on every map.
        let mut scripts: Vec<String> = fs
            .list_prefix("maps/mp/")
            .into_iter()
            .filter(|p| p.ends_with(".gsc"))
            .collect();
        scripts.sort();
        let mut named = std::collections::BTreeSet::new();
        let mut script_missing = Vec::new();
        for path in &scripts {
            let text = String::from_utf8_lossy(&fs.read(path).unwrap()).into_owned();
            let stem = path.rsplit('/').next().unwrap().trim_end_matches(".gsc");
            let own = maps
                .iter()
                .find(|m| stem == m.as_str() || stem.starts_with(&format!("{m}_")));
            for alias in gsc_sound_literals(&text) {
                let gone = match own {
                    Some(m) => check(&alias, &|x: &str| x == m),
                    None => check(&alias, &every),
                };
                if !gone.is_empty() {
                    script_missing.push(format!("{path}: {alias} not on {gone:?}"));
                }
                named.insert(alias);
            }
        }
        // 142 on the 1.3 install.
        assert!(named.len() > 100, "{named:?}");
        assert!(named.contains("MP_announcer_allies_win") && named.contains("ambient_africa"));
        assert!(script_missing.is_empty(), "{script_missing:#?}");
    }

    /// `CG_FireWeapon` plays from the shooter's world `tag_flash`, carried
    /// with him; a body without one, and the first-person body, play on
    /// the entity.
    #[test]
    fn fire_sounds_start_at_the_flash_tag() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let cs = table(&[(vcod_cod2::net::protocol::CS_WEAPONS, "kar98k_mp")]);
        let third = snap_of(&[], 100);
        let mut first = snap_of(&[], 100);
        first.ps.pm_flags = PMF_FIRST_PERSON;
        first.ps.client_num = 3;
        let flash = Vec3::new(10.0, 20.0, 50.0);
        let mut sounds = Cod2Sounds::default();
        let mut e = event(3, ev::EV_FIRE_WEAPON, 0);
        e.weapon = 1;
        e.pos = Vec3::new(10.0, 0.0, 0.0);
        let source = |sounds: &mut Cod2Sounds, snap: &SnapshotView| {
            let c = sounds.cues(&e, snap, &cs, Some(&fs), Vec3::ZERO);
            assert_eq!(c.len(), 1, "{c:?}");
            c[0].source
        };
        let body = Source::Entity { num: 3, pos: e.pos };
        assert_eq!(source(&mut sounds, &third), body);
        sounds.muzzles.insert(3, flash);
        assert_eq!(
            source(&mut sounds, &third),
            Source::Tag { num: 3, pos: flash }
        );
        assert_eq!(source(&mut sounds, &first), body);
    }

    /// The whizby starts at the shooter's drawn `tag_flash` when he has
    /// one, else at his stance point.
    #[test]
    fn the_whizby_starts_at_the_drawn_muzzle() {
        let mut shooter = EntityState2::null(2);
        put(&mut shooter, Vec3::new(0.0, 0.0, -60.0));
        let snap = snap_of(&[shooter], 100);
        let mut e = event(80, ev::EV_BULLET_HIT_SMALL, 5);
        e.etype = ev::ET_EVENTS + e.event;
        e.other_entity = 2;
        e.pos = Vec3::new(1000.0, 0.0, 0.0);
        let cs = ConfigStrings::default();
        let view = Vec3::new(500.0, 100.0, 0.0);
        let mut sounds = Cod2Sounds::default();
        let whiz = |sounds: &mut Cod2Sounds| {
            sounds
                .cues(&e, &snap, &cs, None, view)
                .into_iter()
                .find(|c| c.alias == "whizby")
                .map(|c| c.source.pos())
        };
        assert_eq!(whiz(&mut sounds), Some(Vec3::new(484.0, 0.0, 0.0)));
        // A flash 200 units along the shot: still 16 back from the closest
        // point, which has not moved.
        sounds.muzzles.insert(2, Vec3::new(200.0, 0.0, 0.0));
        assert_eq!(whiz(&mut sounds), Some(Vec3::new(484.0, 0.0, 0.0)));
        // One past the eye's margin: no whizby.
        sounds.muzzles.insert(2, Vec3::new(450.0, 0.0, 0.0));
        assert_eq!(whiz(&mut sounds), None);
    }

    /// `CG_SoundBlend`: the two aliases at `eventParms[0]` and `[1]`, the
    /// blend `leanf` lerped across the snapshot pair; a missing alias
    /// plays nothing.
    #[test]
    fn sound_blend_entities_lerp_their_blend() {
        let cs = table(&[
            (CS_SOUND_ALIASES + 2, "engine_low"),
            (CS_SOUND_ALIASES + 3, "engine_high"),
        ]);
        let blend = |num: u32, lean: f32, second: u32| {
            let mut b = EntityState2::null(num);
            set(&mut b, "eType", ev::ET_SOUND_BLEND as u32);
            set(&mut b, "eventParms[0]", 2);
            set(&mut b, "eventParms[1]", second);
            set(&mut b, "leanf", lean.to_bits());
            put(&mut b, Vec3::new(1.0, 2.0, 3.0));
            b
        };
        let prev = snap_of(&[blend(70, 0.2, 3), blend(71, 0.0, 0)], 50);
        let snap = snap_of(&[blend(70, 0.6, 3), blend(71, 0.0, 0)], 100);
        let frame = Frame {
            snap: &snap,
            prev: Some(&prev),
            time_ms: 75,
            frac: 0.5,
            configstrings: &cs,
        };
        let got = Cod2Sounds::blends(&frame);
        assert_eq!(got.len(), 1);
        let b = &got[0];
        assert_eq!(b.entity, 70);
        assert_eq!(
            b.aliases,
            ["engine_low".to_string(), "engine_high".to_string()]
        );
        assert!((b.blend - 0.4).abs() < 1e-6, "{}", b.blend);
        assert_eq!((b.pitch_scale, b.pos), (1.0, Vec3::new(1.0, 2.0, 3.0)));
    }

    /// `CG_StartAmbient`'s fade is the time left to `t`; `setReverb` and
    /// `deactivateReverb` read as `CG_ServerCommand`'s `r` and `D`.
    #[test]
    fn ambient_fade_and_reverb_commands_read_as_retail() {
        let cs = table(&[(3, "n\\ambient_france\\t\\3000")]);
        let (text, alias, fade) = ambient(&cs, 1000);
        assert_eq!(text, "n\\ambient_france\\t\\3000");
        assert_eq!((alias.as_deref(), fade), (Some("ambient_france"), 2000));
        assert_eq!(ambient(&cs, 4000).2, 0);
        assert_eq!(ambient(&cs, 0).2, 0);
        // `ambientStop(2)`: no alias, a fade.
        let stop = table(&[(3, "t\\3000")]);
        assert_eq!(ambient(&stop, 1000).1, None);
        assert_eq!(
            sound_command("r 1 \"stonecorridor\" 1 0.5 1.5", &cs),
            Some(SoundCommand::Reverb {
                prio: 1,
                room: "stonecorridor".into(),
                dry: 1.0,
                wet: 0.5,
                fade_ms: 1500,
            })
        );
        assert_eq!(sound_command("r 1 \"room\" 1 0.5", &cs), None);
        assert_eq!(
            sound_command("D 2 0.25", &cs),
            Some(SoundCommand::ReverbOff {
                prio: 2,
                fade_ms: 250
            })
        );
        assert_eq!(sound_command("D 2", &cs), None);
    }
}
