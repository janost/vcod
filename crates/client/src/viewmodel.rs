//! The first-person weapon rig: loading the hands, gun and clips, and the
//! walk bob and mouse sway. No rendering or windowing types.

use std::collections::HashMap;
use std::sync::Arc;
use vcod_common::asset_source::AssetSource;
use vcod_common::{model, skeleton, weapon, xanim, xmodel};

/// A weapon's clips, each bound to the rig's skeleton.
pub type ViewAnims = HashMap<weapon::WeaponAnim, (Arc<xanim::XAnim>, skeleton::AnimBinding)>;

/// The skeleton and clips are shared with [`RigCache`]'s copy; the pose and
/// the state machine are this rig's own.
pub struct ViewWeapon {
    pub skeleton: Arc<skeleton::Skeleton>,
    pub pose: skeleton::PoseBuffer,
    pub state: weapon::WeaponState,
    pub def: weapon::WeaponDef,
    /// Missing entries fall back to Idle's clip.
    pub anims: Arc<ViewAnims>,
}

/// Every rig loaded this session, keyed by weapon file and hands override,
/// so a weapon switch reuses the parsed models and clips instead of reading
/// and parsing them again. A failed load is remembered too.
#[derive(Default)]
pub struct RigCache {
    /// Keyed by (weapon file, hands override).
    rigs: HashMap<(String, Option<String>), Option<CachedRig>>,
    parts: Parts,
}

/// Parsed files by name, shared across rigs: a hands model sits under every
/// weapon, and a weapon's file, gun and clips under every hands model. A
/// failed read or parse is remembered as `None`.
#[derive(Default)]
struct Parts {
    weapons: HashMap<String, Option<Arc<HashMap<String, String>>>>,
    models: HashMap<String, Option<Arc<model::Model>>>,
    anims: HashMap<String, Option<Arc<xanim::XAnim>>>,
}

impl Parts {
    fn weapon(&mut self, fs: &dyn AssetSource, name: &str) -> Option<Arc<HashMap<String, String>>> {
        self.weapons
            .entry(name.to_string())
            .or_insert_with(|| {
                let text = fs.read(&format!("weapons/mp/{name}"))?;
                Some(Arc::new(xmodel::parse_weapon(&String::from_utf8_lossy(
                    &text,
                ))))
            })
            .clone()
    }

    fn model(&mut self, fs: &dyn AssetSource, name: &str) -> Option<Arc<model::Model>> {
        self.models
            .entry(name.to_string())
            .or_insert_with(|| match model::load(fs, name) {
                Ok(m) => Some(Arc::new(m)),
                Err(e) => {
                    log::warn!("viewmodel {name}: {e:#}");
                    None
                }
            })
            .clone()
    }

    fn anim(&mut self, fs: &dyn AssetSource, name: &str, key: &str) -> Option<Arc<xanim::XAnim>> {
        self.anims
            .entry(name.to_string())
            .or_insert_with(|| match xanim::load(fs, name) {
                Ok(a) => Some(Arc::new(a)),
                Err(e) => {
                    log::warn!("xanim {name} ({key}): {e:#}");
                    None
                }
            })
            .clone()
    }
}

/// Hands then gun, shared between the cache and whoever draws them.
pub type ViewModels = Arc<[model::Model]>;

struct CachedRig {
    models: ViewModels,
    rig: Option<ViewWeapon>,
}

impl RigCache {
    /// [`load_with`] through the cache; a hit hands back the
    /// same models and a fresh rig (pose seeded with idle, state at rest).
    pub fn load(
        &mut self,
        fs: &dyn AssetSource,
        name: &str,
        hands_model: Option<&str>,
    ) -> Option<(ViewModels, Option<Box<ViewWeapon>>)> {
        let key = (name.to_string(), hands_model.map(str::to_string));
        let entry = self.rigs.entry(key).or_insert_with(|| {
            load_with(&mut self.parts, fs, name, hands_model).map(|(models, rig)| CachedRig {
                models: models.into(),
                rig: rig.map(|r| *r),
            })
        });
        let cached = entry.as_ref()?;
        Some((
            cached.models.clone(),
            cached.rig.as_ref().map(|r| Box::new(r.fresh())),
        ))
    }

    /// A download reopened the search path; a pak may replace a model or a
    /// clip under the same name.
    pub fn clear(&mut self) {
        self.rigs.clear();
        self.parts = Parts::default();
    }
}

