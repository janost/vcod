//! The CoD2 view weapon: the hands and gun a CoD2 playerstate holds, their
//! clips driven off `weapAnim` and `fWeaponPosFrac` as the 1.3 cgame drives
//! its view tree (docs/research/cod2-weapons.md, "The view weapon"). CoD1's
//! rig is `crate::viewmodel`; this one shares its skeleton and pose code,
//! not its weapon state machine.
//!
//! The gun's place off the view (offsets, bob, sway, lean, the idle drift)
//! is `play::cod2_gun`'s, and the scope overlay's hiding of the gun the
//! caller's (`play::cod2_view::ViewOut::overlay`); this module hands back
//! the gun at rest. The two sight clips crossfade over 0.5 s
//! ([`AdsBlend`]). Not modelled: the crossfade between main clips,
//! notetrack sounds, the flash and brass tags.

use std::collections::HashMap;
use std::sync::Arc;
use vcod_common::asset_source::AssetSource;

use glam::{Mat4, Vec3, Vec4};
use vcod_cod2::net::protocol::CS_WEAPONS;
use vcod_cod2::net::{ConfigStrings, Frame, PlayerView};
use vcod_cod2::weapon::{self, WeaponDef, node};
use vcod_common::{model, skeleton, xanim};

use crate::renderer::VmDraw;
use crate::viewmodel::ViewModels;

/// `pm_flags`: the binoculars are up (`BG_GetViewmodelWeaponIndex`, Mac
/// 0x7b226).
const PMF_BINOCULARS: i32 = 0x20000;
/// `pm_flags`: the off-hand weapon is out; also drops the sight layer
/// (`CG_WeaponRunXModelAnims`, Mac 0x1dc3cc).
const PMF_OFFHAND: i32 = 0x10;
/// `pm_flags`: the sight is up.
const PMF_SIGHT: i32 = 0x40;
/// `eFlags`: on a turret; no view weapon (`CG_AddViewWeapon`, Mac 0x1da2f8).
const EF_TURRET: i32 = 0x300;
/// `weaponstate` reloading, under which the sight drops once `weaponTime`
/// is past `adsReloadTransTime`.
const WEAPON_RELOADING: i32 = 5;
/// `pm_type` 4 (spectator) and 5 (intermission) draw no view weapon.
const PM_SPECTATOR: i32 = 4;

/// One weapon's hands and gun, its skeleton and its 22 clips by tree node.
pub struct Rig {
    pub models: ViewModels,
    pub skeleton: Arc<skeleton::Skeleton>,
    pub def: Arc<WeaponDef>,
    /// Indexed by node; 0 (the root) is `None`, as is a clip that failed.
    clips: Vec<Option<(Arc<xanim::XAnim>, skeleton::AnimBinding)>>,
    /// Each node's rate ([`WeaponDef::view_rate`]).
    rates: Vec<f32>,
}

impl Rig {
    fn clip(&self, n: usize) -> Option<&(Arc<xanim::XAnim>, skeleton::AnimBinding)> {
        self.clips.get(n)?.as_ref()
    }

    /// The node's clip has run its length: never for a looping clip.
    fn finished(&self, n: usize, t: f32) -> bool {
        self.clip(n)
            .is_none_or(|(c, _)| !c.looping && t >= c.duration())
    }
}

