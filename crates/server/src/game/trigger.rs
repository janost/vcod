//! The map's triggers as a host-side table, and the two-stage test the touch
//! pass runs against them: a box query, then a contact against the trigger's
//! own brushes.

use crate::game::host::GameHost;
use glam::Vec3;
use std::collections::BTreeMap;
use vcod_gsc::{Atom, Cx, EntId, Host, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    Multiple,
    Once,
    Use,
    LookAt,
    Hurt,
    /// Fired by what damages it, never touched: `Activate_trigger_damage`
    /// (`crate::game::damage_trigger`), its keys in [`Trigger::spec`].
    Damage,
    /// United Offensive's `trigger_mount`, a map's or `spawnTriggerMount`'s:
    /// an LMG deploys on it, crouched with spawnflag 1, and a touch notifies
    /// `"trigger"` every time (`Touch_trigger_mount` is `G_Trigger`,
    /// docs/research/coduo-movement.md 11).
    Mount {
        crouch: bool,
    },
}

/// What a trigger occupies: the submodel's own box, and the model whose
/// brushes the exact test measures against. Both are local, so the absolute
/// shape is taken around the entity's current origin rather than cached.
#[derive(Clone, Copy, Debug)]
pub struct TriggerShape {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// Looked up in `GameHost::model_brushes`. `None` on a trigger with no
    /// brush model, which then has only its box.
    pub model: Option<u32>,
}

impl TriggerShape {
    /// A box with no brushes under it, which is every shape a script builds.
    pub fn boxed(mins: [f32; 3], maxs: [f32; 3]) -> Self {
        Self {
            mins,
            maxs,
            model: None,
        }
    }
}

/// One trigger.
#[derive(Clone, Copy, Debug)]
pub struct Trigger {
    pub kind: TriggerKind,
    pub shape: TriggerShape,
    /// The `wait` key in milliseconds on a `trigger_multiple` (default 500)
    /// or `trigger_once` (always -1000), which only decides whether the first
    /// touch spends the trigger (movers doc 17). On a `Hurt`, the touch
    /// cadence [`Triggers::arm_hurt`] puts here off its spawnflags. 0 on
    /// the other kinds.
    pub wait_ms: i32,
    /// Level-clock time a `Hurt` may fire again.
    pub next_fire_ms: i32,
    /// A `trigger_multiple` or `trigger_once` whose `wait` was not above 0
    /// has fired: `Touch_Multi` cleared its touch function and its free is
    /// scheduled. A `Hurt` has no touch function: spawned `START_OFF`
    /// (spawnflag 1), hurt once under 0x20, or toggled off by `hurt_use`.
    pub spent: bool,
    /// A `Hurt`'s `life` key in seconds (`ent+0x274`, 0 when none), which
    /// `hurt_use` turns into a free.
    pub life: f32,
    /// A `Hurt`'s spawnflags as its last touch read them.
    pub hurt_flags: i32,
    /// A `Hurt`'s sound alias (`ent+0x16e`), counted from `CS_SOUNDS` as
    /// `EV_SOUND_ALIAS`'s parm is; 0 on every other kind.
    pub sound: i32,
    /// A `trigger_use`'s cursor hint (`ent+0xdc`): `SP_trigger_use` stores 2,
    /// `HINT_ACTIVATE`. 0 on every other kind.
    pub cursor_hint: i32,
    /// Its hint-string slot (`ent+0xd8`) as `serverCursorHintString` carries
    /// it, -1 for none (retail's 0xff). Written by `setHintString`.
    pub hint_string: i32,
    /// A `trigger_damage`'s keys; `None` on every other kind.
    pub spec: Option<DamageSpec>,
    /// CoD2's `trigger_radius` and `trigger_disk`: no brush, a cylinder
    /// around the origin; `None` on every other trigger.
    pub round: Option<Round>,
    /// CoD2's `setTeamForTrigger` (`ent+0x167`): 0 any team, 1 axis, 2
    /// allies; the use list skips a trigger of another team.
    pub team: u8,
    /// CoD2's `clientClaimTrigger` (`ent+0x1b4`): the client the trigger
    /// is claimed by, `None` for retail's 0x3ff.
    pub claimer: Option<u16>,
}

/// CoD2's round triggers (cod2_lnxded `SP_trigger_radius` 0x811c6e0,
/// `SP_trigger_disk` 0x811c880, VERIFIED): `r.mins`/`r.maxs` are
/// `(-r, -r, 0)..(r, r, height)` with `r.svFlags` 0x21, or for a disk
/// `(-(r + 64), .., -100000)..(r + 64, .., 100000)` with 0x41. The touch
/// test inside the box is the horizontal distance to the axis, which the
/// two flags select (INFERRED: the flag bits, not the contact code, are
/// read).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Round {
    pub radius: f32,
}

impl Round {
    /// Does a box `lo..hi` reach within `radius` of the vertical axis
    /// through `origin`? Its closest point in the horizontal plane.
    pub fn contacts(&self, origin: [f32; 3], lo: [f32; 3], hi: [f32; 3]) -> bool {
        let dx = origin[0].clamp(lo[0], hi[0]) - origin[0];
        let dy = origin[1].clamp(lo[1], hi[1]) - origin[1];
        dx * dx + dy * dy <= self.radius * self.radius
    }
}

/// What `SP_trigger_damage` (1.1d 0x65278, 1.5b 0x6ac1e, UO 0x76f59) reads
/// off a `trigger_damage`'s keys, and the grenade-touch bit
/// `enableGrenadeTouchDamage` sets (docs/research/cod11-combat.md 14.8).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageSpec {
    /// `wait` and `random` in seconds (`G_SpawnFloat`, defaults "0.5" and
    /// "0"), `random` pulled under `wait` when it reaches it.
    pub wait: f32,
    pub random: f32,
    pub accumulate: i32,
    pub threshold: i32,
    pub spawnflags: i32,
    /// UO's `teamname` key: 1 `axis`, 2 `allies`, else 0.
    pub team: i32,
    /// `flags & 0x8000`.
    pub grenade_touch: bool,
}

// `EntId` orders by entity number first, and entity numbers are handed out
// in spawn order, so `iter()` walks a map's triggers in the order they were
// loaded in.
#[derive(Default)]
pub struct Triggers {
    rows: BTreeMap<EntId, Trigger>,
}

/// `r.contents` a `trigger_multiple` or `trigger_once` links with (0x64d79).
pub const CONTENTS_TRIGGER: i32 = 0x4000_0000;
/// `trap_EntitiesInBox`' mask in `G_TouchTriggers` (0x3f918), and the
/// contents `trigger_hurt` and `trigger_damage` take (0x64f6c, 0x653cd).
pub const CONTENTS_TOUCH_MASK: i32 = 0x405c_0008;
/// A `trigger_mount`'s contents: 0x4000, or 0x400000 for a crouched one,
/// over `SP_trigger_mount_no_brush`'s 0x401c0008 for a script's box
/// (game.mp.uo 0x87754) or over `InitTrigger`'s 0x40000000 for a map's
/// brush (`SP_trigger_mount` 0x876d5).
pub fn mount_contents(crouch: bool, brush: bool) -> i32 {
    let base = if brush { CONTENTS_TRIGGER } else { 0x401c_0008 };
    base | if crouch { 0x40_0000 } else { 0x4000 }
}

/// `SP_trigger_lookat` (0x65e03).
pub const CONTENTS_LOOKAT: i32 = 0x2000_0000;
/// `trigger_use` (0x5745a).
pub const CONTENTS_USE: i32 = 0x20_0000;

/// The `dmg` default, the spawnflags and the two touch intervals, from
/// docs/research/cod11-gsc-object-model.md 8.1 and cod11-combat.md 5.9.
pub const HURT_DEFAULT_DAMAGE: i32 = 5;
const HURT_START_OFF: i32 = 0x1;
pub const HURT_SILENT: i32 = 0x4;
const HURT_NO_PROTECTION: i32 = 0x8;
const HURT_SLOW: i32 = 0x10;
const HURT_ONCE: i32 = 0x20;
const HURT_INTERVAL_MS: i32 = 100;
const HURT_SLOW_INTERVAL_MS: i32 = 1000;

/// `HINT_ACTIVATE`, slot 2 of `hintStrings`: what `SP_trigger_use` (0x5742c)
/// stores as a `trigger_use`'s cursor hint when the map gives no
/// `cursorhint` key, and no stock map does
/// (docs/research/cod11-gsc-object-model.md, the `serverCursorHint` section).
pub const HINT_ACTIVATE: i32 = 2;

/// The mod `hurt_touch` damages with (the same doc section).
pub const MOD_TRIGGER_HURT: &str = "MOD_TRIGGER_HURT";

