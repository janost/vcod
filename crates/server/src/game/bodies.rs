//! The body queue: the eight corpse entities a gametype's `cloneplayer`
//! fills. `docs/research/cod11-combat.md` section 5.2 is the source for the
//! size, the entity numbers, the fields copied off the dying player and the
//! `eFlags` toggle; 5.3 for the settle and what the client does with a body.

use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{Protocol, ENTITYNUM_NONE, ENTITYNUM_WORLD};
use vcod_common::net::trajectory::{TR_GRAVITY, TR_STATIONARY};

/// `ET_CORPSE`. The client resolves the body model through `clientNum` on
/// the roster (`docs/research/clientstate-wire-format.md`).
pub const ET_CORPSE: i32 = 2;

/// `G_SpawnPlayerClone` indexes `&g_entities[64 + level->bodyQueIndex]` and
/// advances the index `(i + 1) & 7`, so the queue is eight entities starting
/// immediately past the 64 client slots (section 5.2).
pub const BODY_FIRST: u32 = 64;
pub const BODY_QUEUE_SIZE: usize = 8;

/// The `eFlags` bit `G_SpawnPlayerClone` inverts on every push so the word
/// differs from whatever the slot's previous occupant sent, which is what
/// makes the client restart the animation. The same bit a player's respawn
/// flips (`spectate::EF_TELEPORT_BIT`).
const EFLAGS_ANIM_TOGGLE: i32 = 8;

/// The `eFlags` bit `cloneplayer` also sets, cleared by the think it arms at
/// `level.time + 250` (`.so` 0x456DC, whose whole body is
/// `s.eFlags &= ~0x800`). The retail client reads it as "this body was just
/// cloned off a live player": set, the death animation plays from the top;
/// clear, it is snapped to its last frame and held (sections 5.2 and 5.3).
const EFLAGS_CORPSE_FRESH: i32 = 0x800;
const CORPSE_FRESH_MS: i32 = 250;

/// The bounds the settle traces with. `player_die` flattens the dying
/// entity's `maxs[2]` to 30 before the clone copies the box (section 5.1
/// item 13); the width is the player's own.
const CORPSE_HEIGHT: f32 = 30.0;

/// How far the settle looks for ground below the body. Retail's own
/// `G_BounceItem` re-trace distance (0x4E916). A body born higher than this
/// above the floor keeps its birth height rather than arcing down: there is
/// no per-frame item physics here, only the one contact.
const SETTLE_DROP: f32 = 128.0;

/// The lift `G_BounceItem` adds to the contact point before it settles
/// (0x4EA02, a random value in (0, 0.5]). Taken as its midpoint: nothing
/// reads the value back, and a constant keeps a body's origin reproducible.
const SETTLE_LIFT: f32 = 0.25;

pub struct Body {
    pub state: EntityState,
    pub born_ms: i32,
    /// The client slot the body was cloned from, taken by the one
    /// `refresh_newborn` the frame it was born.
    source: Option<usize>,
}

/// Nothing frees a clone on a timer: it lives until the eighth subsequent
/// death reuses its slot (section 5.2). So there is no expiry here, only
/// reuse.
pub struct BodyQueue {
    slots: Vec<Option<Body>>,
    /// Per slot, the state of the `eFlags` toggle bit the last push left.
    /// It survives the slot being reused, the way the retail entity's own
    /// `s.eFlags` does.
    toggles: Vec<bool>,
    next: usize,
}

impl BodyQueue {
    pub fn new(size: usize) -> Self {
        BodyQueue {
            slots: (0..size).map(|_| None).collect(),
            toggles: vec![false; size],
            next: 0,
        }
    }