/// `CG_RegisterWeapon` (Mac 0x1d85cc) for `name`: the hands (`hands`, an
/// `xmodel/` name, or the file's `handModel`) and the file's `gunModel` at
/// the hands' `tag_weapon`, and every anim key's clip, an empty key
/// playing idle's. `Err` when the file, a model or idle is missing.
pub fn load_rig(fs: &dyn AssetSource, name: &str, hands: Option<&str>) -> anyhow::Result<Rig> {
    let def = weapon::load(fs, name)?.def;
    let gun = def.str("gunModel");
    anyhow::ensure!(!gun.is_empty(), "{name}: no gunModel");
    let hands = match hands {
        Some(h) => h.strip_prefix("xmodel/").unwrap_or(h).to_string(),
        None => def.str("handModel").to_string(),
    };
    anyhow::ensure!(!hands.is_empty(), "{name}: no hand model");
    anyhow::ensure!(!def.str("idleAnim").is_empty(), "{name}: no idle anim");
    let hands = model::load(fs, &hands)?;
    let gun = model::load(fs, gun)?;
    let skeleton = skeleton::Skeleton::build_grafted(&[(&hands, None), (&gun, Some("tag_weapon"))]);
    let mut loaded: HashMap<String, Option<Arc<xanim::XAnim>>> = HashMap::new();
    let mut clips = vec![None];
    let mut rates = vec![1.0];
    for n in 1..=weapon::VIEW_ANIM_KEYS.len() {
        let anim = def.view_anim(n);
        let clip = loaded
            .entry(anim.to_string())
            .or_insert_with(|| match xanim::load(fs, anim) {
                Ok(a) => Some(Arc::new(a)),
                Err(e) => {
                    log::warn!("{name}: xanim {anim}: {e:#}");
                    None
                }
            })
            .clone();
        let ms = clip.as_ref().map_or(0, |c| (c.duration() * 1000.0) as i32);
        rates.push(def.view_rate(n, ms));
        clips.push(clip.map(|c| {
            let b = skeleton.bind(&c);
            (c, b)
        }));
    }
    anyhow::ensure!(clips[node::IDLE].is_some(), "{name}: idle did not load");
    Ok(Rig {
        models: vec![hands, gun].into(),
        skeleton: Arc::new(skeleton),
        def: Arc::new(def),
        clips,
        rates,
    })
}

/// The view weapon's main node and its clock, as `CG_WeaponRunXModelAnims`
/// moves them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewAnim {
    pub node: usize,
    /// Seconds into the node's clip.
    pub t: f32,
    /// The `weapAnim` last acted on; `None` while a 0 waits for the running
    /// clip to finish.
    last: Option<i32>,
}

impl Default for ViewAnim {
    fn default() -> Self {
        ViewAnim {
            node: node::IDLE,
            t: 0.0,
            last: None,
        }
    }
}

impl ViewAnim {
    /// Advances `dt` seconds under `weap_anim`. A changed number restarts
    /// its node; 0 waits for the running clip and then plays idle, or empty
    /// idle with the clip empty.
    pub fn step(&mut self, rig: &Rig, weap_anim: i32, clip_empty: bool, dt: f32) {
        if self.last != Some(weap_anim) {
            match weapon::view_node(weap_anim) {
                Some(n) => {
                    *self = ViewAnim {
                        node: n,
                        t: 0.0,
                        last: Some(weap_anim),
                    }
                }
                None if self.node < node::MAIN_END
                    && !matches!(self.node, node::IDLE | node::EMPTY_IDLE)
                    && !rig.finished(self.node, self.t) =>
                {
                    self.last = None;
                }
                None => {
                    let n = if clip_empty {
                        node::EMPTY_IDLE
                    } else {
                        node::IDLE
                    };
                    *self = ViewAnim {
                        node: n,
                        t: 0.0,
                        last: Some(weap_anim),
                    }
                }
            }
        }
        self.t += dt * rig.rates.get(self.node).copied().unwrap_or(1.0);
    }
}

/// One node's goal weight as `XAnimSetGoalWeightInternal` (Mac 0x3b59e)
/// sets it and `XAnimUpdateOldTime` (Mac 0x3b1b0) steps it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Goal {
    weight: f32,
    goal: f32,
    /// Seconds left to the goal.
    left: f32,
}

impl Goal {
    /// A goal of `goal` over `time` seconds. A node at 0 starts at a
    /// thousandth of its goal; a fall to 0 takes the whole time unless one
    /// already ends sooner; any other goal the time times the share of the
    /// way left, of the old weight going down, of the goal going up.
    fn set(&mut self, goal: f32, time: f32) {
        if time != 0.0 && self.weight == 0.0 {
            self.weight = goal * 0.001;
        }
        if goal == 0.0 {
            if self.goal != 0.0 || time < self.left {
                self.left = time;
                self.goal = 0.0;
            }
            return;
        }
        let w = self.weight;
        self.left = if goal < w {
            time * (w - goal) / w
        } else {
            time * (goal - w) / goal
        };
        self.goal = goal;
    }