impl Triggers {
    pub fn register(&mut self, id: EntId, kind: TriggerKind, shape: TriggerShape, wait_ms: i32) {
        self.rows.insert(
            id,
            Trigger {
                kind,
                shape,
                wait_ms,
                next_fire_ms: 0,
                spent: false,
                hurt_flags: 0,
                life: 0.0,
                sound: 0,
                cursor_hint: if kind == TriggerKind::Use {
                    HINT_ACTIVATE
                } else {
                    0
                },
                hint_string: -1,
                spec: None,
                round: None,
                team: 0,
                claimer: None,
            },
        );
    }

    /// A CoD2 `trigger_radius` or `trigger_disk`: a `trigger_multiple` with
    /// a round shape (both spawn functions set `Touch_Multi`'s handler 1).
    pub fn register_round(&mut self, id: EntId, shape: TriggerShape, wait_ms: i32, round: Round) {
        self.register(id, TriggerKind::Multiple, shape, wait_ms);
        if let Some(t) = self.rows.get_mut(&id) {
            t.round = Some(round);
        }
    }

    /// A `trigger_damage` with its keys.
    pub fn register_damage(&mut self, id: EntId, shape: TriggerShape, spec: DamageSpec) {
        self.register(id, TriggerKind::Damage, shape, 0);
        if let Some(t) = self.rows.get_mut(&id) {
            t.spec = Some(spec);
        }
    }

    /// A `trigger_hurt` with its sound alias and `life` key
    /// (`SP_trigger_hurt`, 0x64ef8), touchable unless its spawnflags hold
    /// `START_OFF` (1). Its damage, cadence and flags are not kept here:
    /// `hurt_touch` (0x64dc4) reads the entity's `dmg` and `spawnflags` at
    /// every touch ([`Triggers::arm_hurt`], cod11-combat.md 5.9).
    ///
    /// Not modelled, and docs/research/cod11-gsc-object-model.md 8.1 says
    /// why: `hurt_touch` tests a byte on the *toucher* before its timestamp,
    /// so ours hurts a dead player where retail may not.
    pub fn register_hurt(
        &mut self,
        id: EntId,
        shape: TriggerShape,
        sound: i32,
        spawnflags: i32,
        life: f32,
    ) {
        self.register(id, TriggerKind::Hurt, shape, HURT_INTERVAL_MS);
        if let Some(t) = self.rows.get_mut(&id) {
            t.sound = sound;
            t.spent = spawnflags & HURT_START_OFF != 0;
            t.life = life;
        }
    }

    /// `hurt_use` (1.1d 0x65b94, 1.5b 0x7a605, UO 0x868da): toggles the
    /// hurt's touch function and, under a non-zero `life`, returns the
    /// level time past which `hurt_think` frees it. `None` for anything
    /// that is not a `Hurt`.
    pub fn use_hurt(&mut self, id: EntId, now_ms: i32) -> Option<Option<f32>> {
        let t = self
            .rows
            .get_mut(&id)
            .filter(|t| t.kind == TriggerKind::Hurt)?;
        t.spent = !t.spent;
        Some((t.life != 0.0).then_some(now_ms as f32 + t.life * 1000.0))
    }

    /// `hurt_touch`'s read of the trigger's `spawnflags` ahead of
    /// [`Triggers::fire`]: the cadence, slow (0x10) or not, and the once
    /// flag (0x20). Returns the damage flags the touch hurts with,
    /// `DFLAG_NO_PROTECTION` under 0x8.
    pub fn arm_hurt(&mut self, id: EntId, spawnflags: i32) -> i32 {
        if let Some(t) = self.rows.get_mut(&id) {
            t.hurt_flags = spawnflags;
            t.wait_ms = if spawnflags & HURT_SLOW == 0 {
                HURT_INTERVAL_MS
            } else {
                HURT_SLOW_INTERVAL_MS
            };
        }
        if spawnflags & HURT_NO_PROTECTION == 0 {
            0
        } else {
            crate::game::combat::DFLAG_NO_PROTECTION
        }
    }

    pub fn remove(&mut self, id: EntId) {
        self.rows.remove(&id);
    }

    pub fn get(&self, id: EntId) -> Option<&Trigger> {
        self.rows.get(&id)
    }

    pub fn get_mut(&mut self, id: EntId) -> Option<&mut Trigger> {
        self.rows.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntId, &Trigger)> {
        self.rows.iter().map(|(id, t)| (*id, t))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Whether a touch fires the `"trigger"` notify, and whether it was the
    /// last one. `Touch_Multi` (0x65a18) notifies on every touch and only
    /// then looks at `wait`: above 0 it arms a think nothing reads, not
    /// above 0 it clears the touch function and frees the entity 100 ms on,
    /// and a trigger whose think is `enableLinkTo`'s (`linked`) skips both
    /// (movers doc 17). A `Hurt` keeps `hurt_touch`'s timestamp gate. A
    /// lookat fires every time it is aimed at (`G_Trigger` gates nothing).
    pub fn fire(&mut self, id: EntId, now_ms: i32, linked: bool) -> Fire {
        let Some(t) = self.rows.get_mut(&id) else {
            return Fire::Refused;
        };
        match t.kind {
            TriggerKind::Multiple | TriggerKind::Once => {
                if t.spent {
                    Fire::Refused
                } else if linked || t.wait_ms > 0 {
                    Fire::Fired
                } else {
                    t.spent = true;
                    Fire::Spent
                }
            }
            TriggerKind::Hurt => {
                if t.spent || now_ms < t.next_fire_ms {
                    return Fire::Refused;
                }
                t.next_fire_ms = now_ms + t.wait_ms;
                // 0x20 clears the touch function after the damage.
                t.spent = t.hurt_flags & HURT_ONCE != 0;
                Fire::Fired
            }
            TriggerKind::LookAt
            | TriggerKind::Use
            | TriggerKind::Damage
            | TriggerKind::Mount { .. } => Fire::Fired,
        }
    }
}

/// What [`Triggers::fire`] made of a touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fire {
    Refused,
    Fired,
    /// Fired, and the trigger is spent: the caller schedules its free
    /// [`SPENT_FREE_MS`] out.
    Spent,
}

impl Fire {
    pub fn fired(self) -> bool {
        self != Fire::Refused
    }
}

/// `Touch_Multi`'s `nextthink = level.time + 100` ahead of `G_FreeEntity`
/// (0x65b3f).
pub const SPENT_FREE_MS: i32 = 100;

/// `SP_trigger_multiple`'s `G_SpawnFloat("wait", "0.5")` (0x64c6e) in
/// milliseconds, and the -1 `SP_trigger_once` stores (0x65c16, `.rodata`
/// 0x79ad8).
pub const MULTIPLE_DEFAULT_WAIT_MS: i32 = 500;
pub const ONCE_WAIT_MS: i32 = -1000;

/// `SP_trigger_hurt` for a script `spawn("trigger_hurt", origin,
/// spawnflags)`: no `model` key, so the brush-model setter links inline
/// model 0, a copy of the whole world at the origin, as a scripted
/// `trigger_damage` does (cod11-combat.md 14.8); no spawn keys, so the
/// default alias and `dmg` 5 (cod11-combat.md 5.9).
pub fn spawn_scripted_hurt(host: &mut GameHost, cx: &mut Cx, id: EntId, spawnflags: i32) {
    let (mins, maxs) = host.model_bounds.first().copied().unwrap_or_default();
    let shape = TriggerShape {
        mins,
        maxs,
        model: Some(0),
    };
    let sound = hurt_sound_index(host, "world_hurt_me");
    host.triggers
        .register_hurt(id, shape, sound, spawnflags, 0.0);
    default_hurt_dmg(host, cx, id);
    link(host, cx, id, spawnflags);
}

/// `SP_trigger_multiple` and `SP_trigger_once` for a script `spawn`: no
/// keys, so `wait` 0.5 (a `trigger_once` always -1), and no `model`, so
/// inline model 0, a copy of the world at the origin, as
/// [`spawn_scripted_hurt`] links (cod11-combat.md 5.9, "useBy").
pub fn spawn_scripted_multi(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    kind: TriggerKind,
    spawnflags: i32,
) {
    let (mins, maxs) = host.model_bounds.first().copied().unwrap_or_default();
    let shape = TriggerShape {
        mins,
        maxs,
        model: Some(0),
    };
    let wait = if kind == TriggerKind::Once {
        ONCE_WAIT_MS
    } else {
        MULTIPLE_DEFAULT_WAIT_MS
    };
    host.triggers.register(id, kind, shape, wait);
    link(host, cx, id, spawnflags);
}

