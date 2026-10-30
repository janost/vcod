//! The playing client's own first-person weapon: the rig for `ps.weapon`
//! with the hands `ps.viewmodelIndex` names, posed from `ps.weapAnim`.

use super::quake::{Quake, Quakes};
use super::recoil::{self, Recoil};
use crate::hud::scope;
use crate::renderer::VmDraw;
use crate::viewmodel::{self, ViewWeapon};
use glam::{Mat4, Vec3};
use vcod_common::net::msg;
use vcod_common::net::protocol::{CS_MODELS_V1, Protocol};
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::aim;
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
    /// `leanf`, -1 (left) to 1 (right).
    pub lean: f32,
    pub ammoclip: [i16; NUM_AMMO],
    pub pm_type: i32,
    /// On a mounted gun: `eFlags & 0xc000` on the wire.
    pub mounted: bool,
    /// `pm_flags`' view bits: 0x40000 the client's own view, 0x10000 a
    /// follow (`play::recoil`).
    pub pm_flags: i32,
    /// `origin` raised by `viewHeightCurrent`: the viewmodel's lighting
    /// origin.
    pub eye: Vec3,
    /// A UO vehicle seat; the caller sets it.
    pub seat: Option<super::seat::Seat>,
}

/// `pm_type` at intermission.
use vcod_common::net::flags::PM_INTERMISSION;

/// The eye's height over the drawn origin. `CG_CalcViewValues`' intermission
/// arm (cgame 0x300333f2) puts the view at `ps.origin` itself, where retail
/// sends the spawn's standing `viewHeightCurrent`.
pub fn eye_height(pm_type: i32, view_height: f32) -> f32 {
    if pm_type == PM_INTERMISSION {
        0.0
    } else {
        view_height
    }
}

impl ViewPs {
    /// `viewmodel_index` comes from the snapshot: prediction does not carry it.
    pub fn from_predicted(pred: &Predicted, viewmodel_index: i32) -> ViewPs {
        ViewPs {
            weapon: pred.ps.weapon,
            viewmodel_index,
            weap_anim: pred.ps.weap_anim,
            ads_frac: pred.ps.weapon_pos_frac,
            lean: pred.ps.lean,
            ammoclip: pred.ps.ammoclip,
            pm_type: pred.pm_type,
            mounted: pred.ps.mounted.is_some(),
            pm_flags: recoil::PMF_OWN_VIEW,
            eye: pred.ps.origin + Vec3::Z * pred.ps.view_height(),
            seat: None,
        }
    }