    /// `dt` seconds on: the goal outright once its time runs out, else the
    /// rest of the way over the time left, a weight under 1e-6 put back at
    /// a thousandth of the goal.
    fn step(&mut self, dt: f32) {
        if self.left <= dt {
            self.weight = self.goal;
            self.left = 0.0;
            return;
        }
        self.weight += (self.goal - self.weight) * dt / self.left;
        if self.weight < 1e-6 {
            self.weight = self.goal * 0.001;
        }
        self.left -= dt;
    }
}

/// The sight layer's two nodes, ADS up (21) and ADS down (22):
/// `CG_PlayADSAnim` (Mac 0x1d7bc0) sets the wanted one's goal to 1 and the
/// other's to 0, each over 0.5 s, every frame. A node no goal has touched
/// holds no anim info and a goal of 0 leaves it so (the setter's early
/// return), so a fresh tree's first frame raises one node alone.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AdsBlend {
    nodes: [Option<Goal>; 2],
}

impl AdsBlend {
    /// One frame of `dt` seconds with the sight wanted up or down.
    pub fn step(&mut self, up: bool, dt: f32) {
        let goals = if up { [1.0, 0.0] } else { [0.0, 1.0] };
        for (n, goal) in self.nodes.iter_mut().zip(goals) {
            if n.is_none() && goal == 0.0 {
                continue;
            }
            let g = n.get_or_insert_with(Goal::default);
            g.set(goal, 0.5);
            g.step(dt);
        }
    }

    /// ADS up's and ADS down's weights.
    pub fn weights(&self) -> [f32; 2] {
        self.nodes.map(|n| n.map_or(0.0, |g| g.weight))
    }
}

/// The sight fraction the frame draws: a predicted playerstate's own,
/// else lerped between the two snapshots as `CG_InterpolatePlayerState`
/// (Mac 0x1dd8b2) lerps `fWeaponPosFrac` (cod2-hud.md 18.1).
pub fn pos_frac(frame: &Frame, predicted: bool) -> f32 {
    let ps = &frame.snap.ps;
    match frame.prev {
        Some(p) if !predicted && p.ps.client_num == ps.client_num => {
            let a = p.ps.raw.f32("fWeaponPosFrac");
            a + (ps.raw.f32("fWeaponPosFrac") - a) * frame.frac
        }
        _ => ps.raw.f32("fWeaponPosFrac"),
    }
}

/// The sight layer is up: `pm_flags` 0x40 without the off hand, except
/// while a reload's `weaponTime` is still past `adsReloadTransTime`
/// (`CG_WeaponRunXModelAnims`).
pub fn sight_up(ps: &PlayerView, def: &WeaponDef) -> bool {
    let reloading = ps.raw.i32("weaponstate") == WEAPON_RELOADING
        && ps.raw.i32("weaponTime") > def.int("adsReloadTransTime");
    !reloading && ps.pm_flags & PMF_SIGHT != 0 && ps.pm_flags & PMF_OFFHAND == 0
}

/// `BG_GetViewmodelWeaponIndex`: the first binoculars weapon under
/// `pm_flags` 0x20000, the off hand's under 0x10, else `ps.weapon`.
/// `binoculars` is that first weapon's index, 0 for none.
pub fn view_weapon_index(ps: &PlayerView, binoculars: i32) -> i32 {
    if ps.pm_flags & PMF_BINOCULARS != 0 {
        binoculars
    } else if ps.pm_flags & PMF_OFFHAND != 0 {
        ps.raw.i32("offHandIndex")
    } else {
        ps.weapon
    }
}

/// Poses `rig` for one frame: the sight layer (`adsUpAnim` at `frac` and
/// `adsDownAnim` at `1 - frac`, as `CG_PlayADSAnim` sets their times,
/// blended by `ads`, [`AdsBlend::weights`]) for a weapon with
/// `aimDownSight`, then the main node over it.
pub fn pose(rig: &Rig, pose: &mut skeleton::PoseBuffer, anim: &ViewAnim, ads: [f32; 2], frac: f32) {
    if rig.def.flag("aimDownSight") {
        let layers: Vec<skeleton::ClipAt> = [(node::ADS_UP, frac), (node::ADS_DOWN, 1.0 - frac)]
            .into_iter()
            .zip(ads)
            .filter(|&(_, w)| w > 0.0)
            .filter_map(|((n, t), w)| {
                let (c, b) = rig.clip(n)?;
                let t = t.clamp(0.0, 1.0) * c.duration();
                Some((&**c, b, c.frame_pos(t, false), w))
            })
            .collect();
        match layers.as_slice() {
            [] => {}
            [(c, b, t, _)] => pose.apply(c, b, *t),
            _ => pose.blend(&layers),
        }
    }
    let (c, b) = rig
        .clip(anim.node)
        .or_else(|| rig.clip(node::IDLE))
        .expect("a rig holds idle");
    pose.apply(c, b, c.frame_pos(anim.t, c.looping));
}