/// `Use_Multi` (1.1d 0x6594c, 1.5b 0x7a2f2, UO 0x865c7), `Touch_Multi`'s
/// arm without the notify: unless the trigger's think is `enableLinkTo`'s or
/// already armed, a `wait` above 0 arms `multi_wait` (which gates nothing),
/// and any other spends the trigger and frees it 100 ms on
/// (cod11-movers.md 17). Returns false for anything not a
/// `trigger_multiple` or `trigger_once`.
pub fn use_multi(host: &mut GameHost, id: EntId) -> bool {
    let Some(t) = host
        .triggers
        .get(id)
        .filter(|t| matches!(t.kind, TriggerKind::Multiple | TriggerKind::Once))
        .copied()
    else {
        return false;
    };
    let armed = host.ents.get(id).is_some_and(|e| e.nextthink != 0);
    if t.spent || armed || host.links.is_enabled(id) {
        return true;
    }
    let now = host.level_time_ms;
    if t.wait_ms > 0 {
        use crate::game::entity::ThinkFn;
        host.ents.schedule(id, ThinkFn::MultiWait, now + t.wait_ms);
    } else {
        if let Some(t) = host.triggers.get_mut(id) {
            t.spent = true;
        }
        let free = crate::game::entity::ThinkFn::Free;
        host.ents.schedule(id, free, now + SPENT_FREE_MS);
    }
    true
}

/// `G_SoundAliasIndex` of a `trigger_hurt`'s `sound`, counted from
/// `CS_SOUNDS`; 0 (no sound) when the range is full.
pub fn hurt_sound_index(host: &mut GameHost, alias: &str) -> i32 {
    use crate::configstrings::CsRange;
    match host
        .allocators
        .index(&mut host.configstrings, CsRange::SoundAlias, alias)
    {
        Ok(slot) => (slot - host.allocators.layout().sounds()) as i32,
        Err(e) => {
            log::warn!("gsc: sound alias {alias:?} not indexed: {e:?}");
            0
        }
    }
}

/// `SP_trigger_hurt`'s `if (ent->damage == 0) ent->damage = 5` (0x64fbb):
/// a `dmg` key of 0 or none reads 5.
pub fn default_hurt_dmg(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let dmg = cx.intern_folded("dmg");
    if matches!(
        host.get_field(cx, id, dmg),
        Value::Int(0) | Value::Undefined
    ) {
        let _ = host.set_field(cx, id, dmg, Value::Int(HURT_DEFAULT_DAMAGE));
    }
}

pub fn kind_of(classname: &str) -> Option<TriggerKind> {
    Some(match classname {
        "trigger_multiple" => TriggerKind::Multiple,
        "trigger_once" => TriggerKind::Once,
        "trigger_use" => TriggerKind::Use,
        "trigger_lookat" => TriggerKind::LookAt,
        "trigger_hurt" => TriggerKind::Hurt,
        "trigger_damage" => TriggerKind::Damage,
        _ => return None,
    })
}

/// [`kind_of`] under CoD2's spawn table: its `trigger_use_touch` runs
/// `SP_trigger_use`'s function (cod2_lnxded 0x8110052 and 0x811003e both
/// call 0x810fe50, VERIFIED), and its two round triggers are
/// [`Triggers::register_round`]'s.
pub fn kind_of_cod2(classname: &str) -> Option<TriggerKind> {
    match classname {
        "trigger_use_touch" => Some(TriggerKind::Use),
        "trigger_radius" | "trigger_disk" => Some(TriggerKind::Multiple),
        _ => kind_of(classname),
    }
}

/// A box offset to an origin: the shape `abs_bounds` and the point/player
/// cases share.
fn offset_bounds(origin: [f32; 3], mins: [f32; 3], maxs: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let lo = [
        origin[0] + mins[0],
        origin[1] + mins[1],
        origin[2] + mins[2],
    ];
    let hi = [
        origin[0] + maxs[0],
        origin[1] + maxs[1],
        origin[2] + maxs[2],
    ];
    (lo, hi)
}

pub fn abs_bounds(origin: [f32; 3], t: &Trigger) -> ([f32; 3], [f32; 3]) {
    offset_bounds(origin, t.shape.mins, t.shape.maxs)
}

/// The player's own clip box, which is what a touch's exact
/// `trap_EntityContact` test measures against and not what the broad phase
/// queries (docs/research/cod11-gsc-object-model.md section 22). Half-width 15
/// and 0..70 standing: `playerMins`/`playerMaxs` as `G_SetPlayerSize` writes
/// them off `g_bounds_width` and `g_bounds_height_standing` before any script
/// runs (cod11-gsc-object-model.md 16.2), what `placeSpawnpoint` traces too.
pub const PLAYER_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
pub const PLAYER_MAXS: [f32; 3] = [15.0, 15.0, vcod_common::pmove::HEIGHT_STAND];