/// [`load_with`] off empty parts.
#[cfg(test)]
pub fn load_view_weapon(
    fs: &dyn AssetSource,
    name: &str,
    hands_model: Option<&str>,
) -> Option<(Vec<model::Model>, Option<Box<ViewWeapon>>)> {
    load_with(&mut Parts::default(), fs, name, hands_model)
}

/// Hands first so the gun draws over them and the shared skeleton takes the
/// hands' bones as its base. `None` if a model is missing (walk mode then has
/// no viewmodel); the inner `None` means the models loaded but the anims did not.
/// `hands_model`, when given, replaces the file's `handModel`: retail draws the hands `ps.viewmodelIndex` names
/// (docs/research/cod11-gsc-object-model.md, "`setViewmodel` reaches
/// `ps.viewmodelIndex`"). The `xmodel/` prefix is optional.
fn load_with(
    parts: &mut Parts,
    fs: &dyn AssetSource,
    name: &str,
    hands_model: Option<&str>,
) -> Option<(Vec<model::Model>, Option<Box<ViewWeapon>>)> {
    let weapon = parts.weapon(fs, name)?;
    let hands = match hands_model {
        Some(h) => h.strip_prefix("xmodel/").unwrap_or(h),
        None => weapon.get("handModel")?,
    };
    let mut hands = (*parts.model(fs, hands)?).clone();
    xmodel::apply_viewhands_placeholder_override(&mut hands);
    let gun = (*parts.model(fs, weapon.get("gunModel")?)?).clone();
    let models = vec![hands, gun];
    let animated = load_anims(parts, fs, &weapon, &models).map(Box::new);
    Some((models, animated))
}

/// A clip that is unnamed or fails to load is skipped and its state plays
/// idle. Without idle there is no fallback, so the rig is dropped and the
/// viewmodel draws in bind pose.
fn load_anims(
    parts: &mut Parts,
    fs: &dyn AssetSource,
    weapon: &HashMap<String, String>,
    models: &[model::Model],
) -> Option<ViewWeapon> {
    let [hands, gun] = models else {
        return None;
    };
    // Same order as set_viewmodel, so bone_sets[i] matches model i. The gun
    // hangs off `tag_weapon` whatever its root is called: the carbine's root
    // is `tag_origin` (docs/research/xmodel-v14-format.md, "Model space and
    // the view basis").
    let skeleton = skeleton::Skeleton::build_grafted(&[(hands, None), (gun, Some("tag_weapon"))]);

    let mut anims = ViewAnims::new();
    for which in weapon::WeaponAnim::ALL {
        let key = which.key();
        let Some(name) = weapon.get(key).map(|n| n.trim()).filter(|n| !n.is_empty()) else {
            log::debug!("weapon: no {key}, that state will play idle");
            continue;
        };
        if let Some(anim) = parts.anim(fs, name, key) {
            let binding = skeleton.bind(&anim);
            anims.insert(which, (anim, binding));
        }
    }
    if !anims.contains_key(&weapon::WeaponAnim::Idle) {
        log::warn!("no idle anim loaded; drawing the viewmodel statically");
        return None;
    }

    Some(ViewWeapon::new(
        Arc::new(skeleton),
        Arc::new(anims),
        weapon::WeaponDef::from_map(weapon),
    ))
}

impl ViewWeapon {
    /// `anims` must hold Idle.
    fn new(
        skeleton: Arc<skeleton::Skeleton>,
        anims: Arc<ViewAnims>,
        def: weapon::WeaponDef,
    ) -> ViewWeapon {
        // Seeded with idle: a first clip that keys only `tag_torso` (a sight
        // already rising at the spawn) would leave the arms at the zeroed bind.
        let mut pose = skeleton::PoseBuffer::new(&skeleton);
        let (idle, binding) = &anims[&weapon::WeaponAnim::Idle];
        pose.apply(idle, binding, 0.0);
        ViewWeapon {
            pose,
            skeleton,
            state: weapon::WeaponState::new(def.clone()),
            def,
            anims,
        }
    }

    /// The same weapon at rest: shared skeleton and clips, new pose and state.
    pub fn fresh(&self) -> ViewWeapon {
        ViewWeapon::new(self.skeleton.clone(), self.anims.clone(), self.def.clone())
    }

