//! The body queue: the eight corpse entities a gametype's `cloneplayer`
//! fills. `docs/research/cod11-combat.md` section 5.2 is the source for the
//! size, the entity numbers, the fields copied off the dying player and the
//! `eFlags` toggle.

use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{Protocol, ENTITYNUM_NONE};

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
/// makes the client restart the animation.
const EFLAGS_ANIM_TOGGLE: i32 = 8;

/// `s.pos.trType`, the literal 5 the clone is given. With `trDuration` 0 the
/// trajectory evaluates to its base, so the corpse stays where it fell.
const CORPSE_TRTYPE: i32 = 5;

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
        p: &Protocol,
    ) -> u32 {
        let i = self.next % self.slots.len();
        self.next += 1;
        self.toggles[i] = !self.toggles[i];
        let number = BODY_FIRST + i as u32;
        self.slots[i] = Some(Body {
            state: clone_of(&state, number, self.toggles[i], now_ms, p),
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
        p: &Protocol,
    ) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(body) = slot else { continue };
            if body.born_ms != now_ms {
                continue;
            }
            if let Some(fresh) = body.source.take().and_then(&mut from_sim) {
                body.state = clone_of(&fresh, BODY_FIRST + i as u32, self.toggles[i], now_ms, p);
            }
        }
    }

    /// The live bodies, by entity number. `p` is unused -- a body's fields
    /// are resolved once, at the push -- and kept so a caller reads the same
    /// shape here as everywhere else on the wire path.
    pub fn entities(&self, p: &Protocol) -> impl Iterator<Item = (u32, EntityState)> + '_ {
        let _ = p;
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
/// The `0x800` bit the table also lists, and the 250 ms think that clears
/// it, are left out: no capture we hold carries a corpse's `eFlags`, so
/// there is nothing to check the pair against.
fn clone_of(
    state: &EntityState,
    number: u32,
    toggle: bool,
    now_ms: i32,
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
        (get("eFlags") & !EFLAGS_ANIM_TOGGLE) | if toggle { EFLAGS_ANIM_TOGGLE } else { 0 },
    );
    set("groundEntityNum", ENTITYNUM_NONE as i32);
    set("pos.trType", CORPSE_TRTYPE);
    set("pos.trTime", now_ms);
    for axis in 0..3 {
        for field in ["pos.trBase", "pos.trDelta", "apos.trBase"] {
            let name = format!("{field}[{axis}]");
            set(&name, get(&name));
        }
    }
    e
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
        let n = q.push(dying(p, 4), None, 1000, p);
        let (num, body) = q.entities(p).next().unwrap();
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
        q.push(dying(p, 4), None, 1000, p);
        assert_eq!(q.entities(p).count(), 1);
        q.push(dying(p, 5), None, 1000 + 300_000, p);
        assert_eq!(q.entities(p).count(), 2, "a body has no lifetime");
    }

    #[test]
    fn a_third_body_reuses_the_oldest_slot() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(2);
        let first = q.push(dying(p, 1), None, 0, p);
        q.push(dying(p, 2), None, 100, p);
        let third = q.push(dying(p, 3), None, 200, p);
        assert_eq!(third, first, "the third push reuses the first slot");
        assert_eq!(q.entities(p).count(), 2);
        let states: Vec<_> = q.entities(p).collect();
        let reused = states.iter().find(|(n, _)| *n == first).unwrap();
        assert_eq!(reused.1.field_i32(p, "clientNum"), 3);
    }

    /// The slot's `eFlags` anim-restart bit is inverted on every push, so
    /// two occupants of one slot never send the same word (section 5.2).
    #[test]
    fn a_slots_anim_toggle_flips_on_every_push() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(1);
        q.push(dying(p, 1), None, 0, p);
        let first = q.entities(p).next().unwrap().1.field_i32(p, "eFlags");
        q.push(dying(p, 2), None, 100, p);
        let second = q.entities(p).next().unwrap().1.field_i32(p, "eFlags");
        assert_ne!(first & EFLAGS_ANIM_TOGGLE, second & EFLAGS_ANIM_TOGGLE);
        // Everything but that bit is the player's own eFlags.
        assert_eq!(first & !EFLAGS_ANIM_TOGGLE, 16);
        assert_eq!(second & !EFLAGS_ANIM_TOGGLE, 16);
    }

    /// A body born this frame is re-read from its source client's sim once,
    /// so a death animation raised after the clone still reaches the wire.
    /// A body born earlier is left alone.
    #[test]
    fn refresh_newborn_takes_the_sims_anims_once() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(2);
        q.push(dying(p, 4), Some(4), 1000, p);
        let mut fresh = dying(p, 4);
        fresh.fields[EntityState::field_index(p, "legsAnim").unwrap()] = 18;
        let number = q.entities(p).next().unwrap().0;

        q.refresh_newborn(1000, |slot| (slot == 4).then(|| fresh.clone()), p);
        let (n, body) = q.entities(p).next().unwrap();
        assert_eq!(n, number, "the refresh keeps the body's own number");
        assert_eq!(body.number, number);
        assert_eq!(body.field_i32(p, "legsAnim"), 18);
        assert_eq!(body.field_i32(p, "eType"), ET_CORPSE);

        // The source is spent: a later frame does not re-read it.
        fresh.fields[EntityState::field_index(p, "legsAnim").unwrap()] = 999;
        q.refresh_newborn(1000, |_| Some(fresh.clone()), p);
        assert_eq!(q.entities(p).next().unwrap().1.field_i32(p, "legsAnim"), 18);
    }

    /// The queue is retail's own eight slots at 64..71, the numbers the
    /// retail hit fixtures show corpses arriving on.
    #[test]
    fn the_queue_is_eight_slots_starting_at_sixty_four() {
        let p = &PROTOCOL_V1;
        assert_eq!((BODY_FIRST, BODY_QUEUE_SIZE), (64, 8));
        let mut q = BodyQueue::new(BODY_QUEUE_SIZE);
        let numbers: Vec<u32> = (0..4).map(|i| q.push(dying(p, i), None, 0, p)).collect();
        assert_eq!(numbers, vec![64, 65, 66, 67]);
    }

    /// A corpse is not a mover: `groundEntityNum` is the literal 1023 and
    /// the trajectory holds it where it fell.
    #[test]
    fn a_body_does_not_move() {
        let p = &PROTOCOL_V1;
        let mut q = BodyQueue::new(1);
        q.push(dying(p, 0), None, 5000, p);
        let body = q.entities(p).next().unwrap().1;
        assert_eq!(body.field_i32(p, "groundEntityNum"), ENTITYNUM_NONE as i32);
        assert_eq!(body.field_i32(p, "pos.trType"), CORPSE_TRTYPE);
        assert_eq!(body.field_i32(p, "pos.trTime"), 5000);
        assert_eq!(body.field_i32(p, "pos.trDuration"), 0);
    }
}