    pub fn from_snapshot(p: &Protocol, ps: &msg::PlayerState) -> ViewPs {
        let f = |name: &str| ps.field_f32(p, name);
        ViewPs {
            weapon: ps.field_i32(p, "weapon") as u8,
            viewmodel_index: ps.field_i32(p, "viewmodelIndex"),
            weap_anim: ps.field_i32(p, "weapAnim"),
            ads_frac: f("fWeaponPosFrac"),
            lean: f("leanf"),
            ammoclip: ps.arrays.ammoclip,
            pm_type: ps.field_i32(p, "pm_type"),
            mounted: ps.field_i32(p, "eFlags") & 0xc000 != 0,
            pm_flags: ps.field_i32(p, "pm_flags"),
            eye: Vec3::new(
                f("origin[0]"),
                f("origin[1]"),
                f("origin[2]") + f("viewHeightCurrent"),
            ),
            seat: None,
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
    models: usize,
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
        .map(|i| cs(models + i))
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
/// weapon item configstring 8 registers (the items configstring 7 names; the
/// ammo and health rows past them name none), with each hands model the configstrings precache. Retail
/// registers those weapons' models at the gamestate (`CG_RegisterItemVisuals`
/// 0x30036080 calling `CG_RegisterWeapon` 0x30034cf0 for a weapon row).
fn prewarm_names(configstrings: &[String], models: usize) -> Vec<(&str, Option<&str>)> {
    let cs = |i: usize| configstrings.get(i).map(String::as_str).unwrap_or("");
    let weapons: Vec<&str> = cs(7).split(' ').filter(|s| !s.is_empty()).collect();
    let mut hands: Vec<Option<&str>> = configstrings
        .iter()
        .skip(models)
        .take(256)
        .map(String::as_str)
        .filter(|m| m.contains("viewmodel_hands"))
        .map(Some)
        .collect();
    if hands.is_empty() {
        hands.push(None);
    }
    registered_items(cs(8))
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

/// The lean's own term in the viewmodel's position (cgame `0x30036990`,
/// combat doc 15.5): `(1 - fWeaponPosFrac) * f * 1.6` units along the right
/// vector of `(0, 0, f * -2)`, `f` being `GetLeanFraction(leanf)`. View
/// space, X right and Y up.
fn lean_offset(leanf: f32, frac: f32) -> Vec3 {
    if leanf == 0.0 || frac >= 1.0 {
        return Vec3::ZERO;
    }
    let f = aim::lean_fraction(leanf);
    let d = (1.0 - frac) * f * 1.6;
    // `AngleVectors`' right of a pure roll is (0, -cos, -sin) in
    // (forward, left, up).
    let (sr, cr) = (f * -2.0).to_radians().sin_cos();
    Vec3::new(cr * d, -sr * d, 0.0)
}

/// A (forward, left, up) vector in view space (X right, Y up, -Z forward);
/// docs/research/xmodel-v14-format.md, "Model space and the view basis".
pub(crate) fn view_from_ful(v: Vec3) -> Vec3 {
    Vec3::new(-v.y, v.z, -v.x)
}

/// `pose` alone, for `--walk`, which has no lean or landing.
pub fn pose_transform(pose: scope::GunPose) -> Mat4 {
    gun_transform(view_from_ful(Vec3::from(pose.offset)), pose.angles)
}

/// The view weapon's model matrix in view space: the model's axes are
/// `AnglesToAxis(angles)` in the view's frame (`0x30037480`-`0x300374aa`
/// multiply it onto the view's axis), at `pos`.
pub(crate) fn gun_transform(pos: Vec3, angles: [f32; 3]) -> Mat4 {
    let [f, l, u] = aim::angles_to_axis_msvc(angles).map(|r| view_from_ful(Vec3::from(r)));
    Mat4::from_translation(pos)
        * Mat4::from_cols(f.extend(0.0), l.extend(0.0), u.extend(0.0), glam::Vec4::W)
}

/// `cg_drawGun` as `0x300371f0` reads it (`[0x301dbaec]`, combat doc 17.4):
/// 0 never adds the view weapon to the scene, 2 always, anything else
/// unless a scope overlay is up. The pose runs either way.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub enum DrawGun {
    Never,
    #[default]
    Unscoped,
    Always,
}

impl DrawGun {
    pub fn from_cvar(v: i32) -> DrawGun {
        match v {
            0 => DrawGun::Never,
            2 => DrawGun::Always,
            _ => DrawGun::Unscoped,
        }
    }
}

#[derive(Default)]
pub struct OnlineView {
    /// `cg_drawGun`, set by the caller each frame.
    pub draw_gun: DrawGun,
    /// The playerstate's shellshock and the console's `cg_shellshock`.
    pub shock: super::shock::ShellShock,
    /// A `cg_shellshock` typed since the last frame, its arguments; the
    /// frame starts it on its own clock.
    pub shock_command: Option<Vec<String>>,
    /// What the current rig was built for; `Some` with no rig when that
    /// load failed, so it is not retried every frame.
    built_for: Option<Option<RigKey>>,
    rig: Option<Box<ViewWeapon>>,
    rigs: viewmodel::RigCache,
    clock: ViewAnimClock,
    trend: i32,
    sight: SightDirection,
    /// `tag_flash` as last drawn, view space (X right, Y up, -Z forward):
    /// position and the tag's forward.
    flash: Option<(Vec3, Vec3)>,
    /// `tag_brass` as last posed, the same space; kept under a scope, as
    /// `CG_EjectBrass` reads the viewmodel's tag whether it is drawn.
    brass: Option<(Vec3, Vec3)>,
    /// A scope overlay is up, which hides the gun and its flash.
    scoped: bool,
    recoil: Recoil,
    /// The newest snapshot's `(clientNum, stats[5])`.
    spawn: Option<(i32, i32)>,
    /// `now_ms` of the last frame, whole milliseconds.
    last_ms: Option<i64>,
    quakes: Quakes,
    land: Land,
    /// The connection's `CS_MODELS` (`Protocol::cs`), 1.1's until set.
    cs_models: Option<usize>,
}

/// The landing dip (cgame `0x302094e8`, the time, and `0x302094e4`, the
/// change; combat doc 17.5).
#[derive(Default)]
struct Land {
    time_ms: i32,
    change: f32,
    /// The eye's dip this frame, which the gun takes a quarter of again.
    dip: f32,
}

/// The change a landing of the drawn client sets (`CG_EntityEvent`
/// `0x3001dd93`, `0x3001ddfb`): `EV_LANDING_*` sinks by its parm,
/// `EV_LANDING_PAIN_*` by the fall height its damage percent stands for
/// between `bg_fallDamageMinHeight` and `MaxHeight`, as `(h - 12) / 26 * 4
/// + 4` capped at 24, nothing at or under 12 units.
pub fn land_change(event: i32, parm: i32, fall: vcod_common::pmove::FallHeights) -> Option<f32> {
    use vcod_common::net::event_ids::{EV_LANDING_BASE, EV_LANDING_PAIN_BASE, SURFACE_GROUP_SIZE};
    let group = |base: i32| (base..base + SURFACE_GROUP_SIZE).contains(&event);
    if group(EV_LANDING_BASE) {
        return Some(-(parm as f32));
    }
    if !group(EV_LANDING_PAIN_BASE) {
        return None;
    }
    let h = (fall.max - fall.min) * parm as f32 * 0.01 + fall.min;
    if h <= 12.0 {
        return None;
    }
    let n = (((h - 12.0) * (1.0 / 26.0) + 1.0) * 4.0) as i32;
    (n > 0).then(|| -(n.min(24) as f32))
}

impl OnlineView {
    /// The connection's protocol, whose model block the hands index.
    pub fn set_protocol(&mut self, p: &Protocol) {
        self.cs_models = Some(p.cs.models());
    }

    fn models(&self) -> usize {
        self.cs_models.unwrap_or(CS_MODELS_V1)
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
        let names = rig_names(configstrings, self.models(), ps.weapon, ps.viewmodel_index);
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
        prewarm_names(configstrings, self.models())
            .into_iter()
            .filter_map(|(weapon, hands)| Some(self.rigs.load(fs, weapon, hands)?.0))
            .collect()
    }

    /// A shot by the body the view rides: `CG_FireWeapon`'s recoil call
    /// (`0x30038bd2`), made under `pm_flags` 0x50000, own view or a follow.
    pub fn fire(&mut self, weapons: &[Option<WeaponDef>], ps: &ViewPs) {
        if let Some(def) = weapons.get(usize::from(ps.weapon)).and_then(Option::as_ref)
            && ps.pm_flags & recoil::PMF_VIEW_BODY != 0
        {
            self.recoil.fire(def, ps.ads_frac);
        }
    }

    /// The newest snapshot's `(clientNum, stats[5])`: a new client or spawn
    /// count, or the first snapshot after a gamestate or a restart, clears
    /// both kicks and the quakes, as the snapshot transition's call to
    /// `0x30028a70` (`0x300300cd`) does on a respawn or a new followed body
    /// (it zeroes the quake slots `0x3020cff4` with them).
    pub fn track_spawn(&mut self, spawn: (i32, i32)) {
        if self.spawn.replace(spawn) != Some(spawn) {
            self.recoil.reset();
            self.quakes.reset();
        }
    }

    /// `CG_MapRestart` sets `cg.mapRestart` (0x30207154): the next snapshot
    /// runs the respawn reset whatever its spawn count.
    pub fn map_restart(&mut self) {
        self.spawn = None;
    }

    /// An in-game `vid_restart` and every map load free the cgame and
    /// start it again (docs/research/cod11-front-end.md section 17, "The
    /// cgame restart"): everything it held starts fresh, the kicks the
    /// first snapshot's setup clears (`0x3003037f`) included. The cvars
    /// (`cg_drawGun`, the shellshock file `cg_shellshock` reuses), the
    /// connection's configstring layout and the loaded rigs, which are
    /// assets, stay.
    pub fn restart_cgame(&mut self) {
        *self = OnlineView {
            draw_gun: self.draw_gun,
            shock: self.shock.restart_cgame(),
            shock_command: self.shock_command.take(),
            rigs: std::mem::take(&mut self.rigs),
            cs_models: self.cs_models,
            ..OnlineView::default()
        };
    }

    /// An `EV_EARTHQUAKE` or `EV_FIRE_WEAPON_MG42` arrived (`play::quake`)
    /// at `now_ms`, the render clock, before this frame's view.
    pub fn start_quake(&mut self, q: Quake, now_ms: i32) {
        self.quakes.start(q, now_ms);
    }

    /// The quake shake on this frame's view, wire degrees.
    pub fn shake(&mut self, now_ms: i32, eye: Vec3) -> [f32; 3] {
        self.quakes.shake(now_ms, eye)
    }

    /// A landing of the drawn client ([`land_change`]) at the render clock.
    pub fn land(&mut self, change: f32, now_ms: i32) {
        self.land.time_ms = now_ms;
        self.land.change = change;
    }

    /// What `CG_OffsetFirstPersonView` adds to the eye's height for the
    /// last landing (`0x30032ca1`-`0x30032db3`): down over 150 ms, back
    /// over 300. A landing stamped ahead of the clock is moved 450 ms back.
    pub fn land_dip(&mut self, now_ms: i32) -> f32 {
        let l = &mut self.land;
        let dt = now_ms.wrapping_sub(l.time_ms);
        if dt < 0 {
            l.time_ms = now_ms - 450;
        }
        let dt = dt as f32;
        l.dip = if dt < 150.0 {
            l.change * (dt / 150.0)
        } else if dt < 450.0 {
            l.change * (1.0 - (dt - 150.0) / 300.0)
        } else {
            0.0
        };
        l.dip
    }

    /// The view kick for the cmd angles and the drawn view, degrees, wire
    /// convention.
    pub fn view_kick(&self) -> [f32; 3] {
        self.recoil.view_kick()
    }

    /// The gun spring's pitch and yaw, degrees, wire convention.
    pub fn gun_kick(&self) -> [f32; 2] {
        self.recoil.gun_kick()
    }

    /// Whether last frame's sight put a scope overlay up.
    pub fn scoped(&self) -> bool {
        self.scoped
    }

    /// Steps both kicks to `now_ms` for `ps`, ahead of the view that reads
    /// them; `None`, dead or not in the world, clears them.
    pub fn step_recoil(&mut self, weapons: &[Option<WeaponDef>], ps: Option<&ViewPs>, now_ms: f64) {
        let held = ps.and_then(|ps| weapons.get(usize::from(ps.weapon))?.as_ref());
        let ms = now_ms as i64;
        let frametime = self
            .last_ms
            .map_or(0, |last| (ms - last).clamp(0, 1000) as i32);
        self.last_ms = Some(ms);
        match ps {
            Some(ps) => self.recoil.step(
                frametime,
                held,
                ps.ads_frac,
                ps.pm_flags & recoil::PMF_OWN_VIEW != 0,
            ),
            None => self.recoil.reset(),
        }
    }

    /// The viewmodel to draw and the horizontal fov both it and the world
    /// are drawn with, for `ps` when the view is the client's own and alive;
    /// `None` draws no viewmodel, as under a scope overlay. `weapons` is the
    /// configstring 7 table, for the clip index and the zoom. `gun` is this
    /// frame's [`scope::GunPose`] and `world_up` world +Z in the drawn
    /// view's (forward, left, up) frame, for the landing's term.
    pub fn frame(
        &mut self,
        cg_fov: f32,
        weapons: &[Option<WeaponDef>],
        ps: Option<&ViewPs>,
        now_ms: f64,
        gun: Option<scope::GunPose>,
        world_up: Vec3,
    ) -> (Option<VmDraw>, f32) {
        self.flash = None;
        self.brass = None;
        let held = ps.and_then(|ps| weapons.get(usize::from(ps.weapon))?.as_ref());
        // A UO seat draws no gun but the jeep passenger's, and zooms only
        // the driver's sight (uo cgame 0x30046570, 0x3003ffc0).
        if let Some(ps) = ps
            && let Some(seat) = ps.seat
            && let Some(fov) = seat.fov(cg_fov, ps.ads_frac, held.is_some_and(|d| d.aim_down_sight))
        {
            self.scoped = false;
            return (None, fov);
        }
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

        let def = &w.def;
        let wanted = weapon::view_anim(def, ps.weap_anim, clip_empty);
        let idle = weapon::view_anim(def, 0, clip_empty);
        let clip_ms = w
            .anims
            .get(&wanted)
            .map_or(0.0, |(a, _)| f64::from(a.duration()) * 1000.0);
        let (anim, ms) = weapon::resolve(def, wanted, ms_in, clip_ms, idle);
        // A clip named but not loaded plays idle, as `--walk` does.
        let (anim, clip) = match w.anims.get(&anim) {
            Some((c, _)) => (anim, c),
            None => (WeaponAnim::Idle, &w.anims[&WeaponAnim::Idle].0),
        };
        let looping = matches!(anim, WeaponAnim::Idle | WeaponAnim::EmptyIdle);
        let pos = clip.frame_pos((ms / 1000.0) as f32, looping);
        w.pose(frac, frac >= 1.0 || trend > 0, anim, pos);
        // two models, hands then gun, in set_viewmodel order
        let bone_sets = (0..2)
            .map(|m| w.pose.skin_matrices(&w.skeleton, m))
            .collect();

        // `0x30036990`'s position, then the landing's quarter along world up
        // (`0x30036b3b`-`0x30036bb7`).
        let gun = gun.unwrap_or_default();
        let [f, l, u] = gun.offset;
        let pos = lean_offset(ps.lean, frac)
            + view_from_ful(Vec3::new(f, l, u) + world_up * (0.25 * self.land.dip));
        let transform = gun_transform(pos, gun.angles);
        let tag = |name: &str| {
            let (pos, rot) = w.pose.bone_world(&w.skeleton, w.skeleton.bone_index(name)?);
            Some((
                transform.transform_point3(pos),
                transform.transform_vector3(rot * Vec3::X),
            ))
        };
        self.flash = tag("tag_flash");
        self.brass = tag("tag_brass");
        // Posed all the same, so the clips run on under the scope.
        let drawn = match self.draw_gun {
            DrawGun::Never => false,
            DrawGun::Unscoped => !self.scoped,
            DrawGun::Always => true,
        };
        if !drawn {
            self.flash = None;
            return (None, fov);
        }
        (
            Some(VmDraw {
                transform,
                light_origin: ps.eye,
                fov_x: fov,
                bone_sets,
            }),
            fov,
        )
    }

    /// The drawn viewmodel's `tag_flash` in world space, for the camera at
    /// `eye` with basis `(forward, right, up)`. The viewmodel shares the
    /// world's fov, so view space maps straight onto the camera basis.
    pub fn muzzle(&self, eye: Vec3, basis: (Vec3, Vec3, Vec3)) -> Option<(Vec3, Vec3)> {
        self.flash.map(|tag| view_to_world(tag, eye, basis))
    }

    /// The posed viewmodel's `tag_brass` and its forward in world space,
    /// as [`Self::muzzle`]: where our own spent cases leave the gun.
    pub fn brass(&self, eye: Vec3, basis: (Vec3, Vec3, Vec3)) -> Option<(Vec3, Vec3)> {
        self.brass.map(|tag| view_to_world(tag, eye, basis))
    }
}

/// A viewmodel tag `(position, forward)` from view space to world space.
fn view_to_world(
    (pos, dir): (Vec3, Vec3),
    eye: Vec3,
    (forward, right, up): (Vec3, Vec3, Vec3),
) -> (Vec3, Vec3) {
    let world = |v: Vec3| right * v.x + up * v.y - forward * v.z;
    (eye + world(pos), world(dir).normalize_or_zero())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::weapon::CG_FOV;

    #[test]
    fn the_intermission_view_sits_at_the_origin() {
        assert_eq!(eye_height(PM_INTERMISSION, 60.0), 0.0);
        assert_eq!(eye_height(4, 0.0), 0.0);
        assert_eq!(eye_height(6, 8.0), 8.0);
    }

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
            lean: 0.0,
            ammoclip: [0; NUM_AMMO],
            pm_type: 0,
            mounted: false,
            pm_flags: recoil::PMF_OWN_VIEW,
            eye: Vec3::ZERO,
            seat: None,
        }
    }

    /// An in-game `vid_restart` drops the cgame's view state (a landing
    /// dip in flight) and keeps the connection's layout and `cg_drawGun`.
    #[test]
    fn a_cgame_restart_drops_the_view_state() {
        let uo = vcod_common::net::protocol::PROTOCOL_V22;
        let mut v = OnlineView {
            draw_gun: DrawGun::Never,
            ..OnlineView::default()
        };
        v.set_protocol(&uo);
        v.land(-24.0, 1000);
        assert!(v.land_dip(1100) < 0.0);
        v.restart_cgame();
        assert_eq!(v.land_dip(1100), 0.0);
        assert_eq!(v.models(), uo.cs.models());
        assert_eq!(v.draw_gun, DrawGun::Never);
    }

    #[test]
    fn rig_names_follow_weapon_and_hands() {
        let cs = configstrings();
        let carbine_us = Some(("m1carbine_mp", Some("xmodel/viewmodel_hands_us")));
        assert_eq!(rig_names(&cs, CS_MODELS_V1, 1, 82), carbine_us);
        assert_ne!(rig_names(&cs, CS_MODELS_V1, 2, 82), carbine_us, "weapon");
        assert_ne!(rig_names(&cs, CS_MODELS_V1, 1, 52), carbine_us, "hands");
        assert_eq!(
            rig_names(&cs, CS_MODELS_V1, 1, 0),
            Some(("m1carbine_mp", None))
        );
    }

    #[test]
    fn prewarm_names_every_registered_weapon_with_every_hands_model() {
        let mut cs = configstrings();
        // Items 0, 1 and 3 (the colt is not registered), and 68, health.
        cs[8] = "b00000000000000001".to_string();
        assert_eq!(registered_items(&cs[8]).collect::<Vec<_>>(), [1, 3, 68]);
        assert_eq!(
            prewarm_names(&cs, CS_MODELS_V1),
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
        assert_eq!(rig_names(&cs, CS_MODELS_V1, 0, 82), None);
        assert_eq!(
            rig_names(&cs, CS_MODELS_V1, 9, 82),
            None,
            "past the end of CS 7"
        );

        let mut view = OnlineView::default();
        let fs = Pk3Fs::empty();
        assert!(view.sync_rig(&fs, &cs, &ps(0, 82)).is_none());
        assert_eq!(view.built_for, Some(None));
        let (draw, fov) = view.frame(CG_FOV, &[], Some(&ps(0, 82)), 0.0, None, Vec3::Z);
        assert!(draw.is_none());
        assert_eq!(fov, CG_FOV);
    }

    #[test]
    fn not_drawing_restarts_the_clip_clock() {
        let mut view = OnlineView::default();
        view.clock.update(10 | 512, 0.5, 0.0);
        view.trend = 1;
        view.frame(CG_FOV, &[], None, 100.0, None, Vec3::Z);
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

    /// A new spawn count or followed client clears the kick; the same pair
    /// leaves it running.
    #[test]
    fn a_respawn_clears_the_kick() {
        let mut view = OnlineView::default();
        let mut def = WeaponDef::default();
        def.aim.view_kick_pitch = [[40.0; 2]; 2];
        let kicked = |view: &mut OnlineView| {
            view.recoil.fire(&def, 0.0);
            view.recoil.step(5, None, 0.0, true);
            assert_ne!(view.view_kick(), [0.0; 3]);
        };
        view.track_spawn((0, 1));
        kicked(&mut view);
        let kick = view.view_kick();
        view.track_spawn((0, 1));
        assert_eq!(view.view_kick(), kick, "same life");
        view.track_spawn((0, 2));
        assert_eq!(view.view_kick(), [0.0; 3], "respawn");
        assert_eq!(view.gun_kick(), [0.0; 2]);
        kicked(&mut view);
        view.track_spawn((3, 2));
        assert_eq!(view.view_kick(), [0.0; 3], "a new followed client");
        kicked(&mut view);
        view.restart_cgame();
        assert_eq!(view.view_kick(), [0.0; 3], "map load");
    }

    /// A full lean right moves the gun 1.6 units right and a little up at
    /// the hip and rolls it 2 degrees; the sight takes the offset away but
    /// not the roll.
    #[test]
    fn the_lean_moves_and_rolls_the_gun() {
        let at_hip = lean_offset(1.0, 0.0);
        assert!((at_hip.x - 1.6 * 2f32.to_radians().cos()).abs() < 1e-6);
        assert!((at_hip.y - 1.6 * 2f32.to_radians().sin()).abs() < 1e-6);
        assert_eq!(at_hip.z, 0.0);
        let left = lean_offset(-1.0, 0.0);
        assert!((left.x + at_hip.x).abs() < 1e-6 && (left.y - at_hip.y).abs() < 1e-6);
        // GetLeanFraction(0.5) = 0.75, halved again by the sight.
        assert!((lean_offset(0.5, 0.5).x - 0.6 * 1.5f32.to_radians().cos()).abs() < 1e-6);
        assert_eq!(lean_offset(1.0, 1.0), Vec3::ZERO);
        assert_eq!(lean_offset(0.0, 0.0), Vec3::ZERO);
    }

    /// Zero angles hold the model's axes on the view's; wire yaw swings
    /// the muzzle left, wire pitch drops it, wire roll drops the right side.
    #[test]
    fn the_gun_angles_turn_the_model_in_view_space() {
        let at = |angles: [f32; 3], v: Vec3| gun_transform(Vec3::ZERO, angles).transform_vector3(v);
        let close = |a: Vec3, b: Vec3| a.abs_diff_eq(b, 1e-5);
        assert!(close(at([0.0; 3], Vec3::X), Vec3::NEG_Z), "forward");
        assert!(close(at([0.0; 3], Vec3::Y), Vec3::NEG_X), "left");
        assert!(close(at([0.0; 3], Vec3::Z), Vec3::Y), "up");
        assert!(at([0.0, 10.0, 0.0], Vec3::X).x < 0.0);
        assert!(at([10.0, 0.0, 0.0], Vec3::X).y < 0.0);
        assert!(at([0.0, 0.0, 10.0], Vec3::NEG_Y).y < 0.0);
        let moved = gun_transform(Vec3::new(1.0, 2.0, 3.0), [0.0; 3]);
        assert_eq!(moved.w_axis.truncate(), Vec3::new(1.0, 2.0, 3.0));
    }

    /// A plain landing sinks the eye by its parm; a damaging one by its
    /// height's (h - 12) / 26 * 4 + 4, capped at 24; the dip goes down over
    /// 150 ms and back over 300.
    #[test]
    fn a_landing_dips_the_eye() {
        use vcod_common::net::event_ids::{
            EV_FOOTSTEP_RUN_BASE, EV_LANDING_BASE, EV_LANDING_PAIN_BASE,
        };
        let fall = vcod_common::pmove::FallHeights {
            min: 256.0,
            max: 480.0,
        };
        assert_eq!(land_change(EV_LANDING_BASE + 6, 8, fall), Some(-8.0));
        // 50% is 368 units: (356 / 26 + 1) * 4 = 58.8, capped.
        assert_eq!(land_change(EV_LANDING_PAIN_BASE, 50, fall), Some(-24.0));
        let low = vcod_common::pmove::FallHeights {
            min: 0.0,
            max: 100.0,
        };
        // 25 units: (13 / 26 + 1) * 4 = 6.
        assert_eq!(land_change(EV_LANDING_PAIN_BASE + 22, 25, low), Some(-6.0));
        assert_eq!(land_change(EV_LANDING_PAIN_BASE, 12, low), None);
        assert_eq!(land_change(EV_FOOTSTEP_RUN_BASE, 8, fall), None);

        let mut view = OnlineView::default();
        view.land(-12.0, 1000);
        assert_eq!(view.land_dip(1000), 0.0);
        assert_eq!(view.land_dip(1075), -6.0);
        assert_eq!(view.land_dip(1150), -12.0);
        assert_eq!(view.land_dip(1300), -6.0);
        assert_eq!(view.land_dip(1450), 0.0);
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
            let (draw, fov) = view.frame(CG_FOV, &weapons, Some(state), now, None, Vec3::Z);
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
        assert!(view.brass.is_some(), "and a tag_brass");

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
            let (draw, _) =
                scoped.frame(CG_FOV, &weapons, Some(&up), i as f64 * 16.0, None, Vec3::Z);
            assert_eq!(draw.is_none(), frac >= 0.9, "at {frac}");
            assert_eq!(scoped.scoped(), frac >= 0.9);
        }
        // `cg_drawGun 2` keeps it under the overlay, 0 hides it always.
        scoped.draw_gun = DrawGun::from_cvar(2);
        let (draw, _) = scoped.frame(CG_FOV, &weapons, Some(&up), 64.0, None, Vec3::Z);
        assert!(draw.is_some() && scoped.scoped());
        scoped.draw_gun = DrawGun::from_cvar(0);
        up.ads_frac = 0.0;
        let (draw, _) = scoped.frame(CG_FOV, &weapons, Some(&up), 80.0, None, Vec3::Z);
        assert!(draw.is_none() && !scoped.scoped());
        cs[7] = "m1carbine_mp colt_mp mosin_nagant_mp".to_string();

        // A team change swaps the hands and rebuilds.
        cs[CS_MODELS_V1 + 82] = "xmodel/viewmodel_hands_russian".to_string();
        assert!(view.sync_rig(&fs, &cs, &ps(1, 82)).is_some());
    }
}
