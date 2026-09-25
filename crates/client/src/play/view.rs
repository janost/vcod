//! The playing client's own first-person weapon: the rig for `ps.weapon`
//! with the hands `ps.viewmodelIndex` names, posed from `ps.weapAnim`.

use crate::renderer::VmDraw;
use crate::viewmodel::{self, ViewWeapon, ViewmodelMotion};
use glam::Vec3;
use vcod_common::net::msg;
use vcod_common::net::protocol::{Protocol, CS_MODELS_V1, ENTITYNUM_NONE};
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::predict::Predicted;
use vcod_common::pmove::weapon::NUM_AMMO;
use vcod_common::weapon::{self, ViewAnimClock, WeaponAnim, WeaponDef, DEFAULT_FOV};
use vcod_common::xmodel::XModel;

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
}

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
    clock: ViewAnimClock,
    trend: i32,
    motion: ViewmodelMotion,
    mouse: (f32, f32),
}

impl OnlineView {
    /// Raw counts, drained into the sway once per frame.
    pub fn mouse(&mut self, dx: f32, dy: f32) {
        self.mouse.0 += dx;
        self.mouse.1 += dy;
    }

    /// Rebuilds the rig when the weapon or hands `ps` names changed. Returns
    /// the new models (hands, gun) for the renderer to upload.
    pub fn sync_rig(
        &mut self,
        fs: &Pk3Fs,
        configstrings: &[String],
        ps: &ViewPs,
    ) -> Option<Vec<XModel>> {
        let names = rig_names(configstrings, ps.weapon, ps.viewmodel_index);
        if let Some(built) = &self.built_for {
            if built.as_ref().map(RigKey::names) == names {
                return None;
            }
        }
        self.built_for = Some(names.map(|(weapon, hands)| RigKey {
            weapon: weapon.to_string(),
            hands: hands.map(str::to_string),
        }));
        self.rig = None;
        self.clock = ViewAnimClock::default();
        let (weapon, hands) = names?;
        let (models, rig) = viewmodel::load_view_weapon_with_hands(fs, weapon, hands)?;
        if rig.is_none() {
            log::warn!("viewmodel {weapon}: no anim rig, not drawing it");
        }
        self.rig = rig;
        self.rig.is_some().then_some(models)
    }

    /// The viewmodel to draw and the world fov, for `ps` when the view is
    /// the client's own and alive; `None` draws no viewmodel. `weapons` is
    /// the configstring 7 table, for the clip index.
    pub fn frame(
        &mut self,
        weapons: &[Option<WeaponDef>],
        ps: Option<&ViewPs>,
        dt: f32,
        now_ms: f64,
    ) -> (Option<VmDraw>, f32) {
        let mouse = std::mem::take(&mut self.mouse);
        let (Some(ps), Some(w)) = (ps, self.rig.as_deref_mut()) else {
            return (None, DEFAULT_FOV);
        };
        let clip_empty = weapons
            .get(usize::from(ps.weapon))
            .and_then(Option::as_ref)
            .is_some_and(|d| ps.ammoclip.get(d.clip_index) == Some(&0));
        let frac = ps.ads_frac;
        let (_, ms_in, trend) = self.clock.update(ps.weap_anim, frac, now_ms);
        let trend = held_trend(&mut self.trend, trend, frac);

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

        let fov = DEFAULT_FOV + (def.ads_zoom_fov - DEFAULT_FOV) * frac;
        let mut damp = 1.0 + (def.ads_view_bob_mult - 1.0) * frac;
        damp *= 1.0 + (def.ads_bob_factor - 1.0) * frac;
        let ground_speed = if ps.on_ground {
            ps.velocity.truncate().length()
        } else {
            0.0
        };
        self.motion
            .update(dt, ground_speed, ps.on_ground, mouse.0, mouse.1, damp);
        (
            Some(VmDraw {
                transform: self.motion.transform(),
                bone_sets,
            }),
            fov,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn weapon_zero_draws_nothing() {
        let cs = configstrings();
        assert_eq!(rig_names(&cs, 0, 82), None);
        assert_eq!(rig_names(&cs, 9, 82), None, "past the end of CS 7");

        let mut view = OnlineView::default();
        let fs = Pk3Fs::empty();
        assert!(view.sync_rig(&fs, &cs, &ps(0, 82)).is_none());
        assert_eq!(view.built_for, Some(None));
        let (draw, fov) = view.frame(&[], Some(&ps(0, 82)), 0.016, 0.0);
        assert!(draw.is_none());
        assert_eq!(fov, DEFAULT_FOV);
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
            let (draw, fov) = view.frame(&weapons, Some(state), 0.016, now);
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

        let mut sighted = ps(1, 82);
        sighted.ads_frac = 1.0;
        let (up, fov) = pose(&mut view, &sighted, 1000.0);
        assert!(fov < DEFAULT_FOV, "the sight zooms");
        for i in 1..=30 {
            let now = 1000.0 + f64::from(i) * 16.0;
            assert_eq!(pose(&mut view, &sighted, now).0, up, "a raised sight holds");
        }

        // A team change swaps the hands and rebuilds.
        cs[CS_MODELS_V1 + 82] = "xmodel/viewmodel_hands_russian".to_string();
        assert!(view.sync_rig(&fs, &cs, &ps(1, 82)).is_some());
    }
}