    /// Poses the sight layer and the main clip as sibling nodes of the view
    /// tree, each at weight 1 ([`skeleton::PoseBuffer::blend_nodes`]): a bone
    /// one of them keys takes its pose, a bone both key their blend. The
    /// sight layer is `AdsUp` at `frac` while `raising`, else `AdsDown` at
    /// `1 - frac`, so the hip is `AdsDown`'s last frame, not the bind pose
    /// (docs/research/xanim-v14-format.md, "The sight layer"). A main clip
    /// of `AdsUp` or `AdsDown` (`--walk`'s sight states) plays idle, as the
    /// wire's `weapAnim` 0 does. A clip not loaded plays idle.
    pub fn pose(&mut self, frac: f32, raising: bool, main: weapon::WeaponAnim, main_pos: f32) {
        use weapon::WeaponAnim;
        let (which, t) = if raising {
            (WeaponAnim::AdsUp, frac)
        } else {
            (WeaponAnim::AdsDown, 1.0 - frac)
        };
        let anims = self.anims.clone();
        let sight = anims.get(&which).map(|(clip, binding)| {
            let t = t.clamp(0.0, 1.0) * clip.duration();
            (&**clip, binding, clip.frame_pos(t, false), 1.0)
        });
        let (main, main_pos) = match main {
            WeaponAnim::AdsUp | WeaponAnim::AdsDown => (WeaponAnim::Idle, 0.0),
            m => (m, main_pos),
        };
        let (clip, binding) = anims.get(&main).unwrap_or(&anims[&WeaponAnim::Idle]);
        let main = [(&**clip, binding, main_pos, 1.0)];
        match &sight {
            Some(s) => self
                .pose
                .blend_nodes(&[(std::slice::from_ref(s), 1.0), (&main, 1.0)]),
            None => self.pose.blend_nodes(&[(&main, 1.0)]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the real kar98k through the redraw loop's calls without a window.
    /// Reaches idle, fire, rechamber, ADS up and ADS fire; not LastShot,
    /// AdsDown or Reloading.
    #[test]
    fn real_kar98k_animates_through_a_fire_and_ads_cycle() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let (models, view_weapon) =
            load_view_weapon(&fs, "kar98k_mp", None).expect("kar98k viewmodel");
        assert_eq!(models.len(), 2);
        let mut w = view_weapon.expect("kar98k anim rig");
        assert!(
            w.anims.contains_key(&weapon::WeaponAnim::Idle),
            "the rig only exists when idle loaded"
        );

        // fire every 40th frame so the bolt cycle completes; ADS for the second half
        let mut poses = Vec::new();
        for step in 0..240 {
            let out = w.state.update(
                1.0 / 60.0,
                weapon::WeaponInput {
                    fire: step > 60 && step % 40 == 0,
                    fire_held: false,
                    ads: step > 120,
                    reload: false,
                },
            );
            let (anim, _) = w
                .anims
                .get(&out.anim)
                .or_else(|| w.anims.get(&weapon::WeaponAnim::Idle))
                .unwrap_or_else(|| panic!("{:?} has no clip and no idle fallback", out.anim));
            let frame = anim.frame_pos(out.anim_time, out.looping);
            assert!(
                frame.is_finite() && frame >= 0.0 && frame <= (anim.frame_count - 1) as f32,
                "{:?} frame {frame} out of range",
                out.anim
            );
            w.pose(out.ads_frac, step > 120, out.anim, frame);
            for (i, model) in models.iter().enumerate() {
                let mats = w.pose.skin_matrices(&w.skeleton, i);
                assert_eq!(mats.len(), model.bones.len(), "model {i} bone count");
                assert!(mats.iter().all(|m| m.is_finite()), "model {i} step {step}");
            }
            poses.push(w.pose.skin_matrices(&w.skeleton, 1));
        }
        // a static rig would be a silent failure
        assert!(
            poses.iter().any(|p| p != &poses[0]),
            "the gun never moved across 240 frames"
        );
    }

    /// Model-space `tag_flash` (X forward, Y left, Z up) after the sight layer
    /// at `frac` and the idle clip on top, as a frame at rest draws it.
    fn rest_flash(w: &mut ViewWeapon, frac: f32) -> glam::Vec3 {
        w.pose(frac, frac > 0.0, weapon::WeaponAnim::Idle, 0.0);
        let bi = w.skeleton.bone_index("tag_flash").expect("tag_flash");
        w.pose.bone_world(&w.skeleton, bi).0
    }

    /// At the hip the muzzle sits right of and below the eye; with the sight
    /// up it is on the view axis. The carbine's gun root is `tag_origin`, so
    /// it moves at all only through the `tag_weapon` graft.
    #[test]
    fn hip_holds_the_gun_low_right_and_the_sight_centres_it() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        for name in ["kar98k_mp", "m1carbine_mp"] {
            let (_, rig) = load_view_weapon(&fs, name, None).expect("viewmodel");
            let mut w = rig.expect("anim rig");
            let hip = rest_flash(&mut w, 0.0);
            assert!(hip.y < -2.0 && hip.z < -2.0, "{name} hip {hip}");
            let sight = rest_flash(&mut w, 1.0);
            assert!(
                sight.y.abs() < 0.5 && sight.z.abs() < 1.0,
                "{name} sight {sight}"
            );
        }
    }

    /// United Offensive's newer rigs key `tag_torso` in every clip at the hip
    /// and author the sight clips to centre the gun only in the blend with
    /// idle: `adsUpAnim` alone ends 3 units left of the eye and 2 up.
    #[test]
    fn uo_rigs_centre_the_sight_in_the_blend_with_idle() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        for name in ["gewehr43_mp", "svt40_mp"] {
            let (_, rig) = load_view_weapon(&fs, name, None).expect("viewmodel");
            let mut w = rig.expect("anim rig");
            let hip = rest_flash(&mut w, 0.0);
            assert!(hip.y < -1.0 && hip.z < -2.0, "{name} hip {hip}");
            let sight = rest_flash(&mut w, 1.0);
            assert!(
                sight.y.abs() < 1.0 && sight.z.abs() < 2.5,
                "{name} sight {sight}"
            );
        }
    }