/// `rig` at rest in idle, lit from `light_origin`: what a fresh
/// [`Cod2View::frame`] hands the renderer.
#[cfg(test)]
pub fn idle_draw(rig: &Rig, light_origin: Vec3) -> VmDraw {
    let mut p = skeleton::PoseBuffer::new(&rig.skeleton);
    pose(rig, &mut p, &ViewAnim::default(), [0.0, 1.0], 0.0);
    VmDraw {
        transform: view_axes(),
        light_origin,
        fov_x: 0.0,
        bone_sets: (0..rig.models.len())
            .map(|m| p.skin_matrices(&rig.skeleton, m))
            .collect(),
    }
}

/// The model's axes in view space (X right, Y up, -Z forward): its forward
/// down the view, its left to the view's left.
fn view_axes() -> Mat4 {
    Mat4::from_cols(
        Vec4::new(0.0, 0.0, -1.0, 0.0),
        Vec4::new(-1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::W,
    )
}

/// A rig's weapon and hands names.
type RigKey = (String, Option<String>);

/// The view models a level can draw, loaded for the renderer's preload
/// (`Renderer::preload_viewmodel`) at its gamestate, so no weapon's first
/// showing uploads its models in the frame (cod2-client-perf.md, "The
/// view weapon's first draw"). `CG_RegisterWeapon` (Mac 0x1d85cc)
/// registers each configstring 7 weapon's `gunModel` and `handModel`;
/// the hands `setViewmodel` names are configstring models
/// `CG_RegisterGraphics` registers, all `xmodel/viewmodel_*` in the
/// stock character scripts. Each name once; one that does not load is
/// left to the draw, which warns.
pub fn level_view_models(cs: &ConfigStrings, fs: &dyn AssetSource) -> Vec<model::Model> {
    let mut names: Vec<String> = Vec::new();
    for w in (1..).map_while(|i| cs.weapon_name(i)) {
        let Ok(def) = weapon::load(fs, w).map(|p| p.def) else {
            continue;
        };
        names.extend(
            [def.str("gunModel"), def.str("handModel")]
                .into_iter()
                .filter(|n| !n.is_empty())
                .map(str::to_string),
        );
    }
    names.extend(
        (1..256)
            .map_while(|i| cs.model(i))
            .filter_map(|n| n.strip_prefix("xmodel/"))
            .filter(|n| n.starts_with("viewmodel_"))
            .map(str::to_string),
    );
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.to_ascii_lowercase()));
    names
        .iter()
        .filter_map(|n| model::load(fs, n).ok())
        .collect()
}

/// What a CoD2 session's view weapon holds across frames: the loaded rigs
/// and the drawn one's pose and clock.
#[derive(Default)]
pub struct Cod2View {
    rigs: HashMap<(String, Option<String>), Option<Arc<Rig>>>,
    /// The drawn rig's key, its pose and its clock.
    current: Option<(RigKey, skeleton::PoseBuffer, ViewAnim, AdsBlend)>,
    last_ms: Option<i32>,
    table: WeaponTable,
}

/// One frame's view weapon: the models when they changed since the last
/// frame (for `Renderer::set_viewmodel`) and the draw, fov left to the
/// caller.
pub struct ViewFrame {
    pub models: Option<ViewModels>,
    pub draw: VmDraw,
}

impl Cod2View {
    /// The drawn view weapon's name.
    #[cfg(test)]
    pub(crate) fn drawn(&self) -> Option<&str> {
        self.current.as_ref().map(|((w, _), ..)| w.as_str())
    }