/// `origin` is taken as an already-interned atom rather than folded here:
/// the touch pass calls this once per client plus once per trigger row per
/// cmd, and folding lowercases and allocates every time.
fn entity_origin(host: &mut GameHost, cx: &mut Cx, id: EntId, origin: Atom) -> [f32; 3] {
    match host.get_field(cx, id, origin) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

/// An entity's absolute box: a registered trigger's submodel box around its
/// current origin, a client's player box as its last cmd's stance left it
/// (`GameHost::client_box`), and a point box for everything else, which is
/// what an unset `r.mins`/`r.maxs` gives retail.
pub fn entity_abs_bounds(host: &mut GameHost, cx: &mut Cx, id: EntId) -> ([f32; 3], [f32; 3]) {
    let origin = cx.intern_folded("origin");
    abs_bounds_with_atom(host, cx, id, origin)
}

fn abs_bounds_with_atom(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    origin_atom: Atom,
) -> ([f32; 3], [f32; 3]) {
    let origin = entity_origin(host, cx, id, origin_atom);
    if let Some(t) = host.triggers.get(id) {
        return abs_bounds(origin, t);
    }
    let is_client = host.ents.get(id).is_some_and(|e| e.client.is_some());
    let (mins, maxs) = if is_client {
        host.client_box(id.0 as usize)
    } else {
        ([0.0; 3], [0.0; 3])
    };
    offset_bounds(origin, mins, maxs)
}

/// The candidate box retail hands `trap_EntitiesInBox`, taken around the
/// client's origin rather than around its clip box
/// (docs/research/cod11-gsc-object-model.md section 22).
pub(crate) const TOUCH_BOX: [f32; 3] = [40.0, 40.0, 52.0];

/// One submodel brush as clip planes, model-local the way `Trigger::mins` is:
/// the entity's current origin is applied at test time, so a relocated
/// trigger takes its brushes with it. Point p is inside iff n·p <= d for
/// every plane.
#[derive(Clone)]
pub struct BrushHull {
    pub planes: Vec<(Vec3, f32)>,
}

/// Every lump-27 submodel's brushes as clip planes, indexed by model number,
/// for the exact stage of the touch pass. Model 0 is the world and is left
/// empty: no trigger uses it and its brushes are the map's whole solid
/// geometry.
///
/// Trigger brushes are not in `CollisionWorld` -- it drops them on purpose,
/// since retail leaves a trigger hollow to movement -- so they are decoded
/// straight off the lump here, through the same side decode the clip uses.
pub fn model_brush_hulls(bsp: &vcod_common::mapworld::MapWorld) -> Vec<Vec<BrushHull>> {
    bsp.models
        .iter()
        .enumerate()
        .map(|(mi, model)| {
            if mi == 0 {
                return Vec::new();
            }
            let range =
                model.first_brush as usize..(model.first_brush + model.num_brushes) as usize;
            bsp.brushes[range]
                .iter()
                .map(|b| {
                    let mut planes = Vec::new();
                    vcod_common::collision::brush_side_planes(bsp, b, Vec3::ZERO, &mut planes);
                    BrushHull { planes }
                })
                .collect()
        })
        .collect()
}

/// Does an absolute box touch any of a model's brushes, the model sitting at
/// `origin`? Q3's separating-plane test, which is what a contact test reduces
/// to for an AABB: the box clears a brush only if some plane separates it, so
/// a brush with no separating plane is in contact.
pub(crate) fn box_contacts_hulls(
    centre: Vec3,
    half: Vec3,
    origin: Vec3,
    hulls: &[BrushHull],
) -> bool {
    let c = centre - origin;
    hulls.iter().any(|b| {
        b.planes.iter().all(|&(n, d)| {
            let r = n.x.abs() * half.x + n.y.abs() * half.y + n.z.abs() * half.z;
            n.dot(c) - r <= d
        })
    })
}

/// `box_contacts_hulls` for the capsule retail makes of a box when the
/// boxed entity carries `SVF_CAPSULE` (0x200, every client): radius the
/// smaller of the half width and half height, spheres `half.z - radius`
/// above and below the centre. An axial side sees the capsule as its box;
/// an angled one sees the nearer sphere pushed out by the radius
/// (docs/research/cod11-gsc-object-model.md 22.2), so a player's corner
/// reaches 21 into a 45-degree wall where its capsule reaches 15.
pub(crate) fn capsule_contacts_hulls(
    centre: Vec3,
    half: Vec3,
    origin: Vec3,
    hulls: &[BrushHull],
) -> bool {
    let c = centre - origin;
    let r = half.x.min(half.z);
    let off = half.z - r;
    hulls.iter().any(|b| {
        b.planes
            .iter()
            .all(|&(n, d)| n.dot(c) - n.z.abs() * off - r <= d)
    })
}

/// Everything client `slot` standing at `origin` touches, in the order
/// `G_TouchTriggers` (0x3f88c) meets it: the `trap_EntitiesInBox` walk of the
/// area tree around the origin with mask 0x405c0008 (combat doc 14.7), then
/// per entity its own test. An item (`eType` 3) is touched by
/// `BG_PlayerTouchesItem`'s proximity box (`pickup::touches`); anything else
/// by `trap_EntityContact`, the client's own clip box against the trigger's
/// *brushes* (docs/research/cod11-gsc-object-model.md section 22). The clip
/// box is `r.mins`/`r.maxs`, which `ClientThink_real` copies from pmove's
/// stance box ahead of the pass (22.3): 70, 50 or 30 high. Neither box
/// contains the other -- the candidate reaches 52 below the feet and a
/// standing clip box 70 above them -- so both have to hold.
///
/// Only an entity with a touch function is touched (0x3f9cd): a
/// `trigger_damage` sets none, and a `trigger_lookat` or `trigger_use` has
/// contents outside the mask (22.1).
///
/// `origin` is `ps.origin`, which both of `G_TouchTriggers`' boxes are built
/// on, not the client's `origin` field, which holds the snapped
/// `r.currentOrigin` while the pass runs (combat doc 5.5).
///
/// A trigger whose model has no brushes decoded (script-registered, or a map
/// loaded without a BSP) keeps the box as its exact test.
pub fn touched(host: &mut GameHost, cx: &mut Cx, slot: usize, origin: [f32; 3]) -> Vec<EntId> {
    let origin_atom = cx.intern_folded("origin");
    let candidate = offset_bounds(
        origin,
        [-TOUCH_BOX[0], -TOUCH_BOX[1], -TOUCH_BOX[2]],
        TOUCH_BOX,
    );
    let (mins, maxs) = host.client_box(slot);
    let exact = offset_bounds(origin, mins, maxs);
    let ids: Vec<EntId> = host
        .area
        .entities_in_box(candidate.0, candidate.1, CONTENTS_TOUCH_MASK)
        .into_iter()
        .filter_map(|n| host.ents.handle(n))
        .collect();
    ids.into_iter()
        .filter(|id| {
            if host.ents.get(*id).is_some_and(|e| e.item.is_some()) {
                let at = entity_origin(host, cx, *id, origin_atom);
                return crate::game::pickup::touches(origin, at);
            }
            let Some(t) = host.triggers.get(*id).copied() else {
                return false;
            };
            if matches!(
                t.kind,
                TriggerKind::LookAt | TriggerKind::Use | TriggerKind::Damage
            ) {
                return false;
            }
            box_contacts(host, cx, *id, &t, exact)
        })
        .collect()
}

/// Does the box `exact` (absolute) meet trigger `id`'s shape: its absolute
/// box, then a round trigger's axis distance, a world copy's clip or its
/// brushes (`SV_EntityContact`)?
pub fn box_contacts(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    t: &Trigger,
    exact: ([f32; 3], [f32; 3]),
) -> bool {
    contacts(host, cx, id, t, exact, false)
}

/// [`box_contacts`] with `capsule` meeting the brushes as the capsule of
/// the box (`capsule_contacts_hulls`): CoD2's `SV_EntityContact` traces
/// every box as one (cod2-turrets.md 16).
pub fn contacts(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    t: &Trigger,
    exact: ([f32; 3], [f32; 3]),
    capsule: bool,
) -> bool {
    let lo = Vec3::from_array(exact.0);
    let hi = Vec3::from_array(exact.1);
    let centre = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    let origin_atom = cx.intern_folded("origin");
    let t_origin = entity_origin(host, cx, id, origin_atom);
    let b = abs_bounds(t_origin, t);
    if !boxes_overlap(exact, b) {
        return false;
    }
    if let Some(round) = t.round {
        return round.contacts(t_origin, exact.0, exact.1);
    }
    // A script-spawned trigger's inline model 0 is a copy of the
    // world at its origin: the box's position test against that
    // copy's clip, under `trap_EntityContact`'s -1 (combat doc 5.9).
    if t.shape.model == Some(0)
        && let Some(w) = host.world.as_ref()
    {
        let tr = w.collision.pusher_box_trace(
            vcod_common::collision::Pusher::Copy(id.0),
            centre,
            centre,
            -half,
            half,
            u32::MAX,
        );
        return tr.startsolid || tr.allsolid;
    }
    match t
        .shape
        .model
        .and_then(|m| host.model_brushes.get(m as usize))
    {
        Some(hulls) if !hulls.is_empty() => {
            let hull_test = if capsule {
                capsule_contacts_hulls
            } else {
                box_contacts_hulls
            };
            hull_test(centre, half, Vec3::from_array(t_origin), hulls)
        }
        _ => true,
    }
}

/// A trigger's links (combat doc 14.7, `game.mp.i386.so`), off its row: the
/// brush-model setter's with contents -1 (`cod_lnxded` 0x8089544), then the
/// spawn function's contents, with a link of their own except on
/// `trigger_hurt` (0x64f6c) and `trigger_use` (0x5745a), which write theirs
/// after the last link. A `trigger_multiple` or `trigger_once` with
/// `spawnflags & 8` takes no 0x40000000 (0x64d66), so with no other flag it
/// leaves the tree.
pub fn link(host: &mut GameHost, cx: &mut Cx, id: EntId, spawnflags: i32) {
    use crate::game::entity::{LinkKind, LinkShape};
    let Some(t) = host.triggers.get(id).copied() else {
        return;
    };
    let shape = |contents| LinkShape {
        kind: LinkKind::Brush,
        contents,
        mins: t.shape.mins,
        maxs: t.shape.maxs,
    };
    host.link_shaped(cx, id, shape(-1));
    // `spawnflags` 1, 2 and 4 OR 0x40000, 0x80000 and 0x100000 into the
    // contents (0x64d83..0x64dac, 0x65cce..0x65cf7), whichever base it has.
    let flagged = [(1, 0x4_0000), (2, 0x8_0000), (4, 0x10_0000)]
        .iter()
        .filter(|(f, _)| spawnflags & f != 0)
        .fold(0, |c, (_, bit)| c | bit);
    let (contents, links) = match t.kind {
        TriggerKind::Multiple | TriggerKind::Once if spawnflags & 8 != 0 => (flagged, true),
        TriggerKind::Multiple | TriggerKind::Once => (CONTENTS_TRIGGER | flagged, true),
        TriggerKind::Damage => (CONTENTS_TOUCH_MASK, true),
        TriggerKind::LookAt => (CONTENTS_LOOKAT, true),
        TriggerKind::Hurt => (CONTENTS_TOUCH_MASK, false),
        TriggerKind::Use => (CONTENTS_USE, false),
        TriggerKind::Mount { crouch } => (mount_contents(crouch, t.shape.model.is_some()), true),
    };
    if links {
        host.link_shaped(cx, id, shape(contents));
    } else {
        host.set_link_contents(id, contents);
    }
}

/// The boxes [`vcod_common::movetrace::MoveWorld`]'s point contents add on
/// the server: every linked trigger whose contents hold a mount bit, a
/// `trigger_mount` unless a `setContents` moved one, as its own box.
/// `SV_PointContents` tests a brush trigger's brushes, vcod its box; no
/// stock map places a brush `trigger_mount` (coduo-movement.md 11).
pub fn mount_boxes(host: &mut GameHost, cx: &mut Cx) -> Vec<vcod_common::movetrace::ContentsBox> {
    let hmg = vcod_common::pmove::CONTENTS_HMG;
    let origin_atom = cx.intern_folded("origin");
    let rows: Vec<(EntId, Trigger)> = host
        .triggers
        .iter()
        .filter(|(id, _)| host.area.contents(id.0) as u32 & hmg != 0)
        .map(|(id, t)| (id, *t))
        .collect();
    rows.into_iter()
        .map(|(id, t)| {
            let (lo, hi) = abs_bounds(entity_origin(host, cx, id, origin_atom), &t);
            vcod_common::movetrace::ContentsBox {
                mins: lo.into(),
                maxs: hi.into(),
                contents: host.area.contents(id.0) as u32,
            }
        })
        .collect()
}

/// `G_CheckForCursorHints`' LMG probe (game.mp.uo 0x67b2f): `point`, 15
/// ahead of the feet along the view's yaw and 1 up, meets mount contents
/// (`trap_PointContents` under 0x404000, the world's and `boxes`'), or
/// `G_CheckPointInsideTriggerMount` (0x8780f) finds a `trigger_mount` whose
/// linked box comes within 0.1 of it. `Some` of `serverCursorHintVal` for
/// hint 7: 0 off the contents, the trigger's spawnflag 1 off the second
/// test, which only a point in the link's one-unit margin reaches.
pub fn lmg_mount(
    host: &GameHost,
    boxes: &[vcod_common::movetrace::ContentsBox],
    point: Vec3,
) -> Option<i32> {
    let hmg = vcod_common::pmove::CONTENTS_HMG;
    let contents = host
        .world
        .as_ref()
        .map_or(0, |w| w.collision.point_contents(point))
        | vcod_common::movetrace::ContentsBox::contents_at(boxes, point);
    if contents & hmg != 0 {
        return Some(0);
    }
    let lo = (point - Vec3::splat(0.1)).to_array();
    let hi = (point + Vec3::splat(0.1)).to_array();
    host.area
        .entities_in_box(lo, hi, CONTENTS_TOUCH_MASK)
        .into_iter()
        .filter_map(|n| host.ents.handle(n))
        .find_map(|id| match host.triggers.get(id)?.kind {
            TriggerKind::Mount { crouch } => Some(i32::from(crouch)),
            _ => None,
        })
}

/// How far the aim ray reaches: muzzle plus forward times 8192
/// (docs/research/cod11-gsc-object-model.md 23.1).
pub const AIM_TRACE_RANGE: f32 = 8192.0;

/// The fraction along `start..end` at which the segment first enters any of
/// a model's hulls, the model sitting at `origin`; `None` when it enters
/// none. A start already inside a hull enters it at 0.
pub fn segment_enters_hulls(
    start: Vec3,
    end: Vec3,
    origin: Vec3,
    hulls: &[BrushHull],
) -> Option<f32> {
    let s = start - origin;
    let d = end - start;
    hulls
        .iter()
        .filter(|b| !b.planes.is_empty())
        .filter_map(|b| {
            let (mut t_enter, mut t_exit) = (0.0f32, 1.0f32);
            for &(n, dist) in &b.planes {
                let s_dist = n.dot(s) - dist;
                let rate = n.dot(d);
                if rate.abs() < 1e-6 {
                    if s_dist > 0.0 {
                        return None;
                    }
                    continue;
                }
                let t = -s_dist / rate;
                if rate > 0.0 {
                    t_exit = t_exit.min(t);
                } else {
                    t_enter = t_enter.max(t);
                }
                if t_enter > t_exit {
                    return None;
                }
            }
            Some(t_enter)
        })
        .min_by(|a, b| a.total_cmp(b))
}

/// The lookat trigger the client's aim ray enters first, if nothing
/// `G_CheckForPreventFriendlyFire`'s two traces meet stops it sooner: the
/// world, and any live body but the aimer's own (`pass`), posed at `now_ms`
/// and ranked through `riflePriorityMap` when `rifle`
/// (docs/research/cod11-gsc-object-model.md 23.1). `aim` is `[pitch, yaw]`
/// in wire degrees, the pair `ClientSim::aim_angles` returns. A lookat with
/// brushes is entered through them, one without through its box. The bodies
/// are `GameHost::client_bodies`.
///
/// Not modelled: script models (0x2080, which the second mask's 0x2000
/// meets). Also unmeasured: retail's traces end on whatever entity they hit
/// first, so an intervening non-lookat trigger brush may stop it on the
/// wrong classname; ours skips every other kind and reaches the lookat
/// behind it.
pub fn aim_trace(
    host: &mut GameHost,
    cx: &mut Cx,
    pass: usize,
    eye: [f32; 3],
    aim: [f32; 2],
    rifle: bool,
    now_ms: i32,
) -> Option<EntId> {
    // `CalcMuzzlePoints` truncates the muzzle point toward zero (23.1).
    let start = Vec3::from(eye).trunc();
    let (yaw, pitch) = crate::game::combat::aim_radians(aim);
    let forward = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );
    let end = start + forward * AIM_TRACE_RANGE;
    // The second trace's mask less the lookat bit and BODY, which no brush
    // carries; it holds every world bit the first one does.
    let world_f = host.world.as_ref().map_or(1.0, |w| {
        w.collision
            .point_trace(start, end, AIM_WORLD_MASK, true)
            .fraction
    });
    let rows: Vec<(EntId, Trigger)> = host
        .triggers
        .iter()
        .filter(|(_, t)| t.kind == TriggerKind::LookAt)
        .map(|(id, t)| (id, *t))
        .collect();
    let origin_atom = cx.intern_folded("origin");
    let mut best: Option<(f32, EntId)> = None;
    for (id, t) in rows {
        let origin = entity_origin(host, cx, id, origin_atom);
        let f = match t
            .shape
            .model
            .and_then(|m| host.model_brushes.get(m as usize))
        {
            Some(hulls) if !hulls.is_empty() => {
                segment_enters_hulls(start, end, Vec3::from(origin), hulls)
            }
            _ => {
                let (lo, hi) = abs_bounds(origin, &t);
                crate::game::combat::ray_box(start, end, lo.into(), hi.into())
            }
        };
        if let Some(f) = f
            && f < world_f
            && best.is_none_or(|(b, _)| f < b)
        {
            best = Some((f, id));
        }
    }
    let (lookat_f, id) = best?;
    let bodies: Vec<_> = host.client_bodies.iter().flatten().cloned().collect();
    let (fs, anims) = (host.fs.clone(), host.anims.clone());
    let mut bones = crate::game::combat::BoneTraceCtx::of(
        fs.as_deref(),
        anims.as_deref(),
        &mut host.hit_rigs,
        now_ms,
        &mut host.dobj_posed,
        vcod_common::patch::Arith::for_protocol(host.defs.proto),
    );
    let priority = if rifle {
        &crate::game::combat::RIFLE_PRIORITY
    } else {
        &crate::game::combat::BULLET_PRIORITY
    };
    let body = crate::game::combat::trace_bodies(
        start,
        end,
        pass,
        &bodies,
        lookat_f,
        priority,
        bones.as_mut(),
    );
    match body {
        Some((_, f, _)) if f < lookat_f => None,
        _ => Some(id),
    }
}

