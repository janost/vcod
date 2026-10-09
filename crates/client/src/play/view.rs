//! The playing client's own first-person weapon: the rig for `ps.weapon`
//! with the hands `ps.viewmodelIndex` names, posed from `ps.weapAnim`.

use crate::hud::scope;
use crate::renderer::VmDraw;
use crate::viewmodel::{self, ViewWeapon, ViewmodelMotion};
use glam::Vec3;
use vcod_common::net::msg;
use vcod_common::net::protocol::{CS_MODELS_V1, ENTITYNUM_NONE, Protocol};
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::predict::Predicted;
use vcod_common::pmove::weapon::NUM_AMMO;
use vcod_common::weapon::{self, SightDirection, ViewAnimClock, WeaponAnim, WeaponDef};

/// The playerstate fields the viewmodel reads, from the prediction when there
/// is one, else from the newest snapshot.
pub struct ViewPs {
    pub weapon: u8,
    pub viewmodel_index: i32,
    pub weap_anim: i32,
    pub ads_frac: f32,
    pub ammoclip: [i16; NUM_AMMO],
    pub velocity: Vec3,
    pub on_ground: bool,
    pub pm_type: i32,
    /// On a mounted gun: `eFlags & 0xc000` on the wire.
    pub mounted: bool,
}

/// `pm_type` at intermission.
use vcod_common::net::flags::PM_INTERMISSION;

impl ViewPs {
    /// `viewmodel_index` comes from the snapshot: prediction does not carry it.
    pub fn from_predicted(pred: &Predicted, viewmodel_index: i32) -> ViewPs {
        ViewPs {
            weapon: pred.ps.weapon,
            viewmodel_index,
            weap_anim: pred.ps.weap_anim,
            ads_frac: pred.ps.weapon_pos_frac,
            ammoclip: pred.ps.ammoclip,
            velocity: pred.ps.velocity,
            on_ground: pred.ps.on_ground,
            pm_type: pred.pm_type,
            mounted: pred.ps.mounted.is_some(),
        }
    }

    pub fn from_snapshot(p: &Protocol, ps: &msg::PlayerState) -> ViewPs {
        let f = |name: &str| ps.field_f32(p, name);
        ViewPs {
            weapon: ps.field_i32(p, "weapon") as u8,
            viewmodel_index: ps.field_i32(p, "viewmodelIndex"),
            weap_anim: ps.field_i32(p, "weapAnim"),
            ads_frac: f("fWeaponPosFrac"),
            ammoclip: ps.arrays.ammoclip,
            velocity: Vec3::new(f("velocity[0]"), f("velocity[1]"), f("velocity[2]")),
            on_ground: ps.field_i32(p, "groundEntityNum") as u32 != ENTITYNUM_NONE,
            pm_type: ps.field_i32(p, "pm_type"),
            mounted: ps.field_i32(p, "eFlags") & 0xc000 != 0,
        }
    }
}

/// What a rig is built from: the weapon file and the hands model override.
#[derive(Clone, Debug, PartialEq)]
struct RigKey {
    weapon: String,
    hands: Option<String>,
}

/// The weapon file and hands model `ps` names, borrowed from the
/// configstrings. `None` for weapon 0 or an index configstring 7 (1-based,
/// empty tokens skipped as `entities::split_weapon_list` does) does not name:
/// no viewmodel. Hands index 0 keeps the weapon file's own `handModel`.
fn rig_names(
    configstrings: &[String],
    weapon: u8,
    viewmodel_index: i32,
) -> Option<(&str, Option<&str>)> {
    let cs = |i: usize| configstrings.get(i).map(String::as_str).unwrap_or("");
    let name = cs(7)
        .split(' ')
        .filter(|s| !s.is_empty())
        .nth(usize::from(weapon).checked_sub(1)?)?;
    let hands = usize::try_from(viewmodel_index)
        .ok()
        .filter(|&i| i > 0)
        .map(|i| cs(CS_MODELS_V1 + i))
        .filter(|h| !h.is_empty());
    Some((name, hands))
}

/// The items configstring 8 marks registered, retail's
/// `CG_RegisterItems` walk (cgame 0x30036150): item `i` is bit `i & 3` of
/// the hex digit at `i >> 2`, items 1..0x46.
fn registered_items(cs8: &str) -> impl Iterator<Item = usize> + '_ {
    let digits = cs8.as_bytes();
    (1..0x46).filter(move |&i| {
        let d = digits.get(i >> 2).and_then(|&c| (c as char).to_digit(16));
        d.is_some_and(|d| d & 1 << (i & 3) != 0)
    })
}