    /// Clones the dying player's entity state into the next slot and returns
    /// the body's entity number. `source` is the client slot the server
    /// refreshes the body from once, at the build of the frame it was born,
    /// so a death animation the script raised after this call still lands on
    /// the corpse.
    pub fn push(
        &mut self,
        state: EntityState,
        source: Option<usize>,
        now_ms: i32,
        collision: Option<&CollisionWorld>,
        p: &Protocol,
    ) -> u32 {
        let i = self.next % self.slots.len();
        self.next += 1;
        self.toggles[i] = !self.toggles[i];
        let number = BODY_FIRST + i as u32;
        self.slots[i] = Some(Body {
            state: clone_of(&state, number, self.toggles[i], now_ms, collision, p),
            born_ms: now_ms,
            source,
        });
        number
    }

    /// Re-reads every body born this frame from its source client's sim.
    pub fn refresh_newborn(
        &mut self,
        now_ms: i32,
        mut from_sim: impl FnMut(usize) -> Option<EntityState>,
        collision: Option<&CollisionWorld>,
        p: &Protocol,
    ) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(body) = slot else { continue };
            if body.born_ms != now_ms {
                continue;
            }
            if let Some(fresh) = body.source.take().and_then(&mut from_sim) {
                body.state = clone_of(
                    &fresh,
                    BODY_FIRST + i as u32,
                    self.toggles[i],
                    now_ms,
                    collision,
                    p,
                );
            }
        }
    }

    /// The think `cloneplayer` arms: 250 ms after a body is born its
    /// `eFlags` bit 0x800 goes out (`.so` 0x456DC). `ScriptRuntime::run_frame`
    /// runs it beside the object table's thinks, which is where `G_RunFrame`
    /// runs the clone's.
    pub fn run_thinks(&mut self, now_ms: i32, p: &Protocol) {
        let Some(index) = EntityState::field_index(p, "eFlags") else {
            return;
        };
        for body in self.slots.iter_mut().flatten() {
            if now_ms.wrapping_sub(body.born_ms) >= CORPSE_FRESH_MS {
                body.state.fields[index] &= !EFLAGS_CORPSE_FRESH;
            }
        }
    }

    /// The live bodies, by entity number. No `Protocol`: a body's fields are
    /// resolved once, at the push.
    pub fn entities(&self) -> impl Iterator<Item = (u32, EntityState)> + '_ {
        self.slots
            .iter()
            .flatten()
            .map(|b| (b.state.number, b.state.clone()))
    }
}

/// The corpse `cloneplayer` builds out of the dying player's entity state:
/// the fields section 5.2's table names, written onto an empty state rather
/// than onto a copy of the player's, which is what the retail function does.
/// `toggle` is the slot's inverted `eFlags` anim bit.
///
/// Retail's clone is launched under gravity and settled by the next frame's
/// `G_RunItem` / `G_BounceItem`; `settle` does both halves here, so the birth
/// values below are what a retail body carries for a frame and the settled
/// ones are what reaches the wire.
fn clone_of(
    state: &EntityState,
    number: u32,
    toggle: bool,
    now_ms: i32,
    collision: Option<&CollisionWorld>,
    p: &Protocol,
) -> EntityState {
    let mut e = EntityState::null(p);
    e.number = number;
    let get = |name: &str| state.field_i32(p, name);
    let mut set = |name: &str, v: i32| {
        if let Some(i) = EntityState::field_index(p, name) {
            e.fields[i] = v;
        }
    };
    set("eType", ET_CORPSE);
    set("clientNum", get("clientNum"));
    set("legsAnim", get("legsAnim"));
    set("torsoAnim", get("torsoAnim"));
    set(
        "eFlags",
        (get("eFlags") & !EFLAGS_ANIM_TOGGLE)
            | if toggle { EFLAGS_ANIM_TOGGLE } else { 0 }
            | EFLAGS_CORPSE_FRESH,
    );
    set("groundEntityNum", ENTITYNUM_NONE as i32);
    set("pos.trType", TR_GRAVITY);
    set("pos.trTime", now_ms);
    for axis in 0..3 {
        for field in ["pos.trBase", "pos.trDelta", "apos.trBase"] {
            let name = format!("{field}[{axis}]");
            set(&name, get(&name));
        }
    }
    settle(&mut e, collision, p);
    e
}