/// `G_CheckForPreventFriendlyFire`'s second mask, 0x22802001, with the
/// `trigger_lookat` bit (0x20000000) and BODY taken out: the world half of it.
const AIM_WORLD_MASK: u32 = 0x0080_2001;

/// `isTouching` (0x59eb8): the box of whichever side is no brush model
/// against the other's brushes through `trap_EntityContact`, or through
/// `trap_EntityContactCapsule` when that side is a client, a box overlap
/// when both are brush models (docs/research/cod11-gsc-object-model.md
/// 22.2). Only a trigger with decoded brushes counts as a brush model here.
pub fn entity_contact(host: &mut GameHost, cx: &mut Cx, a: EntId, b: EntId) -> bool {
    let origin_atom = cx.intern_folded("origin");
    let hulls_of = |host: &GameHost, id: EntId| {
        let m = host.triggers.get(id)?.shape.model?;
        Some(m as usize).filter(|&m| host.model_brushes.get(m).is_some_and(|h| !h.is_empty()))
    };
    let (ma, mb) = (hulls_of(host, a), hulls_of(host, b));
    let (boxed, brushes, model) = match (ma, mb) {
        (None, Some(m)) => (a, b, m),
        (Some(m), None) => (b, a, m),
        _ => {
            let ba = abs_bounds_with_atom(host, cx, a, origin_atom);
            let bb = abs_bounds_with_atom(host, cx, b, origin_atom);
            let is_client = |id: EntId| host.ents.get(id).is_some_and(|e| e.client.is_some());
            if ma.is_some() {
                return boxes_overlap(ba, bb);
            }
            // The argument's temp box or capsule model carries its
            // `r.contents`, which the contact's mask of -1 must meet: a
            // `script_origin` or a dead client is never touched (22.2). A
            // brush model's inline model has its brushes' own contents.
            let bmodel = host
                .ents
                .get(b)
                .is_some_and(|e| e.brush_model.is_some_and(|n| n > 0));
            if !bmodel && host.area.contents(b.0) == 0 {
                return false;
            }
            if is_client(a) && is_client(b) {
                return capsules_overlap(ba, bb);
            }
            return boxes_overlap(ba, bb);
        }
    };
    let (lo, hi) = abs_bounds_with_atom(host, cx, boxed, origin_atom);
    let (lo, hi) = (Vec3::from_array(lo), Vec3::from_array(hi));
    let at = Vec3::from_array(entity_origin(host, cx, brushes, origin_atom));
    // `SVF_CAPSULE` (0x200) is on every client and nothing else here.
    let capsule = host.ents.get(boxed).is_some_and(|e| e.client.is_some());
    let contact = if capsule {
        capsule_contacts_hulls
    } else {
        box_contacts_hulls
    };
    contact(
        (lo + hi) * 0.5,
        (hi - lo) * 0.5,
        at,
        &host.model_brushes[model],
    )
}