/// Every (weapon file, hands) pair a rig can be asked for on this map: each
/// weapon item configstring 8 registers (items 1..=64 are configstring 7's
/// weapons), with each hands model the configstrings precache. Retail
/// registers those weapons' models at the gamestate (`CG_RegisterItemVisuals`
/// 0x30036080 calling `CG_RegisterWeapon` 0x30034cf0 for a weapon row).
fn prewarm_names(configstrings: &[String]) -> Vec<(&str, Option<&str>)> {
    let cs = |i: usize| configstrings.get(i).map(String::as_str).unwrap_or("");
    let weapons: Vec<&str> = cs(7).split(' ').filter(|s| !s.is_empty()).collect();
    let mut hands: Vec<Option<&str>> = configstrings
        .iter()
        .skip(CS_MODELS_V1)
        .take(256)
        .map(String::as_str)
        .filter(|m| m.contains("viewmodel_hands"))
        .map(Some)
        .collect();
    if hands.is_empty() {
        hands.push(None);
    }
    registered_items(cs(8))
        .filter(|&i| i <= 64)
        .filter_map(|i| weapons.get(i - 1).copied())
        .flat_map(|w| hands.iter().map(move |&h| (w, h)))
        .collect()
}

impl RigKey {
    fn names(&self) -> (&str, Option<&str>) {
        (&self.weapon, self.hands.as_deref())
    }
}

/// The frac trend `view_anim` reads, held through the frames between two cmds
/// (or two snapshots) that leave a mid-ramp frac unchanged, which would
/// otherwise read as idle and pop the sight down for a frame.
fn held_trend(last: &mut i32, trend: i32, frac: f32) -> i32 {
    if trend != 0 {
        *last = trend;
    } else if frac <= 0.0 || frac >= 1.0 {
        *last = 0;
    }
    *last
}

/// Seconds into `anim`'s clip, and whether it loops. The sight clips follow
/// the frac, so a raised sight holds `AdsUp`'s last frame.
fn clip_time(anim: WeaponAnim, ms: f64, frac: f32, clip_secs: f32) -> (f32, bool) {
    match anim {
        WeaponAnim::AdsUp => (frac.clamp(0.0, 1.0) * clip_secs, false),
        WeaponAnim::AdsDown => ((1.0 - frac.clamp(0.0, 1.0)) * clip_secs, false),
        WeaponAnim::Idle | WeaponAnim::EmptyIdle => ((ms / 1000.0) as f32, true),
        _ => ((ms / 1000.0) as f32, false),
    }
}

#[derive(Default)]
pub struct OnlineView {
    /// What the current rig was built for; `Some` with no rig when that
    /// load failed, so it is not retried every frame.
    built_for: Option<Option<RigKey>>,
    rig: Option<Box<ViewWeapon>>,
    rigs: viewmodel::RigCache,
    clock: ViewAnimClock,
    trend: i32,
    sight: SightDirection,
    motion: ViewmodelMotion,
    mouse: (f32, f32),
    /// `tag_flash` as last drawn, view space (X right, Y up, -Z forward):
    /// position and the tag's forward.
    flash: Option<(Vec3, Vec3)>,
    /// A scope overlay is up, which hides the gun and its flash.
    scoped: bool,
}

impl OnlineView {
    /// Raw counts, drained into the sway once per frame.
    pub fn mouse(&mut self, dx: f32, dy: f32) {
        self.mouse.0 += dx;
        self.mouse.1 += dy;
    }

    /// A download reopened the search path, and a pak can replace a model
    /// under the same name: rebuild the rig on the next `sync_rig`.
    pub fn reopen(&mut self) {
        self.built_for = None;
        self.rigs.clear();
    }