/// The landing `G_RunItem` (0x4EB18) and `G_BounceItem` (0x4E858) give a
/// clone on its first frame: one downward trace, and on contact
/// `G_SetOrigin(ent, trace.endpos)` -- `pos.trType` 0, `pos.trDelta` 0,
/// `pos.trTime` 0, `pos.trBase` at the endpoint -- plus
/// `s.groundEntityNum = trace.entityNum`. A clone's `physicsBounce` is 0, so
/// the reflected velocity is multiplied away and the first contact settles
/// it; no bounce loop is needed.
///
/// The trajectory rewrite happens whether or not a trace found ground: a
/// corpse left under `TR_GRAVITY` sinks by `400 * age^2` on a retail client,
/// which clips a body against nothing (section 5.3). With no
/// collision world -- every unit test, and any host built without a map --
/// the body settles where it fell.
fn settle(e: &mut EntityState, collision: Option<&CollisionWorld>, p: &Protocol) {
    set_field(e, p, "pos.trType", TR_STATIONARY);
    set_field(e, p, "pos.trTime", 0);
    set_field(e, p, "groundEntityNum", ENTITYNUM_WORLD as i32);
    for axis in 0..3 {
        set_field(e, p, &format!("pos.trDelta[{axis}]"), 0);
    }
    let Some(collision) = collision else { return };
    // `pos.trBase` is the player's origin, at the feet, so the trace box
    // stands on the start point.
    let start = Vec3::from(e.origin(p));
    let half = vcod_common::pmove::HALF_WIDTH;
    let trace = collision.box_trace(
        start,
        start - Vec3::Z * SETTLE_DROP,
        Vec3::new(-half, -half, 0.0),
        Vec3::new(half, half, CORPSE_HEIGHT),
    );
    // Retail's startsolid arm re-traces; a body already inside geometry has
    // nowhere better to go, so it keeps where it fell.
    if trace.startsolid || trace.allsolid || trace.fraction >= 1.0 {
        return;
    }
    let endpos = trace.endpos + Vec3::Z * SETTLE_LIFT;
    for (axis, v) in endpos.to_array().iter().enumerate() {
        set_field(e, p, &format!("pos.trBase[{axis}]"), v.to_bits() as i32);
    }
}