    /// Rigs loaded so far, misses included.
    #[cfg(test)]
    pub(crate) fn rigs_loaded(&self) -> usize {
        self.rigs.len()
    }

    fn rig(&mut self, fs: &dyn AssetSource, key: &(String, Option<String>)) -> Option<Arc<Rig>> {
        self.rigs
            .entry(key.clone())
            .or_insert_with(|| match load_rig(fs, &key.0, key.1.as_deref()) {
                Ok(r) => Some(Arc::new(r)),
                Err(e) => {
                    log::warn!("cod2 viewmodel: {e:#}");
                    None
                }
            })
            .clone()
    }

    /// The weapon and hands `ps` draws, `None` when it draws none.
    fn key(
        ps: &PlayerView,
        cs: &ConfigStrings,
        binoculars: i32,
    ) -> Option<(String, Option<String>)> {
        if ps.pm_type >= PM_SPECTATOR || ps.eflags & EF_TURRET != 0 {
            return None;
        }
        let weapon = cs.weapon_name(view_weapon_index(ps, binoculars))?;
        let hands = cs.model(ps.raw.i32("viewmodelIndex")).map(str::to_string);
        Some((weapon.to_string(), hands))
    }

    /// Steps and poses the view weapon `frame`'s playerstate holds;
    /// `predicted` when that playerstate is the frame's prediction.
    pub fn frame(
        &mut self,
        frame: &Frame,
        fs: &dyn AssetSource,
        predicted: bool,
    ) -> Option<ViewFrame> {
        let dt = self
            .last_ms
            .map_or(0, |l| (frame.time_ms - l).clamp(0, 1000));
        self.last_ms = Some(frame.time_ms);
        let ps = &frame.snap.ps;
        self.table.sync(frame.configstrings, fs);
        let Some(key) = Self::key(ps, frame.configstrings, self.table.binoculars) else {
            self.current = None;
            return None;
        };
        let rig = self.rig(fs, &key)?;
        let mut models = None;
        if self.current.as_ref().is_none_or(|(k, ..)| *k != key) {
            let mut p = skeleton::PoseBuffer::new(&rig.skeleton);
            // Seeded with idle, so a first clip keying few bones holds arms.
            let (c, b) = rig.clip(node::IDLE).expect("a rig holds idle");
            p.apply(c, b, 0.0);
            self.current = Some((key, p, ViewAnim::default(), AdsBlend::default()));
            models = Some(rig.models.clone());
        }
        let (_, pose_buf, anim, ads) = self.current.as_mut().expect("set above");
        let weapon = view_weapon_index(ps, self.table.binoculars);
        let clip = usize::try_from(weapon)
            .ok()
            .and_then(|w| self.table.clips.get(w));
        let clip_empty = clip.is_some_and(|&c| ps.raw.ammoclip.get(c) == Some(&0));
        anim.step(&rig, ps.raw.i32("weapAnim"), clip_empty, dt as f32 / 1000.0);
        let frac = pos_frac(frame, predicted);
        ads.step(sight_up(ps, &rig.def), dt as f32 / 1000.0);
        pose(&rig, pose_buf, anim, ads.weights(), frac);
        let bone_sets = (0..rig.models.len())
            .map(|m| pose_buf.skin_matrices(&rig.skeleton, m))
            .collect();
        Some(ViewFrame {
            models,
            draw: VmDraw {
                transform: view_axes(),
                light_origin: Vec3::from(frame.eye()),
                fov_x: 0.0,
                bone_sets,
            },
        })
    }
}

/// Configstring 7's weapons as the cgame's `bg_weaponDefs` index them: each
/// one's clip index and whether it is binoculars, rebuilt when the string
/// changes.
#[derive(Default)]
struct WeaponTable {
    cs7: String,
    /// Index 0 is the default weapon's slot.
    clips: Vec<usize>,
    /// The first `weaponType` binoculars weapon, 0 for none.
    binoculars: i32,
}