/// `CM_TestCapsuleInCapsule` (`cod_lnxded` 0x805475c): the capsules made of
/// two absolute boxes, each of radius `min(half x, half z)` with its sphere
/// centres `half z - radius` above and below the box centre, overlap when any
/// pair of sphere centres sits nearer than the two radii or the centres' z
/// gap is within both offsets and their xy gap nearer than the radii
/// (docs/research/cod11-gsc-object-model.md 22.2).
pub fn capsules_overlap(a: ([f32; 3], [f32; 3]), b: ([f32; 3], [f32; 3])) -> bool {
    let shape = |(lo, hi): ([f32; 3], [f32; 3])| {
        let (lo, hi) = (Vec3::from_array(lo), Vec3::from_array(hi));
        let c = (lo + hi) * 0.5;
        let half = hi - c;
        let r = half.x.min(half.z);
        (c, r, half.z - r)
    };
    let ((ca, ra, oa), (cb, rb, ob)) = (shape(a), shape(b));
    let reach = (ra + rb) * (ra + rb);
    let up = Vec3::Z;
    let spheres = [
        (cb + up * ob, ca + up * oa),
        (cb + up * ob, ca - up * oa),
        (cb - up * ob, ca + up * oa),
        (cb - up * ob, ca - up * oa),
    ];
    if spheres
        .iter()
        .any(|&(p, q)| (p - q).length_squared() < reach)
    {
        return true;
    }
    let dz = ca.z - cb.z;
    let span = ob + oa;
    let xy = (ca - cb).truncate();
    dz <= span && -span <= dz && xy.length_squared() < reach
}