    /// Swaps the rig when the weapon or hands `ps` names changed, out of the
    /// cache after the first time. Returns the new models (hands, gun) for
    /// the renderer to draw.
    pub fn sync_rig(
        &mut self,
        fs: &Pk3Fs,
        configstrings: &[String],
        ps: &ViewPs,
    ) -> Option<viewmodel::ViewModels> {
        let names = rig_names(configstrings, ps.weapon, ps.viewmodel_index);
        if let Some(built) = &self.built_for
            && built.as_ref().map(RigKey::names) == names
        {
            return None;
        }
        self.built_for = Some(names.map(|(weapon, hands)| RigKey {
            weapon: weapon.to_string(),
            hands: hands.map(str::to_string),
        }));
        self.rig = None;
        self.clock = ViewAnimClock::default();
        let (weapon, hands) = names?;
        let (models, rig) = self.rigs.load(fs, weapon, hands)?;
        if rig.is_none() {
            log::warn!("viewmodel {weapon}: no anim rig, not drawing it");
        }
        self.rig = rig;
        self.rig.is_some().then_some(models)
    }

    /// Loads every rig this map can ask for into the cache, so the first
    /// switch to a weapon does not stall on reading it. Returns the models
    /// for the renderer to upload.
    pub fn prewarm(&mut self, fs: &Pk3Fs, configstrings: &[String]) -> Vec<viewmodel::ViewModels> {
        prewarm_names(configstrings)
            .into_iter()
            .filter_map(|(weapon, hands)| Some(self.rigs.load(fs, weapon, hands)?.0))
            .collect()
    }

    /// Whether last frame's sight put a scope overlay up.
    pub fn scoped(&self) -> bool {
        self.scoped
    }

    /// The viewmodel to draw and the horizontal fov both it and the world
    /// are drawn with, for `ps` when the view is the client's own and alive;
    /// `None` draws no viewmodel, as under a scope overlay. `weapons` is the
    /// configstring 7 table, for the clip index and the zoom.
    pub fn frame(
        &mut self,
        cg_fov: f32,
        weapons: &[Option<WeaponDef>],
        ps: Option<&ViewPs>,
        dt: f32,
        now_ms: f64,
    ) -> (Option<VmDraw>, f32) {
        let mouse = std::mem::take(&mut self.mouse);
        self.flash = None;
        let held = ps.and_then(|ps| weapons.get(usize::from(ps.weapon))?.as_ref());
        let mut zooming_in = false;
        let fov = ps.map_or(cg_fov, |ps| {
            zooming_in = self.sight.step(held, ps.ads_frac);
            weapon::view_fov_x(
                cg_fov,
                held,
                ps.ads_frac,
                zooming_in,
                ps.pm_type == PM_INTERMISSION,
                ps.mounted,
            )
        });
        self.scoped = ps.zip(held).is_some_and(|(ps, def)| {
            !ps.mounted && scope::overlay_frac(def, ps.ads_frac, zooming_in).is_some()
        });
        // No view weapon on a mounted gun (`0x300371f0`'s `eFlags & 0xc000`
        // test); the gun itself is the turret entity.
        if ps.is_some_and(|ps| ps.mounted) {
            return (None, fov);
        }
        let (Some(ps), Some(w)) = (ps, self.rig.as_deref_mut()) else {
            // A respawn whose `weapAnim` matches the pre-death one bit for bit
            // still restarts the raise.
            self.clock = ViewAnimClock::default();
            self.trend = 0;
            return (None, fov);
        };
        let clip_empty = held.is_some_and(|d| ps.ammoclip.get(d.clip_index) == Some(&0));
        let frac = ps.ads_frac;
        let (_, ms_in, trend) = self.clock.update(ps.weap_anim, frac, now_ms);
        let trend = held_trend(&mut self.trend, trend, frac);

        w.pose_sight(frac, frac >= 1.0 || trend > 0);
        let def = &w.def;
        let wanted = weapon::view_anim(def, ps.weap_anim, frac, trend, clip_empty);
        let idle = weapon::view_anim(def, 0, frac, trend, clip_empty);
        let clip_ms = w
            .anims
            .get(&wanted)
            .map_or(0.0, |(a, _)| f64::from(a.duration()) * 1000.0);
        let (anim, ms) = weapon::resolve(def, wanted, ms_in, clip_ms, idle);
        // A clip named but not loaded plays idle, as `--walk` does.
        let (anim, (clip, binding)) = match w.anims.get(&anim) {
            Some(c) => (anim, c),
            None => (WeaponAnim::Idle, &w.anims[&WeaponAnim::Idle]),
        };
        let (t, looping) = clip_time(anim, ms, frac, clip.duration());
        w.pose.apply(clip, binding, clip.frame_pos(t, looping));
        // two models, hands then gun, in set_viewmodel order
        let bone_sets = (0..2)
            .map(|m| w.pose.skin_matrices(&w.skeleton, m))
            .collect();

        let mut damp = 1.0 + (def.ads_view_bob_mult - 1.0) * frac;
        damp *= 1.0 + (def.ads_bob_factor - 1.0) * frac;
        let ground_speed = if ps.on_ground {
            ps.velocity.truncate().length()
        } else {
            0.0
        };
        self.motion
            .update(dt, ground_speed, ps.on_ground, mouse.0, mouse.1, damp);
        let transform = self.motion.transform();
        self.flash = w.skeleton.bone_index("tag_flash").map(|bi| {
            let (pos, rot) = w.pose.bone_world(&w.skeleton, bi);
            (
                transform.transform_point3(pos),
                transform.transform_vector3(rot * Vec3::X),
            )
        });
        // Posed all the same, so the clips run on under the scope.
        if self.scoped {
            self.flash = None;
            return (None, fov);
        }
        (
            Some(VmDraw {
                transform,
                fov_x: fov,
                bone_sets,
            }),
            fov,
        )
    }