fn set_field(e: &mut EntityState, p: &Protocol, name: &str, v: i32) {
    if let Some(i) = EntityState::field_index(p, name) {
        e.fields[i] = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::PROTOCOL_V1;

    fn dying(p: &Protocol, slot: u32) -> EntityState {
        let mut e = EntityState::null(p);
        e.number = slot;
        let set = |e: &mut EntityState, n: &str, v: i32| {
            e.fields[EntityState::field_index(p, n).unwrap()] = v;
        };
        set(&mut e, "eType", 1);
        set(&mut e, "clientNum", slot as i32);
        set(&mut e, "legsAnim", 700);
        set(&mut e, "torsoAnim", 512);
        set(&mut e, "eFlags", 16);
        set(&mut e, "pos.trBase[0]", 32.0f32.to_bits() as i32);
        e
    }

    #[test]
    fn a_body_keeps_the_dead_players_anims_client_num_and_place() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(2);
        let n = q.push(dying(p, 4), None, 1000, None, p);
        let (num, body) = q.entities().next().unwrap();
        assert_eq!(num, n);
        assert_eq!(body.number, n);
        assert_eq!(body.field_i32(p, "eType"), ET_CORPSE);
        assert_eq!(body.field_i32(p, "clientNum"), 4);
        assert_eq!(body.field_i32(p, "legsAnim"), 700);
        assert_eq!(body.field_i32(p, "torsoAnim"), 512);
        assert_eq!(body.origin(p)[0], 32.0);
    }

    /// Nothing frees a body on a timer; only the ninth death does. The dm
    /// hit fixture's first corpse is still on the wire 190 s after it died
    /// (`crates/server/tests/fixtures/playerstate/mp_carentan-dm-hit-target.txt`).
    #[test]
    fn a_body_outlives_any_timer_and_only_reuse_frees_it() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(2);
        q.push(dying(p, 4), None, 1000, None, p);
        assert_eq!(q.entities().count(), 1);
        q.push(dying(p, 5), None, 1000 + 300_000, None, p);
        assert_eq!(q.entities().count(), 2, "a body has no lifetime");
    }

    #[test]
    fn a_third_body_reuses_the_oldest_slot() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(2);
        let first = q.push(dying(p, 1), None, 0, None, p);
        q.push(dying(p, 2), None, 100, None, p);
        let third = q.push(dying(p, 3), None, 200, None, p);
        assert_eq!(third, first, "the third push reuses the first slot");
        assert_eq!(q.entities().count(), 2);
        let states: Vec<_> = q.entities().collect();
        let reused = states.iter().find(|(n, _)| *n == first).unwrap();
        assert_eq!(reused.1.field_i32(p, "clientNum"), 3);
    }

    /// The slot's `eFlags` anim-restart bit is inverted on every push, so
    /// two occupants of one slot never send the same word (section 5.2).
    #[test]
    fn a_slots_anim_toggle_flips_on_every_push() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(1);
        q.push(dying(p, 1), None, 0, None, p);
        let first = q.entities().next().unwrap().1.field_i32(p, "eFlags");
        q.push(dying(p, 2), None, 100, None, p);
        let second = q.entities().next().unwrap().1.field_i32(p, "eFlags");
        assert_ne!(first & EFLAGS_ANIM_TOGGLE, second & EFLAGS_ANIM_TOGGLE);
        // Everything but that bit is the player's own eFlags, plus the
        // fresh-corpse marker every push sets.
        let rest = !(EFLAGS_ANIM_TOGGLE | EFLAGS_CORPSE_FRESH);
        assert_eq!(first & rest, 16);
        assert_eq!(second & rest, 16);
        assert_eq!(first & EFLAGS_CORPSE_FRESH, EFLAGS_CORPSE_FRESH);
    }

    /// A body born this frame is re-read from its source client's sim once,
    /// so a death animation raised after the clone still reaches the wire.
    /// A body born earlier is left alone.
    #[test]
    fn refresh_newborn_takes_the_sims_anims_once() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(2);
        q.push(dying(p, 4), Some(4), 1000, None, p);
        let mut fresh = dying(p, 4);
        fresh.fields[EntityState::field_index(p, "legsAnim").unwrap()] = 18;
        let number = q.entities().next().unwrap().0;

        q.refresh_newborn(1000, |slot| (slot == 4).then(|| fresh.clone()), None, p);
        let (n, body) = q.entities().next().unwrap();
        assert_eq!(n, number, "the refresh keeps the body's own number");
        assert_eq!(body.number, number);
        assert_eq!(body.field_i32(p, "legsAnim"), 18);
        assert_eq!(body.field_i32(p, "eType"), ET_CORPSE);

        // The source is spent: a later frame does not re-read it.
        fresh.fields[EntityState::field_index(p, "legsAnim").unwrap()] = 999;
        q.refresh_newborn(1000, |_| Some(fresh.clone()), None, p);
        assert_eq!(q.entities().next().unwrap().1.field_i32(p, "legsAnim"), 18);
    }

    /// The queue is retail's own eight slots at 64..71, the numbers the
    /// retail hit fixtures show corpses arriving on.
    #[test]
    fn the_queue_is_eight_slots_starting_at_sixty_four() {
        let p = &PROTOCOL_V1;
        assert_eq!((BODY_FIRST, BODY_QUEUE_SIZE), (64, 8));
        let mut q = BodyQueue::new(BODY_QUEUE_SIZE);
        let numbers: Vec<u32> = (0..4)
            .map(|i| q.push(dying(p, i), None, 0, None, p))
            .collect();
        assert_eq!(numbers, vec![64, 65, 66, 67]);
    }

    /// A body leaves the queue settled, the way `G_BounceItem`'s first
    /// contact leaves retail's: `trType` 0, no delta, `trTime` 0 and the
    /// world as its ground entity. A retail client evaluates the trajectory
    /// forward and clips it against nothing, so a body still under
    /// `TR_GRAVITY` sinks by `400 * age^2` for as long as it lives.
    #[test]
    fn a_body_reaches_the_wire_settled() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(1);
        q.push(dying(p, 0), None, 5000, None, p);
        let body = q.entities().next().unwrap().1;
        assert_eq!(body.field_i32(p, "groundEntityNum"), ENTITYNUM_WORLD as i32);
        assert_eq!(body.field_i32(p, "pos.trType"), TR_STATIONARY);
        assert_eq!(body.field_i32(p, "pos.trTime"), 0);
        assert_eq!(body.field_i32(p, "pos.trDuration"), 0);
        for axis in 0..3 {
            assert_eq!(body.field_f32(p, &format!("pos.trDelta[{axis}]")), 0.0);
        }
    }

    /// The settle traces down and takes the contact point, plus the sub-unit
    /// lift `G_BounceItem` adds. The test world's floor is a plane at z = 0.
    #[test]
    fn a_body_born_above_the_floor_lands_on_it() {
        let p = &PROTOCOL_V1;
        let world = vcod_common::collision::test_world(&[]);
        let mut q = BodyQueue::new(1);
        let mut fell = dying(p, 0);
        fell.fields[EntityState::field_index(p, "pos.trBase[2]").unwrap()] =
            40.0f32.to_bits() as i32;
        q.push(fell, None, 5000, Some(&world), p);
        let z = q.entities().next().unwrap().1.origin(p)[2];
        assert!(
            (z - SETTLE_LIFT).abs() < 0.5,
            "a body dropped from z=40 rested at {z}"
        );
    }

    /// A body already standing on the floor keeps its height: the trace is a
    /// no-op settle, not a 128-unit fall.
    #[test]
    fn a_body_on_the_floor_stays_where_it_fell() {
        let p = &PROTOCOL_V1;
        let world = vcod_common::collision::test_world(&[]);
        let mut q = BodyQueue::new(1);
        let mut standing = dying(p, 0);
        standing.fields[EntityState::field_index(p, "pos.trBase[2]").unwrap()] =
            0.125f32.to_bits() as i32;
        q.push(standing, None, 5000, Some(&world), p);
        let z = q.entities().next().unwrap().1.origin(p)[2];
        assert!(
            (-0.1..1.0).contains(&z),
            "a body on the floor rested at {z}"
        );
    }

    /// The `0x800` marker rides for 250 ms and then the think takes it off.
    /// A retail client plays the death animation from the top while it is
    /// set and snaps to the clip's last frame once it is clear (section 5.2).
    #[test]
    fn the_fresh_corpse_bit_lasts_two_hundred_and_fifty_ms() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(1);
        q.push(dying(p, 0), None, 5000, None, p);
        let flags = |q: &BodyQueue| q.entities().next().unwrap().1.field_i32(p, "eFlags");
        assert_eq!(flags(&q) & EFLAGS_CORPSE_FRESH, EFLAGS_CORPSE_FRESH);
        q.run_thinks(5200, p);
        assert_eq!(
            flags(&q) & EFLAGS_CORPSE_FRESH,
            EFLAGS_CORPSE_FRESH,
            "the marker is cleared 250 ms in, not before"
        );
        q.run_thinks(5250, p);
        assert_eq!(flags(&q) & EFLAGS_CORPSE_FRESH, 0);
        // The slot's own anim toggle is untouched by the think.
        assert_eq!(flags(&q) & EFLAGS_ANIM_TOGGLE, EFLAGS_ANIM_TOGGLE);
    }
}