    /// Every stock weapon file at rest (sight layer down, idle on top) holds
    /// the gun ahead of, right of and below the eye. A rig that leaves
    /// `tag_torso` at its zeroed bind puts `tag_weapon` behind the eye, the
    /// receiver in the camera's face (docs/research/xanim-v14-format.md,
    /// "The sight layer").
    #[test]
    fn every_stock_weapon_rests_ahead_right_and_below() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let mut checked = 0;
        for path in fs.list_prefix("weapons/mp/") {
            let name = path.rsplit('/').next().unwrap();
            let Some((_, Some(mut w))) = load_view_weapon(&fs, name, None) else {
                continue;
            };
            w.pose(0.0, false, weapon::WeaponAnim::Idle, 0.0);
            let bi = w.skeleton.bone_index("tag_weapon").expect("tag_weapon");
            let p = w.pose.bone_world(&w.skeleton, bi).0;
            assert!(
                p.x > 2.0 && p.y < -1.0 && p.z < -2.0,
                "{name}: tag_weapon at rest {p}"
            );
            checked += 1;
        }
        assert!(checked >= 20, "only {checked} stock weapons had a rig");
    }

    /// The MP44's raise is cut at `raiseTime` (0.35 s) before its pullout
    /// clip ends (0.4 s), with `tag_weapon` still turned a few degrees, and
    /// its idle keys no rotation on `tag_weapon`. Idle after the cut raise
    /// must hold the gun where a fresh idle does, or the gun stays turned
    /// out of the hands (issue 5).
    #[test]
    fn idle_after_a_cut_raise_puts_the_gun_back_in_the_hands() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let (_, rig) = load_view_weapon(&fs, "mp44_mp", None).expect("mp44 viewmodel");
        let mut w = rig.expect("anim rig");
        let gun = |w: &ViewWeapon| {
            let bi = w.skeleton.bone_index("tag_flash").expect("tag_flash");
            w.pose.bone_world(&w.skeleton, bi).0
        };
        let anims = w.anims.clone();
        let (idle, idle_b) = &anims[&weapon::WeaponAnim::Idle];
        w.pose.apply(idle, idle_b, 0.0);
        let rest = gun(&w);
        let (raise, raise_b) = &anims[&weapon::WeaponAnim::Raise];
        w.pose
            .apply(raise, raise_b, raise.frame_pos(w.def.raise_time, false));
        w.pose.apply(idle, idle_b, 0.0);
        let after = gun(&w);
        assert!(
            after.distance(rest) < 0.01,
            "muzzle {after} after the raise, {rest} at rest"
        );
    }

    /// A second load of the same weapon and hands reuses the parsed models
    /// and clips but starts its own pose and state; other hands are their
    /// own entry.
    #[test]
    fn rig_cache_shares_the_load_and_hands_out_fresh_rigs() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut cache = RigCache::default();
        let (m1, r1) = cache.load(&fs, "kar98k_mp", None).expect("kar98k");
        let mut r1 = r1.expect("rig");
        // past the raise, then one shot
        for step in 0..121 {
            r1.state.update(
                1.0 / 60.0,
                weapon::WeaponInput {
                    fire: step == 120,
                    fire_held: step == 120,
                    ads: false,
                    reload: false,
                },
            );
        }
        assert_eq!(r1.state.ammo() + 1, r1.def.clip_size, "the first rig fired");
        let (m2, r2) = cache.load(&fs, "kar98k_mp", None).expect("cached kar98k");
        let r2 = r2.expect("rig");
        assert!(Arc::ptr_eq(&m1, &m2), "the models come from the cache");
        assert!(Arc::ptr_eq(&r1.anims, &r2.anims), "the clips are shared");
        assert!(Arc::ptr_eq(&r1.skeleton, &r2.skeleton));
        assert_eq!(r2.state.ammo(), r2.def.clip_size, "a fresh state");

        let (us, _) = cache
            .load(&fs, "kar98k_mp", Some("xmodel/viewmodel_hands_us"))
            .expect("kar98k with US hands");
        assert!(!Arc::ptr_eq(&m1, &us), "other hands are another entry");
        assert!(cache.load(&fs, "no_such_weapon_mp", None).is_none());
        assert!(cache.load(&fs, "no_such_weapon_mp", None).is_none());
    }

    /// Two hands under one weapon parse its clips once, and one hands model
    /// under two weapons is parsed once.
    #[test]
    fn rigs_share_parsed_parts_by_name() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut cache = RigCache::default();
        let us = Some("xmodel/viewmodel_hands_us");
        let ru = Some("xmodel/viewmodel_hands_russian");
        let (_, a) = cache.load(&fs, "kar98k_mp", us).expect("kar98k, us");
        let (_, b) = cache.load(&fs, "kar98k_mp", ru).expect("kar98k, russian");
        let idle = |r: &Option<Box<ViewWeapon>>| {
            r.as_ref().expect("rig").anims[&weapon::WeaponAnim::Idle]
                .0
                .clone()
        };
        assert!(Arc::ptr_eq(&idle(&a), &idle(&b)), "one parse of the clip");
        cache.load(&fs, "m1carbine_mp", us).expect("carbine, us");
        assert_eq!(cache.parts.models.len(), 4, "two hands, two guns");
        assert_eq!(cache.parts.weapons.len(), 2);
    }

    /// A sight already rising on the rig's first frame keys only `tag_torso`;
    /// the arms still hold the idle pose instead of the zeroed bind.
    #[test]
    fn a_fresh_rig_holds_the_idle_arms() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let (_, rig) = load_view_weapon(&fs, "m1carbine_mp", None).expect("carbine viewmodel");
        let mut w = rig.expect("anim rig");
        w.pose(0.5, true, weapon::WeaponAnim::AdsUp, 0.0);
        let at = |w: &ViewWeapon, bone: &str| {
            let bi = w.skeleton.bone_index(bone).expect(bone);
            w.pose.bone_world(&w.skeleton, bi).0
        };
        let reach = at(&w, "bip01 l hand").distance(at(&w, "tag_torso"));
        assert!(
            reach > 5.0,
            "the left hand collapsed onto tag_torso: {reach}"
        );
    }

    /// The nationality hands replace the file's `handModel`; the gun and the
    /// clips still come from the weapon file.
    #[test]
    fn hands_override_replaces_the_file_hand_model() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let (own, _) = load_view_weapon(&fs, "m1carbine_mp", None).expect("carbine viewmodel");
        let (us, rig) = load_view_weapon(&fs, "m1carbine_mp", Some("xmodel/viewmodel_hands_us"))
            .expect("carbine with the US hands");
        assert_eq!(us.len(), 2);
        // The hand files share one mesh and differ in their sleeve skins.
        assert_ne!(
            us[0].materials(),
            own[0].materials(),
            "hands must come from the override"
        );
        assert!(us[0].materials().iter().any(|m| m == "viewsleeves_new.tga"));
        assert_eq!(us[1].name, own[1].name, "the gun stays the file's");
        assert!(rig.is_some_and(|w| w.anims.contains_key(&weapon::WeaponAnim::Idle)));
    }
}