impl WeaponTable {
    fn sync(&mut self, cs: &ConfigStrings, fs: &dyn AssetSource) {
        let cs7 = cs.get(CS_WEAPONS);
        if self.cs7 == cs7 && !self.clips.is_empty() {
            return;
        }
        let defs: Vec<Option<WeaponDef>> = (1..)
            .map_while(|i| cs.weapon_name(i))
            .map(|n| weapon::load(fs, n).ok().map(|p| p.def))
            .collect();
        let names = defs
            .iter()
            .map(|d| d.as_ref().map_or("", |d| d.str("clipName")));
        self.clips = std::iter::once(0)
            .chain(weapon::clip_indices(names))
            .collect();
        self.binoculars = defs
            .iter()
            .position(|d| {
                d.as_ref()
                    .is_some_and(|d| d.weap_type() == weapon::WEAPTYPE_BINOCULARS)
            })
            .map_or(0, |i| i as i32 + 1);
        self.cs7 = cs7.to_string();
    }
}

/// The drawn playerstate's view angles, lerped the short way between the
/// two snapshots of the same client, degrees.
pub fn view_angles(frame: &Frame) -> [f32; 3] {
    let b = frame.snap.ps.viewangles;
    match frame.prev {
        Some(p) if p.ps.client_num == frame.snap.ps.client_num => {
            let a = p.ps.viewangles;
            [0, 1, 2].map(|i| {
                let d = (b[i] - a[i] + 180.0).rem_euclid(360.0) - 180.0;
                a[i] + d * frame.frac
            })
        }
        _ => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::pk3::Pk3Fs;
    use vcod_common::testing::cod2_fs;

    /// A followed body's sight lerps between its two snapshots, as
    /// `CG_InterpolatePlayerState` lerps `snap + 0xe8` (ps+0xdc); a
    /// predicted one draws its own.
    #[test]
    fn the_sight_fraction_lerps_unless_predicted() {
        use vcod_cod2::net::delta::field_index;
        use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;
        use vcod_cod2::net::{PlayerState2, SnapshotView};
        let snap = |pos: f32| {
            let mut raw = PlayerState2::default();
            raw.f[field_index(&PLAYER_STATE_FIELDS, "fWeaponPosFrac")] = pos.to_bits();
            SnapshotView {
                message_num: 0,
                server_time: 0,
                snap_flags: 0,
                ps: PlayerView::new(&raw, 0),
                entities: Vec::new(),
                clients: Vec::new(),
            }
        };
        let (prev, next) = (snap(0.2), snap(0.6));
        let cs = ConfigStrings::default();
        let f = Frame {
            snap: &next,
            prev: Some(&prev),
            time_ms: 0,
            frac: 0.25,
            configstrings: &cs,
        };
        assert!((pos_frac(&f, false) - 0.3).abs() < 1e-6);
        assert_eq!(pos_frac(&f, true), 0.6);
        let first = Frame { prev: None, ..f };
        assert_eq!(pos_frac(&first, false), 0.6);
    }

    /// The sight clips crossfade linearly over 0.5 s; a fresh tree raises
    /// the wanted node alone, from a thousandth; a turn back mid-fade
    /// raises the other the rest of the way in its share of the time and
    /// drops the first over the whole time.
    #[test]
    fn the_sight_clips_crossfade_over_half_a_second() {
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 3e-3 && (a[1] - b[1]).abs() < 3e-3;
        let mut b = AdsBlend::default();
        b.step(false, 0.0);
        assert_eq!(b.weights(), [0.0, 0.001]);
        for _ in 0..50 {
            b.step(false, 0.01);
        }
        assert!(close(b.weights(), [0.0, 1.0]), "{:?}", b.weights());
        for _ in 0..25 {
            b.step(true, 0.01);
        }
        assert!(close(b.weights(), [0.5, 0.5]), "{:?}", b.weights());
        // Back down at the halfway mark: ADS down climbs the last half in
        // 0.25 s, ADS up falls from 0.5 over the whole 0.5 s.
        for _ in 0..25 {
            b.step(false, 0.01);
        }
        assert!(close(b.weights(), [0.25, 1.0]), "{:?}", b.weights());
        for _ in 0..26 {
            b.step(false, 0.01);
        }
        assert!(close(b.weights(), [0.0, 1.0]), "{:?}", b.weights());
    }

    fn fs() -> Option<Pk3Fs> {
        let fs = cod2_fs()?;
        vcod_cod2::model::register();
        Some(fs)
    }

    /// Every stock weapon with a gun assembles: hands and gun load, the
    /// hands' bind is rebuilt off the origin, every node's clip samples to
    /// finite skinning over its length, and at rest every gun sits ahead
    /// of, right of and below the eye.
    #[test]
    fn every_stock_weapon_assembles_and_animates() {
        let Some(fs) = fs() else {
            return;
        };
        let mut built = 0;
        for path in fs.list_prefix("weapons/mp/") {
            let name = path.rsplit('/').next().unwrap();
            if name.is_empty() {
                continue;
            }
            let def = weapon::load(&fs, name).unwrap().def;
            if def.str("gunModel").is_empty() {
                continue;
            }
            let rig = load_rig(&fs, name, None).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let hands = &rig.models[0];
            for m in rig.models.iter() {
                // every stock hands model carries 69 bones, past CoD1's 64
                let n = m.bones.len();
                assert!(n <= crate::renderer::VM_BONE_COUNT, "{name}: {n} bones");
            }
            let spread = hands
                .bones
                .iter()
                .map(|b| b.pos.length())
                .fold(0f32, f32::max);
            assert!(spread > 5.0, "{name}: the hands' bind stayed at the origin");
            let mut p = skeleton::PoseBuffer::new(&rig.skeleton);
            for n in 1..rig.clips.len() {
                let Some((c, b)) = rig.clip(n) else {
                    panic!("{name}: node {n} ({}) has no clip", def.view_anim(n));
                };
                for i in 0..=4 {
                    let t = c.duration() * i as f32 / 4.0;
                    p.apply(c, b, c.frame_pos(t, c.looping));
                    for (m, model) in rig.models.iter().enumerate() {
                        let mats = p.skin_matrices(&rig.skeleton, m);
                        assert_eq!(mats.len(), model.bones.len());
                        assert!(mats.iter().all(|m| m.is_finite()), "{name} node {n}");
                    }
                }
            }
            pose(&rig, &mut p, &ViewAnim::default(), [0.0, 1.0], 0.0);
            let bi = rig.skeleton.bone_index("tag_weapon").expect("tag_weapon");
            let at = p.bone_world(&rig.skeleton, bi).0;
            // A grenade's idle holds it low at the side; the placeholder
            // default weapon sits left.
            let held = def.weap_class() != 5 && name != "defaultweapon_mp";
            assert!(
                !held || (at.x > 2.0 && at.y < -1.0 && at.z < -2.0),
                "{name}: tag_weapon at rest {at}"
            );
            built += 1;
        }
        assert!(built >= 35, "only {built} stock weapons built a rig");
    }

    /// A shot plays fire, a 0 lets it run out before idle comes back, and a
    /// reload's clip is stretched over `reloadTime`.
    #[test]
    fn weap_anim_drives_the_nodes() {
        let Some(fs) = fs() else {
            return;
        };
        let rig = load_rig(&fs, "kar98k_mp", None).unwrap();
        let mut a = ViewAnim::default();
        a.step(&rig, 0, false, 0.1);
        assert_eq!(a.node, node::IDLE);
        a.step(&rig, 2, false, 0.05);
        assert_eq!((a.node, a.t), (node::FIRE, 0.05));
        a.step(&rig, 0, false, 0.05);
        assert_eq!(a.node, node::FIRE, "fire runs out first");
        let fire = rig.clip(node::FIRE).unwrap().0.duration();
        for _ in 0..(fire / 0.05) as usize + 2 {
            a.step(&rig, 0, false, 0.05);
        }
        assert_eq!(a.node, node::IDLE);
        a.step(&rig, 0, true, 0.05);
        assert_eq!(a.node, node::IDLE, "a held 0 does not restart");
        a.step(&rig, 2 | weapon::ANIM_TOGGLEBIT, true, 0.0);
        a.step(&rig, 0, true, 10.0);
        a.step(&rig, 0, true, 0.0);
        assert_eq!(a.node, node::EMPTY_IDLE);
        a.step(&rig, 11, false, 0.5);
        let reload = rig.clip(node::RELOAD).unwrap().0.duration();
        let rate = reload * 1000.0 / rig.def.int("reloadTime") as f32;
        assert!((a.t - 0.5 * rate).abs() < 1e-3, "{} vs {}", a.t, 0.5 * rate);
    }
}