    /// The drawn viewmodel's `tag_flash` in world space, for the camera at
    /// `eye` with basis `(forward, right, up)`. The viewmodel shares the
    /// world's fov, so view space maps straight onto the camera basis.
    pub fn muzzle(
        &self,
        eye: Vec3,
        (forward, right, up): (Vec3, Vec3, Vec3),
    ) -> Option<(Vec3, Vec3)> {
        let (pos, dir) = self.flash?;
        let world = |v: Vec3| right * v.x + up * v.y - forward * v.z;
        Some((eye + world(pos), world(dir).normalize_or_zero()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::weapon::CG_FOV;

    fn configstrings() -> Vec<String> {
        let mut cs = vec![String::new(); CS_MODELS_V1 + 90];
        cs[7] = "m1carbine_mp colt_mp mosin_nagant_mp".to_string();
        cs[CS_MODELS_V1 + 52] = "xmodel/viewmodel_hands_russian".to_string();
        cs[CS_MODELS_V1 + 82] = "xmodel/viewmodel_hands_us".to_string();
        cs
    }

    fn ps(weapon: u8, viewmodel_index: i32) -> ViewPs {
        ViewPs {
            weapon,
            viewmodel_index,
            weap_anim: 0,
            ads_frac: 0.0,
            ammoclip: [0; NUM_AMMO],
            velocity: Vec3::ZERO,
            on_ground: true,
            pm_type: 0,
            mounted: false,
        }
    }

    #[test]
    fn rig_names_follow_weapon_and_hands() {
        let cs = configstrings();
        let carbine_us = Some(("m1carbine_mp", Some("xmodel/viewmodel_hands_us")));
        assert_eq!(rig_names(&cs, 1, 82), carbine_us);
        assert_ne!(rig_names(&cs, 2, 82), carbine_us, "weapon");
        assert_ne!(rig_names(&cs, 1, 52), carbine_us, "hands");
        assert_eq!(rig_names(&cs, 1, 0), Some(("m1carbine_mp", None)));
    }

    #[test]
    fn prewarm_names_every_registered_weapon_with_every_hands_model() {
        let mut cs = configstrings();
        // Items 0, 1 and 3 (the colt is not registered), and 68, health.
        cs[8] = "b00000000000000001".to_string();
        assert_eq!(registered_items(&cs[8]).collect::<Vec<_>>(), [1, 3, 68]);
        assert_eq!(
            prewarm_names(&cs),
            [
                ("m1carbine_mp", Some("xmodel/viewmodel_hands_russian")),
                ("m1carbine_mp", Some("xmodel/viewmodel_hands_us")),
                ("mosin_nagant_mp", Some("xmodel/viewmodel_hands_russian")),
                ("mosin_nagant_mp", Some("xmodel/viewmodel_hands_us")),
            ]
        );
    }

    #[test]
    fn weapon_zero_draws_nothing() {
        let cs = configstrings();
        assert_eq!(rig_names(&cs, 0, 82), None);
        assert_eq!(rig_names(&cs, 9, 82), None, "past the end of CS 7");

        let mut view = OnlineView::default();
        let fs = Pk3Fs::empty();
        assert!(view.sync_rig(&fs, &cs, &ps(0, 82)).is_none());
        assert_eq!(view.built_for, Some(None));
        let (draw, fov) = view.frame(CG_FOV, &[], Some(&ps(0, 82)), 0.016, 0.0);
        assert!(draw.is_none());
        assert_eq!(fov, CG_FOV);
    }

    #[test]
    fn not_drawing_restarts_the_clip_clock() {
        let mut view = OnlineView::default();
        view.clock.update(10 | 512, 0.5, 0.0);
        view.trend = 1;
        view.frame(CG_FOV, &[], None, 0.016, 100.0);
        assert!(view.clock.update(10 | 512, 0.5, 200.0).0, "raise restarts");
        assert_eq!(view.trend, 0);
    }

    #[test]
    fn a_failed_rig_is_not_retried_until_the_key_changes() {
        let cs = configstrings();
        let fs = Pk3Fs::empty();
        let mut view = OnlineView::default();
        assert!(view.sync_rig(&fs, &cs, &ps(1, 82)).is_none());
        let built = view.built_for.clone();
        assert!(built.as_ref().is_some_and(Option::is_some));
        view.sync_rig(&fs, &cs, &ps(1, 82));
        assert_eq!(view.built_for, built);
        view.sync_rig(&fs, &cs, &ps(2, 82));
        assert_ne!(view.built_for, built);
    }

    #[test]
    fn trend_holds_between_cmds_mid_ramp() {
        let mut last = 0;
        assert_eq!(held_trend(&mut last, 1, 0.2), 1);
        assert_eq!(held_trend(&mut last, 0, 0.2), 1, "unchanged mid-ramp");
        assert_eq!(held_trend(&mut last, 0, 1.0), 0, "settled up");
        assert_eq!(held_trend(&mut last, -1, 0.7), -1);
        assert_eq!(held_trend(&mut last, 0, 0.7), -1);
        assert_eq!(held_trend(&mut last, 0, 0.0), 0, "settled down");
    }

    /// A raised sight holds `AdsUp`'s last frame: neither looping nor
    /// restarting, however long it is held.
    #[test]
    fn raised_sight_holds_the_last_ads_up_frame() {
        let secs = 0.3;
        for ms in [0.0, 150.0, 10_000.0] {
            assert_eq!(clip_time(WeaponAnim::AdsUp, ms, 1.0, secs), (secs, false));
        }
        assert_eq!(clip_time(WeaponAnim::AdsUp, 0.0, 0.5, secs), (0.15, false));
        assert_eq!(clip_time(WeaponAnim::AdsDown, 0.0, 1.0, secs), (0.0, false));
        assert_eq!(
            clip_time(WeaponAnim::AdsDown, 0.0, 0.0, secs),
            (secs, false)
        );
        assert_eq!(clip_time(WeaponAnim::Idle, 2500.0, 0.0, secs), (2.5, true));
    }

    /// `resolve` keeps `HoldFire` past its clip; played once, it clamps to
    /// the last frame rather than looping.
    #[test]
    fn held_grenade_holds_its_last_frame() {
        assert_eq!(
            clip_time(WeaponAnim::HoldFire, 10_000.0, 0.0, 0.6),
            (10.0, false)
        );
    }

    /// The flash lands on the pixel the viewmodel's projection draws its
    /// `tag_flash` at: the same fov as the world, only a nearer near plane.
    #[test]
    fn muzzle_lands_where_the_viewmodel_draws_it() {
        let aspect = 16.0 / 9.0;
        let at = Vec3::new(3.0, -2.5, -22.0);
        let mut view = OnlineView {
            flash: Some((at, Vec3::NEG_Z)),
            ..OnlineView::default()
        };
        let eye = Vec3::new(100.0, -40.0, 60.0);
        let (yaw, pitch) = (0.7_f32, -0.3_f32);
        let basis = crate::camera::basis(yaw, pitch);
        for fov in [CG_FOV, 50.0] {
            let want = crate::camera::perspective(fov, aspect, crate::renderer::VM_NEAR, 500.0)
                .project_point3(at);
            let (pos, dir) = view.muzzle(eye, basis).expect("drawn");
            let got = crate::camera::view_proj_from(eye, yaw, pitch, 0.0, fov, aspect)
                .project_point3(pos);
            assert!(
                (got.truncate() - want.truncate()).length() < 1e-4,
                "{fov}: {got} {want}"
            );
            assert!(dir.dot(basis.0) > 0.999, "along the view: {dir}");
        }
        view.flash = None;
        assert!(view.muzzle(eye, basis).is_none());
    }

    /// The gamestate's prewarm leaves the first switch a cache hit: the rig
    /// `sync_rig` builds shares the prewarmed models.
    #[test]
    fn prewarmed_rigs_serve_the_first_switch() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut cs = configstrings();
        cs[8] = "e".to_string(); // items 1..=3
        let mut view = OnlineView::default();
        let t0 = std::time::Instant::now();
        let rigs = view.prewarm(&fs, &cs);
        eprintln!("prewarm: {} rigs in {:?}", rigs.len(), t0.elapsed());
        assert_eq!(rigs.len(), 6, "three weapons with two hands each");
        let colt_us = view.sync_rig(&fs, &cs, &ps(2, 82)).expect("colt rig");
        assert!(rigs.iter().any(|m| std::sync::Arc::ptr_eq(m, &colt_us)));
    }

    /// The real carbine with the US hands, driven through a hip shot and a
    /// raised sight: every frame poses, and the sight holds still once up.
    #[test]
    fn real_carbine_poses_through_a_shot_and_the_sight() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut cs = configstrings();
        let mut view = OnlineView::default();
        let models = view
            .sync_rig(&fs, &cs, &ps(1, 82))
            .expect("carbine rig with the US hands");
        assert_eq!(models.len(), 2);
        assert!(view.sync_rig(&fs, &cs, &ps(1, 82)).is_none(), "no rebuild");
        let weapons = vcod_common::weapon_table::from_configstring(&fs, &cs[7]);

        // Gun bones for one frame at `now`.
        let pose = |view: &mut OnlineView, state: &ViewPs, now: f64| {
            let (draw, fov) = view.frame(CG_FOV, &weapons, Some(state), 0.016, now);
            let draw = draw.expect("drawn");
            assert_eq!(draw.bone_sets.len(), 2);
            assert!(draw.bone_sets.iter().flatten().all(|m| m.is_finite()));
            (draw.bone_sets[1].clone(), fov)
        };

        // The same clock driven once at hip idle and once through one shot,
        // a single edge onto 514 held from there.
        let mut idle = OnlineView::default();
        idle.sync_rig(&fs, &cs, &ps(1, 82));
        let hip = ps(1, 82);
        let mut shot = ps(1, 82);
        shot.weap_anim = 2 | 512;
        pose(&mut view, &hip, 0.0);
        pose(&mut idle, &hip, 0.0);
        let mut fired = Vec::new();
        for i in 1..=10 {
            let now = f64::from(i) * 16.0;
            let (fire, _) = pose(&mut view, &shot, now);
            assert_ne!(
                fire,
                pose(&mut idle, &hip, now).0,
                "fire, not idle, at {now} ms"
            );
            fired.push(fire);
        }
        assert_ne!(fired[0], fired[9], "the fire clip plays");

        assert!(view.flash.is_some(), "the gun has a tag_flash");

        let mut sighted = ps(1, 82);
        sighted.ads_frac = 1.0;
        let (up, fov) = pose(&mut view, &sighted, 1000.0);
        assert_eq!(fov, 65.0, "the sight zooms to the carbine's adsZoomFov");
        for i in 1..=30 {
            let now = 1000.0 + f64::from(i) * 16.0;
            assert_eq!(pose(&mut view, &sighted, now).0, up, "a raised sight holds");
        }

        // The scoped kar98k hides the gun once its overlay is up, and the
        // clips keep running under it.
        cs[7] = "kar98k_sniper_mp".to_string();
        let weapons = vcod_common::weapon_table::from_configstring(&fs, &cs[7]);
        let mut scoped = OnlineView::default();
        assert!(scoped.sync_rig(&fs, &cs, &ps(1, 82)).is_some());
        let mut up = ps(1, 82);
        for (i, frac) in [0.0, 0.3, 0.9, 1.0].into_iter().enumerate() {
            up.ads_frac = frac;
            let (draw, _) = scoped.frame(CG_FOV, &weapons, Some(&up), 0.016, i as f64 * 16.0);
            assert_eq!(draw.is_none(), frac >= 0.9, "at {frac}");
            assert_eq!(scoped.scoped(), frac >= 0.9);
        }
        cs[7] = "m1carbine_mp colt_mp mosin_nagant_mp".to_string();

        // A team change swaps the hands and rebuilds.
        cs[CS_MODELS_V1 + 82] = "xmodel/viewmodel_hands_russian".to_string();
        assert!(view.sync_rig(&fs, &cs, &ps(1, 82)).is_some());
    }
}