/// Do two absolute boxes overlap, the test `trap_EntitiesInBox` performs.
pub fn boxes_overlap(a: ([f32; 3], [f32; 3]), b: ([f32; 3], [f32; 3])) -> bool {
    (0..3).all(|i| a.0[i] <= b.1[i] && a.1[i] >= b.0[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `G_SetPlayerSize` on the registered defaults, which are cheat
    /// protected on all three servers (cod11-gsc-object-model.md 16.2).
    #[test]
    fn the_player_box_is_g_set_player_size_on_the_cvar_defaults() {
        for proto in [1, 6, 22] {
            let cv = crate::cvars::Cvars::for_protocol(proto);
            let half = cv.get("g_bounds_width").parse::<f32>().unwrap() * 0.5;
            let height = cv.get("g_bounds_height_standing").parse::<f32>().unwrap();
            assert_eq!(PLAYER_MINS, [-half, -half, 0.0], "protocol {proto}");
            assert_eq!(PLAYER_MAXS, [half, half, height], "protocol {proto}");
            for name in ["g_bounds_width", "g_bounds_height_standing"] {
                let flags = cv.flags(name).unwrap();
                assert_ne!(flags & crate::cvars::flag::CHEAT, 0, "{name}");
            }
        }
    }

    /// The six trigger classnames in the `spawns` table map to kinds; nothing
    /// else does (docs/research/cod11-gsc-object-model.md section 8).
    #[test]
    fn the_spawns_tables_trigger_classnames_map_to_kinds() {
        assert_eq!(kind_of("trigger_multiple"), Some(TriggerKind::Multiple));
        assert_eq!(kind_of("trigger_once"), Some(TriggerKind::Once));
        assert_eq!(kind_of("trigger_use"), Some(TriggerKind::Use));
        assert_eq!(kind_of("trigger_lookat"), Some(TriggerKind::LookAt));
        assert_eq!(kind_of("trigger_hurt"), Some(TriggerKind::Hurt));
        assert_eq!(kind_of("trigger_damage"), Some(TriggerKind::Damage));
        assert_eq!(kind_of("script_brushmodel"), None);
        assert_eq!(kind_of("misc_mg42"), None);
    }

    /// Bounds are the submodel's, taken around the entity's *current* origin:
    /// `sd.gsc` relocates its defuse trigger with `bombtrigger.origin =
    /// level.bombmodel.origin`, so a cached absolute box would be wrong from
    /// `hurt_touch`'s cadence and once flag, read off the spawnflags at each
    /// touch (cod11-combat.md 5.9).
    #[test]
    fn a_hurt_rearms_off_its_spawnflags() {
        let mut t = Triggers::default();
        let id = EntId(80, 0);
        t.register_hurt(id, TriggerShape::boxed([0.0; 3], [0.0; 3]), 1, 0, 0.0);
        assert_eq!(t.arm_hurt(id, 0), 0);
        assert!(t.fire(id, 100, false).fired());
        assert!(!t.fire(id, 150, false).fired());
        assert!(t.fire(id, 200, false).fired());
        assert_eq!(
            t.arm_hurt(id, 0x10 | 0x8),
            crate::game::combat::DFLAG_NO_PROTECTION
        );
        assert!(t.fire(id, 300, false).fired());
        assert!(!t.fire(id, 1200, false).fired());
        t.arm_hurt(id, 0x20);
        assert!(t.fire(id, 1300, false).fired());
        assert!(!t.fire(id, 5000, false).fired(), "0x20 cleared the touch");
        assert_eq!(t.use_hurt(id, 5000), Some(None), "hurt_use sets it back");
        assert!(t.fire(id, 5000, false).fired());
    }

    /// `START_OFF` leaves the touch unset until `hurt_use` toggles it, and a
    /// `life` arms the free at the use (cod11-combat.md 5.9).
    #[test]
    fn a_start_off_hurt_waits_for_its_use() {
        let mut t = Triggers::default();
        let id = EntId(80, 0);
        t.register_hurt(id, TriggerShape::boxed([0.0; 3], [0.0; 3]), 1, 0x11, 2.5);
        t.arm_hurt(id, 0x11);
        assert!(!t.fire(id, 100, false).fired());
        assert_eq!(t.use_hurt(id, 1000), Some(Some(3500.0)));
        assert!(t.fire(id, 1000, false).fired());
        assert_eq!(t.use_hurt(id, 1500), Some(Some(4000.0)));
        assert!(!t.fire(id, 2000, false).fired());
        let other = EntId(81, 0);
        t.register(
            other,
            TriggerKind::Multiple,
            TriggerShape::boxed([0.0; 3], [0.0; 3]),
            0,
        );
        assert_eq!(t.use_hurt(other, 0), None);
    }

    /// the plant onward.
    #[test]
    fn abs_bounds_follow_the_origin() {
        let t = Trigger {
            kind: TriggerKind::Multiple,
            shape: TriggerShape::boxed([-16.0, -16.0, 0.0], [16.0, 16.0, 72.0]),
            wait_ms: 0,
            next_fire_ms: 0,
            spent: false,
            hurt_flags: 0,
            life: 0.0,
            sound: 0,
            cursor_hint: 0,
            hint_string: -1,
            spec: None,
            round: None,
            team: 0,
            claimer: None,
        };
        assert_eq!(
            abs_bounds([100.0, -50.0, 8.0], &t),
            ([84.0, -66.0, 8.0], [116.0, -34.0, 80.0])
        );
    }

    /// A registered trigger is found by id and a removed one is gone: the
    /// plant deletes both bombzones and a stale row would keep firing.
    #[test]
    fn register_and_remove() {
        let mut ts = Triggers::default();
        let id = EntId(72, 0);
        assert!(ts.is_empty());
        ts.register(
            id,
            TriggerKind::Hurt,
            TriggerShape::boxed([-8.0; 3], [8.0; 3]),
            0,
        );
        assert_eq!(ts.len(), 1);
        assert_eq!(ts.get(id).map(|t| t.kind), Some(TriggerKind::Hurt));
        ts.remove(id);
        assert!(ts.get(id).is_none());
        assert!(ts.is_empty());
    }

    /// A trigger has to clear both boxes: one 65 units up is inside the
    /// client's clip box and outside the candidate box, one 30 down is the
    /// other way round, and retail touches neither.
    #[test]
    fn a_touch_needs_both_the_candidate_box_and_the_clip_box() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let origin = cx.intern_folded("origin");
            let place = |host: &mut GameHost, cx: &mut Cx, z: f32| {
                let id = host.ents.spawn(cx).unwrap();
                host.set_field(cx, id, origin, Value::Vector([0.0, 0.0, z]))
                    .unwrap();
                host.triggers.register(
                    id,
                    TriggerKind::Multiple,
                    TriggerShape::boxed([-8.0; 3], [8.0; 3]),
                    crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
                );
                host.link_trigger(cx, id);
                id
            };
            let at_feet = place(&mut host, cx, 4.0);
            place(&mut host, cx, 65.0);
            place(&mut host, cx, -30.0);
            assert_eq!(touched(&mut host, cx, 0, [0.0; 3]), vec![at_feet]);
        });
    }

    /// The touch pass's clip box is the stance box the cmd's pmove left in
    /// `r.maxs` (gsc object model doc 22.3): a trigger whose bottom sits 40
    /// up is touched standing (70) and crouched (50), not prone (30).
    #[test]
    fn the_touch_pass_clip_box_follows_the_stance() {
        use vcod_common::pmove::Stance;
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let origin = cx.intern_folded("origin");
            let id = host.ents.spawn(cx).unwrap();
            host.set_field(cx, id, origin, Value::Vector([0.0, 0.0, 48.0]))
                .unwrap();
            host.triggers.register(
                id,
                TriggerKind::Multiple,
                TriggerShape::boxed([-8.0; 3], [8.0; 3]),
                crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
            );
            host.link_trigger(cx, id);
            for (stance, hit) in [
                (Stance::Stand, true),
                (Stance::Crouch, true),
                (Stance::Prone, false),
            ] {
                host.client_height[0] = stance.height();
                let want = if hit { vec![id] } else { vec![] };
                assert_eq!(touched(&mut host, cx, 0, [0.0; 3]), want, "{stance:?}");
            }
        });
    }

    /// A `trigger_lookat` sharing a box with a `trigger_multiple` is not
    /// touched: retail's broad-phase contents mask has its bit clear
    /// (docs/research/cod11-gsc-object-model.md 22.1).
    #[test]
    fn a_lookat_trigger_is_not_touched_where_a_multiple_is() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let origin = cx.intern_folded("origin");
            let place = |host: &mut GameHost, cx: &mut Cx, kind| {
                let id = host.ents.spawn(cx).unwrap();
                host.set_field(cx, id, origin, Value::Vector([0.0, 0.0, 4.0]))
                    .unwrap();
                host.triggers
                    .register(id, kind, TriggerShape::boxed([-8.0; 3], [8.0; 3]), 0);
                host.link_trigger(cx, id);
                id
            };
            let multiple = place(&mut host, cx, TriggerKind::Multiple);
            place(&mut host, cx, TriggerKind::LookAt);
            assert_eq!(touched(&mut host, cx, 0, [0.0; 3]), vec![multiple]);
        });
    }

    /// `Touch_Multi`'s `wait` never gates the notify: above 0 every touch
    /// fires, not above 0 (and on every `trigger_once`) the first touch fires
    /// and spends the trigger, and an `enableLinkTo` trigger is never spent
    /// (movers doc 17, probe_trigwait).
    #[test]
    fn wait_spends_a_trigger_and_never_gates_its_notify() {
        let mut ts = Triggers::default();
        let shape = TriggerShape::boxed([-8.0; 3], [8.0; 3]);
        let (w5, w0, once, linked) = (EntId(72, 0), EntId(73, 0), EntId(74, 0), EntId(75, 0));
        ts.register(w5, TriggerKind::Multiple, shape, 5000);
        ts.register(w0, TriggerKind::Multiple, shape, 0);
        ts.register(once, TriggerKind::Once, shape, ONCE_WAIT_MS);
        ts.register(linked, TriggerKind::Multiple, shape, -1000);
        for t in [0, 0, 50, 100, 150] {
            assert_eq!(ts.fire(w5, t, false), Fire::Fired, "w5 at {t}");
            assert_eq!(ts.fire(linked, t, true), Fire::Fired, "linked at {t}");
        }
        for id in [w0, once] {
            assert_eq!(ts.fire(id, 0, false), Fire::Spent);
            assert_eq!(ts.fire(id, 0, false), Fire::Refused);
            assert_eq!(ts.fire(id, 100_000, false), Fire::Refused);
        }
    }

    /// The two spellings of the hurt mod agree. `MOD_TRIGGER_HURT` is the
    /// string `deliver_world_hit` passes and `MOD_NAMES[23]` is what
    /// `mod_index` looks it up in; a drift between them makes every
    /// `trigger_hurt` death an unnamed mod.
    #[test]
    fn the_hurt_mod_name_matches_the_means_of_death_table() {
        assert_eq!(crate::game::combat::MOD_NAMES[23], MOD_TRIGGER_HURT);
    }

    /// A trigger's own box for the test to register, with its model's
    /// brushes under it.
    fn place_wedge(host: &mut GameHost, cx: &mut Cx) -> EntId {
        // A 256 x 256 x 128 brush cut by `x + y <= 256`: the half of its
        // bounding box past that diagonal holds no brush at all, which is the
        // shape of mp_pavlov's minefield wedges and where the reported death
        // happened.
        let diagonal = Vec3::new(1.0, 1.0, 0.0).normalize();
        host.model_bounds = vec![
            ([0.0; 3], [0.0; 3]),
            ([0.0, 0.0, 0.0], [256.0, 256.0, 128.0]),
        ];
        host.model_brushes = vec![
            Vec::new(),
            vec![BrushHull {
                planes: vec![
                    (-Vec3::X, 0.0),
                    (Vec3::X, 256.0),
                    (-Vec3::Y, 0.0),
                    (Vec3::Y, 256.0),
                    (-Vec3::Z, 0.0),
                    (Vec3::Z, 128.0),
                    (diagonal, 256.0 / 2.0_f32.sqrt()),
                ],
            }],
        ];
        let origin = cx.intern_folded("origin");
        let zone = host.ents.spawn(cx).unwrap();
        host.set_field(cx, zone, origin, Value::Vector([0.0; 3]))
            .unwrap();
        host.triggers.register(
            zone,
            TriggerKind::Multiple,
            TriggerShape {
                mins: [0.0, 0.0, 0.0],
                maxs: [256.0, 256.0, 128.0],
                model: Some(1),
            },
            crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
        );
        host.link_trigger(cx, zone);
        zone
    }

    /// The exact stage is a brush contact, not a bounds overlap: a player
    /// standing in the bulge of a wedge's bounding box touches nothing, and
    /// one inside the brush touches it. That difference is a minefield
    /// killing a player meters clear of the mines
    /// (docs/research/cod11-gsc-object-model.md section 22).
    #[test]
    fn a_wedge_is_touched_by_its_brush_and_not_by_the_bulge_of_its_box() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let zone = place_wedge(&mut host, cx);
            let at = |host: &mut GameHost, cx: &mut Cx, p: [f32; 3]| touched(host, cx, 0, p);
            assert_eq!(
                at(&mut host, cx, [60.0, 60.0, 4.0]),
                vec![zone],
                "inside the brush"
            );
            assert!(
                at(&mut host, cx, [200.0, 200.0, 4.0]).is_empty(),
                "inside the bounding box, 70-odd units clear of the brush"
            );
        });
    }

    /// The same asymmetry against real geometry: one of mp_pavlov's shaped
    /// `minefield` submodels, whose bounding box reaches well past its
    /// brushes. Needs `COD_DIR`; without the paks it returns early.
    #[test]
    fn a_shaped_minefield_model_on_mp_pavlov_leaves_its_bounding_box_hollow() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes = fs
            .read("maps/mp/mp_pavlov.bsp")
            .expect("mp_pavlov in the paks");
        let bsp = vcod_common::bsp::parse(&bytes).expect("parse mp_pavlov");
        let hulls = model_brush_hulls(&bsp);

        // `*25` is one of the 19 minefield brushes carrying angled planes.
        const MODEL: usize = 25;
        assert!(
            hulls[MODEL].iter().any(|b| b.planes.len() > 6),
            "model *{MODEL} is a plain box; pick another shaped one"
        );
        let (mins, maxs) = (bsp.models[MODEL].mins, bsp.models[MODEL].maxs);

        // Two stations sampled over the model's own box at its floor: one
        // whose clip box meets a brush and one whose clip box meets only the
        // box. The second existing at all is the bug.
        let half = Vec3::new(15.0, 15.0, 35.0);
        let mut inside = None;
        let mut bulge = None;
        for ix in 0..=32 {
            for iy in 0..=32 {
                let p = [
                    mins[0] + (maxs[0] - mins[0]) * ix as f32 / 32.0,
                    mins[1] + (maxs[1] - mins[1]) * iy as f32 / 32.0,
                    mins[2],
                ];
                let centre = Vec3::new(p[0], p[1], p[2] + 35.0);
                let clip = offset_bounds(p, PLAYER_MINS, PLAYER_MAXS);
                if !boxes_overlap(clip, (mins, maxs)) {
                    continue;
                }
                if box_contacts_hulls(centre, half, Vec3::ZERO, &hulls[MODEL]) {
                    inside.get_or_insert(p);
                } else {
                    bulge.get_or_insert(p);
                }
            }
        }
        let inside = inside.expect("a station inside the minefield's brushes");
        let bulge = bulge.expect("a station in its bounding box but in none of its brushes");

        let (mut vm, mut host) = crate::game::testing::fixture();
        host.model_bounds = bsp.models.iter().map(|m| m.clip_bounds()).collect();
        host.model_brushes = hulls;
        vm.with_cx(|cx| {
            let origin = cx.intern_folded("origin");
            let zone = host.ents.spawn(cx).unwrap();
            host.set_field(cx, zone, origin, Value::Vector([0.0; 3]))
                .unwrap();
            host.triggers.register(
                zone,
                TriggerKind::Hurt,
                TriggerShape {
                    mins,
                    maxs,
                    model: Some(MODEL as u32),
                },
                0,
            );
            host.link_trigger(cx, zone);
            let at = |host: &mut GameHost, cx: &mut Cx, p: [f32; 3]| touched(host, cx, 0, p);
            assert_eq!(at(&mut host, cx, inside), vec![zone], "at {inside:?}");
            assert!(
                at(&mut host, cx, bulge).is_empty(),
                "at {bulge:?}, which is in the box and in no brush"
            );
        });
    }

    /// A segment through a hull enters it at the near face, and one that
    /// passes beside it enters nothing.
    #[test]
    fn a_lookat_hull_on_the_ray_is_found_and_a_box_beside_it_is_not() {
        let hull = BrushHull {
            planes: vec![
                (Vec3::X, 16.0),
                (-Vec3::X, 16.0),
                (Vec3::Y, 16.0),
                (-Vec3::Y, 16.0),
                (Vec3::Z, 16.0),
                (-Vec3::Z, 16.0),
            ],
        };
        let o = Vec3::new(200.0, 0.0, 0.0);
        let f = segment_enters_hulls(
            Vec3::ZERO,
            Vec3::new(400.0, 0.0, 0.0),
            o,
            std::slice::from_ref(&hull),
        )
        .unwrap();
        assert!((f - 184.0 / 400.0).abs() < 1e-4);
        assert!(
            segment_enters_hulls(
                Vec3::ZERO,
                Vec3::new(400.0, 40.0, 0.0),
                o,
                std::slice::from_ref(&hull)
            )
            .is_none()
        );
        // A start inside the hull enters it at 0.
        let inside = segment_enters_hulls(
            Vec3::new(200.0, 0.0, 0.0),
            Vec3::new(400.0, 0.0, 0.0),
            o,
            std::slice::from_ref(&hull),
        );
        assert_eq!(inside, Some(0.0));
        // A hull with no planes is nothing, not everything.
        let empty = BrushHull { planes: Vec::new() };
        assert!(
            segment_enters_hulls(
                Vec3::ZERO,
                Vec3::new(400.0, 40.0, 0.0),
                o,
                std::slice::from_ref(&empty)
            )
            .is_none()
        );
    }

    /// The aim trace answers the nearest `trigger_lookat` on the ray and
    /// nothing else: a `trigger_multiple` in front of it is not a lookat,
    /// and a lookat further down the ray loses to the nearer one
    /// (docs/research/cod11-gsc-object-model.md 23.1).
    #[test]
    fn the_aim_trace_returns_the_nearest_lookat_and_skips_other_kinds() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let origin = cx.intern_folded("origin");
            let place_at = |host: &mut GameHost, cx: &mut Cx, kind, at: [f32; 3]| {
                let id = host.ents.spawn(cx).unwrap();
                host.set_field(cx, id, origin, Value::Vector(at)).unwrap();
                host.triggers.register(
                    id,
                    kind,
                    TriggerShape::boxed([-20.0, -20.0, 0.0], [20.0, 20.0, 80.0]),
                    0,
                );
                host.link_trigger(cx, id);
                id
            };
            let near = place_at(&mut host, cx, TriggerKind::Multiple, [100.0, 0.0, 0.0]);
            let far = place_at(&mut host, cx, TriggerKind::LookAt, [300.0, 0.0, 0.0]);
            let farther = place_at(&mut host, cx, TriggerKind::LookAt, [500.0, 0.0, 0.0]);
            let hit = aim_trace(&mut host, cx, 0, [0.0, 0.0, 60.0], [0.0, 0.0], false, 0);
            assert_eq!(hit, Some(far));
            let miss = aim_trace(&mut host, cx, 0, [0.0, 0.0, 60.0], [0.0, 90.0], false, 0);
            assert_eq!(miss, None);
            let _ = (near, farther);
        });
    }

    /// A world brush between the eye and a lookat stops the aim trace short
    /// of it; the same trace with the brush gone reaches it.
    #[test]
    fn a_world_brush_between_eye_and_lookat_blocks_the_aim_trace() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let world = |extra: &[(Vec3, Vec3)]| {
            Some(std::rc::Rc::new(crate::world::World {
                collision: vcod_common::collision::test_world(extra),
                vis: vcod_common::mapworld::Visibility::none(),
                spawn: ([0.0; 3], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
                rubble: Vec::new(),
                nav_cover: Vec::new(),
            }))
        };
        vm.with_cx(|cx| {
            let origin = cx.intern_folded("origin");
            let id = host.ents.spawn(cx).unwrap();
            host.set_field(cx, id, origin, Value::Vector([300.0, 0.0, 0.0]))
                .unwrap();
            host.triggers.register(
                id,
                TriggerKind::LookAt,
                TriggerShape::boxed([-20.0, -20.0, 0.0], [20.0, 20.0, 80.0]),
                0,
            );
            host.link_trigger(cx, id);
            host.world = world(&[]);
            assert_eq!(
                aim_trace(&mut host, cx, 0, [0.0, 0.0, 60.0], [0.0, 0.0], false, 0),
                Some(id)
            );
            host.world = world(&[(Vec3::new(150.0, -64.0, 0.0), Vec3::new(160.0, 64.0, 128.0))]);
            assert_eq!(
                aim_trace(&mut host, cx, 0, [0.0, 0.0, 60.0], [0.0, 0.0], false, 0),
                None
            );
        });
    }

    /// A lookat with brushes under it is entered through the brushes, not
    /// its box: the wedge's bulge is hollow to the aim the way it is to a
    /// touch.
    #[test]
    fn the_aim_trace_enters_a_lookat_through_its_brushes() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let zone = place_wedge(&mut host, cx);
            if let Some(t) = host.triggers.rows.get_mut(&zone) {
                t.kind = TriggerKind::LookAt;
            }
            // Straight down into the brush half, and straight down into the
            // bulge past the diagonal.
            assert_eq!(
                aim_trace(&mut host, cx, 0, [60.0, 60.0, 200.0], [90.0, 0.0], false, 0),
                Some(zone)
            );
            assert_eq!(
                aim_trace(
                    &mut host,
                    cx,
                    0,
                    [200.0, 200.0, 200.0],
                    [90.0, 0.0],
                    false,
                    0
                ),
                None
            );
        });
    }

    /// What each spawn leaves in the area tree (combat doc 14.7): a
    /// `trigger_hurt` keeps the place its contents -1 link gave it and is
    /// walked under its own contents, a `trigger_use` is out of the touch
    /// mask and in the use key's, a taken item leaves, an origin write moves an entity, and a
    /// free unlinks it.
    #[test]
    fn spawns_and_origin_writes_link_as_retail_does() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let origin = cx.intern_folded("origin");
            let place = |host: &mut GameHost, cx: &mut Cx, kind| {
                let id = host.ents.spawn(cx).unwrap();
                host.set_field(cx, id, origin, Value::Vector([0.0; 3]))
                    .unwrap();
                host.triggers
                    .register(id, kind, TriggerShape::boxed([-8.0; 3], [8.0; 3]), 0);
                host.link_trigger(cx, id);
                id
            };
            let hurt = place(&mut host, cx, TriggerKind::Hurt);
            let use_ = place(&mut host, cx, TriggerKind::Use);
            let item = host.ents.spawn(cx).unwrap();
            host.set_field(cx, item, origin, Value::Vector([0.0; 3]))
                .unwrap();
            crate::game::item::attach(&mut host, item, 1);
            crate::game::item::link(&mut host, cx, item, crate::game::item::CONTENTS_ITEM);
            let walk = |host: &GameHost, at: f32, mask| -> Vec<u32> {
                host.area
                    .entities_in_box([at - 50.0; 3], [at + 50.0; 3], mask)
            };
            assert_eq!(walk(&host, 0.0, CONTENTS_TOUCH_MASK), [item.0, hurt.0]);
            // An item's 0x407c0108 carries the use bit too: `G_GetActivateEnt`
            // finds both.
            assert_eq!(walk(&host, 0.0, CONTENTS_USE), [item.0, use_.0]);

            host.set_field(cx, item, origin, Value::Vector([500.0; 3]))
                .unwrap();
            assert_eq!(walk(&host, 0.0, CONTENTS_TOUCH_MASK), [hurt.0]);
            assert_eq!(walk(&host, 500.0, CONTENTS_TOUCH_MASK), [item.0]);
            crate::game::item::link(&mut host, cx, item, 0);
            assert!(walk(&host, 500.0, -1).is_empty(), "a taken item leaves");

            host.free_entity(hurt);
            assert!(walk(&host, 0.0, CONTENTS_TOUCH_MASK).is_empty());
        });
    }

    /// `delete()` takes the row with the entity. `sd.gsc` deletes both
    /// bombzones the instant a plant completes.
    #[test]
    fn freeing_an_entity_drops_its_trigger_row() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            host.triggers.register(
                id,
                TriggerKind::Multiple,
                TriggerShape::boxed([-8.0; 3], [8.0; 3]),
                crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
            );
            host.link_trigger(cx, id);
            host.free_entity(id);
            assert!(host.triggers.get(id).is_none());
            assert!(host.ents.get(id).is_none());
        });
    }
}
