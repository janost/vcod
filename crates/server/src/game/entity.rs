//! The script object table: one flat table of gentities and HUD elements,
//! numbered as the wire numbers them. Layout and numbering facts are in
//! docs/research/cod11-gsc-object-model.md.

/// `MAX_GENTITIES` on CoD 1.1; the object table's gentity range is
/// `0..MAX_GENTITIES` and HUD elements start above it.
pub const MAX_GENTITIES: u32 = 1024;

/// The world. `G_SpawnEntitiesFromString` runs `SP_worldspawn` on the first
/// entity block directly rather than allocating one, so this number is never
/// handed out by the allocator.
pub const ENTITYNUM_WORLD: u32 = 1022;

/// `G_InitGame` sets `level.num_entities` to 72, so the first entity the map
/// load creates is number 72 whatever `sv_maxclients` is. `MAX_CLIENTS` is 64
/// and slots 64..71 are the body queue (`crate::game::bodies`).
pub const FIRST_MAP_ENTITY: u32 = 72;

/// `EntId(FIRST_HUD_ELEM..)` is a HUD element, which has its own field table,
/// so `getEntityNumber` on one is an error rather than a number.
pub const FIRST_HUD_ELEM: u32 = MAX_GENTITIES;

/// `g_hudelems` is 1024 records of 124 bytes: `GScr_NewHudElem` (0x4b184)
/// and `HudElem_Alloc` (0x4c470) both scan it with the bound `index <=
/// 0x3ff`, and `Scr_GetHudElemField` (0x4c003) indexes it as `(n * 32 - n) *
/// 4`.
pub const MAX_HUDELEMS: u32 = 1024;

use crate::game::fields::{
    engine_slot_count, hud_slot_count, pers_index, route_hud, Route, CLIENT_FIELDS, HUD_WHITE,
};
use crate::server::MAX_CLIENTS;
use vcod_gsc::{Atom, Cx, EntId, ErrorKind, StructId, Value};

/// One script-visible object. `engine` is indexed by the dense slot
/// `fields::route_entity` returns, so two aliased names hit one cell.
/// Liveness is the `Option` in `ObjectTable::ents`, not a flag on the entity:
/// retail's `inuse` byte and a `None` slot would be two representations of
/// one fact, and they disagree the moment anything is freed.
pub struct GEntity {
    pub engine: Vec<Value>,
    /// A client entity's `gclient_t` fields, indexed by raw `CLIENT_FIELDS`
    /// position (`Route::Client`), a separate store from `engine` because
    /// retail's `gclient_s *client` is a separate struct from `gentity_t` —
    /// sharing one array between the two field tables' index spaces would
    /// alias unrelated fields together. `Some` only for an entity
    /// `spawn_client` made; `None` for every map entity and HUD element,
    /// which is what makes a client-routed field on one of those a real
    /// error rather than a coincidence of a number comparison.
    pub client: Option<Vec<Value>>,
    /// Script-defined fields, including every radiant key. There is exactly
    /// one key-value store per object and it is the VM's.
    pub script: StructId,
    pub solid: bool,
    pub hidden: bool,
    /// `(model, tag)` pairs from `attach`, in call order; `getAttachSize`
    /// and friends index into this.
    pub attachments: Vec<(Atom, Atom)>,
    /// The half of a `g_hudelems` record no script field reaches: what the
    /// hudelem methods write, and who the element is drawn for. `Some` only
    /// for a record `spawn_hud_elem` made, the same test `client` is.
    pub hud: Option<HudState>,
    /// What runs when `nextthink` comes due, or `None` for no think armed.
    pub think: Option<ThinkFn>,
    /// The think deadline on `GameHost::level_time_ms`'s clock. Retail's 0
    /// means "no think" rather than "due immediately"
    /// (docs/research/cod11-gsc-object-model.md section 14), which is why
    /// `run_thinks` treats 0 as idle too.
    pub nextthink: i32,
}

/// `hudelem_t`'s owner field (`+0x70`) for an element every client is drawn:
/// `ENTITYNUM_NONE`, which `GScr_NewHudElem` writes at 0x4b249.
pub const HUD_OWNER_ALL: u32 = 0x3ff;

/// The `hudelem_t` members the script field table does not cover. The
/// script-visible half (x, y, colour, alignment, sort, `archived`) stays in
/// the record's engine slots, so each fact has one home; these are what the
/// hudelem methods write plus the two filters the wire build reads
/// (docs/research/cod11-hud-protocol.md section 5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudState {
    /// `+0x0`, the tagged union's tag: 1 text, 2 value, 3 shader, 4..7 the
    /// four timers, 8/9 the two clocks. 0 is a free record, which is why a
    /// fresh element is 1 and not 0 (`GScr_NewHudElem` 0x4b1b0).
    pub elem_type: i32,
    /// `+0x30`, `+0x34`, `+0x38`: `setShader`'s size and material index.
    pub width: i32,
    pub height: i32,
    pub shader: i32,
    /// `+0x5c`, the timers' absolute end time on the level clock.
    pub time: i32,
    /// `+0x64`, `setValue`'s number.
    pub value: f32,
    /// `+0x68`, `setText`'s localized-string index.
    pub text: i32,
    /// `+0x70`: the client this is drawn for, or [`HUD_OWNER_ALL`].
    pub owner: u32,
    /// `+0x74`: the `clientState.team` this is drawn for, 0 for every team.
    pub team: i32,
}

impl Default for HudState {
    /// `GScr_NewHudElem`'s non-zero defaults for the fields it covers
    /// (0x4b184): a text element owned by nobody in particular.
    fn default() -> Self {
        HudState {
            elem_type: 1,
            width: 0,
            height: 0,
            shader: 0,
            time: 0,
            value: 0.0,
            text: 0,
            owner: HUD_OWNER_ALL,
            team: 0,
        }
    }
}

impl HudState {
    /// The block every value-setting method clears before it writes its own
    /// (`setText` 0x4c5bc..0x4c61d and its four twins clear the same one),
    /// so an element that was a timer and becomes text carries no stale
    /// deadline.
    pub fn clear_payload(&mut self) {
        self.width = 0;
        self.height = 0;
        self.shader = 0;
        self.time = 0;
        self.value = 0.0;
        self.text = 0;
    }
}

/// What an entity's `think` runs when `nextthink` comes due. A Rust enum
/// rather than a script reference: retail's `think` is a C function pointer
/// (`G_FreeEntity` for a deleted entity) and script-side timing goes through
/// `thread`/`wait`. Stage 4's movers and doors add variants.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ThinkFn {
    Free,
}

pub struct ObjectTable {
    ents: Vec<Option<GEntity>>,
    /// The HUD element range, `EntId(FIRST_HUD_ELEM..)`. A separate vector
    /// rather than a longer `ents`, so `iter_inuse` — and through it
    /// `getEntArray` — cannot see a HUD element. A HUD element reuses
    /// `GEntity` for the two stores it does need (engine slots, keyed by
    /// `fields::route_hud`, and its own script struct); the gentity-only
    /// `solid`, `hidden` and `attachments` go unused.
    huds: Vec<Option<GEntity>>,
    num_entities: u32,
    /// `level.firstFreeEnt`/`level.lastFreeEnt` (level+0x10, level+0x14): a
    /// FIFO of freed slots that `G_Spawn` drains before it bumps the counter.
    free_list: std::collections::VecDeque<u32>,
}

impl Default for ObjectTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectTable {
    pub fn new() -> Self {
        ObjectTable {
            ents: (0..MAX_GENTITIES).map(|_| None).collect(),
            huds: (0..MAX_HUDELEMS).map(|_| None).collect(),
            num_entities: FIRST_MAP_ENTITY,
            free_list: std::collections::VecDeque::new(),
        }
    }

    pub fn num_entities(&self) -> u32 {
        self.num_entities
    }

    /// `G_Spawn` (0x667e0): pop the head of the free list if it has one,
    /// otherwise hand out `level.num_entities` and increment. Retail calls
    /// `G_Error` when the counter reaches `ENTITYNUM_WORLD` with nothing
    /// free; we raise instead.
    pub fn spawn(&mut self, cx: &mut Cx) -> Result<EntId, ErrorKind> {
        let id = match self.free_list.pop_front() {
            Some(n) => EntId(n),
            None => {
                if self.num_entities >= ENTITYNUM_WORLD {
                    return Err(ErrorKind::BadType("entity table full"));
                }
                let id = EntId(self.num_entities);
                self.num_entities += 1;
                id
            }
        };
        let script = cx.new_struct();
        self.ents[id.0 as usize] = Some(GEntity {
            engine: vec![Value::Undefined; engine_slot_count()],
            client: None,
            script,
            solid: true,
            hidden: false,
            attachments: Vec::new(),
            hud: None,
            think: None,
            nextthink: 0,
        });
        Ok(id)
    }

    /// A client's entity, at entity number == its client slot. Retail's
    /// `G_InitGame` sets `level.num_entities` to 72 whatever
    /// `sv_maxclients` is and `MAX_CLIENTS` is 64, so 0..63 belong to
    /// clients and never reach the allocator
    /// (docs/research/cod11-gsc-object-model.md section 2). A second call on
    /// a slot that already holds a client entity replaces it outright, the
    /// same full reset a reconnect wants.
    pub fn spawn_client(&mut self, cx: &mut Cx, slot: usize) -> Result<EntId, ErrorKind> {
        if slot >= MAX_CLIENTS {
            return Err(ErrorKind::BadType("client slot out of range"));
        }
        let script = cx.new_struct();
        // A numeric client field is a plain int or float on the `gclient_t`,
        // which `ClientConnect` has already zeroed, so retail's getter can
        // never hand script an undefined one. That is what lets every stock
        // gametype write `self.deaths++` and `attacker.score++` on a client
        // that has scored nothing yet. The string and object fields keep
        // their undefined reading: those have custom getters, and what each
        // returns before it is written is unmeasured.
        let mut client: Vec<Value> = CLIENT_FIELDS
            .iter()
            .map(|f| match f.ty {
                crate::game::fields::FieldType::Int => Value::Int(0),
                crate::game::fields::FieldType::Float => Value::Float(0.0),
                _ => Value::Undefined,
            })
            .collect();
        // `.pers` is an array from the moment the client connects: the
        // engine writes the handle in `ClientConnect` (0x4250f), and every
        // gametype's first act is to read `self.pers["team"]` off it without
        // creating it. Measured live on the retail 1.1d server: defined
        // before `begin`, with `pers["team"]` undefined inside it
        // (docs/research/cod11-gsc-object-model.md, "Client fields").
        //
        // A fresh array every call, where retail's handle is what survives
        // the `gclient_t` reset -- that is the whole point of `pers`, whose
        // contents outlive a round. Not measured, and equivalent today only
        // because nothing yet re-runs `spawn_client` on a live client: a map
        // change or a round restart, neither of which exists, is where the
        // two readings part.
        client[pers_index()] = Value::Array(cx.new_array());
        let mut engine = vec![Value::Undefined; engine_slot_count()];
        // A client is `classname` "player" from the moment it exists, because
        // that is how every stock script reaches the players:
        // `getentarray("player", "classname")` in `bel.gsc` and the spawn
        // logic. Without it `_spawnlogic::getSpawnpoint_DM` finds nobody to
        // score against and quietly degrades to `getSpawnpoint_Random`.
        if let crate::game::fields::Route::Engine { slot: cs, .. } =
            crate::game::fields::route_entity("classname")
        {
            engine[cs] = Value::String(cx.intern_exact("player"));
        }
        self.ents[slot] = Some(GEntity {
            engine,
            client: Some(client),
            script,
            solid: true,
            hidden: false,
            attachments: Vec::new(),
            hud: None,
            think: None,
            nextthink: 0,
        });
        Ok(EntId(slot as u32))
    }

    /// The client's slot goes back to being empty. It is deliberately not
    /// pushed onto the free list: that list feeds `spawn`, and a map entity
    /// must never be handed a client's number.
    ///
    /// `GameHost::client_weapons` and `client_viewmodel` are left alone; the
    /// Connect path resets them, so the slot is clean before anything can
    /// read it and a reset here would only be the second half of the same
    /// guarantee.
    pub fn free_client(&mut self, slot: usize) {
        if slot < MAX_CLIENTS {
            self.ents[slot] = None;
        }
    }

    /// `GScr_NewHudElem` (0x4b184): hand out the first free `g_hudelems`
    /// slot, or fail the way retail's `Scr_Error("out of hudelems")` does
    /// once all 1024 are taken. There is no free list, because retail has
    /// none here either: the allocator is a linear scan for a clear record.
    ///
    /// Retail zeroes the record and then sets four non-zero defaults:
    /// `fontscale` 1.0 (0x4b19b), a packed white `color` (0x4b1d9),
    /// `archived` 1 (0x4b1fc) and the owner `ENTITYNUM_NONE` (0x4b249).
    /// The first three are engine slots, the last is [`HudState`]. Every
    /// other slot is left `undefined` rather than retail's zero, the
    /// convention a gentity's fields follow; the wire build reads an
    /// undefined slot as the zero it stands for.
    pub fn spawn_hud_elem(&mut self, cx: &mut Cx) -> Result<EntId, ErrorKind> {
        let Some(i) = self.huds.iter().position(|h| h.is_none()) else {
            return Err(ErrorKind::BadType("out of hudelems"));
        };
        let script = cx.new_struct();
        let mut engine = vec![Value::Undefined; hud_slot_count()];
        for (name, v) in [
            ("fontscale", Value::Float(1.0)),
            ("color", Value::Int(HUD_WHITE)),
            ("archived", Value::Int(1)),
        ] {
            if let Route::Engine { slot, .. } = route_hud(name) {
                engine[slot] = v;
            }
        }
        self.huds[i] = Some(GEntity {
            engine,
            client: None,
            script,
            solid: true,
            hidden: false,
            attachments: Vec::new(),
            hud: Some(HudState::default()),
            think: None,
            nextthink: 0,
        });
        Ok(EntId(FIRST_HUD_ELEM + i as u32))
    }

    /// `G_FreeEntity` (0x66948): clear the slot and put it on the tail of
    /// the free list, but only for numbers above the reserved range (the
    /// `index <= 71` skip at 0x66bb1). A HUD element takes the other path,
    /// `HudElem_Free` (0x4c570), which only clears the record.
    pub fn free(&mut self, id: EntId) {
        if id.0 >= FIRST_HUD_ELEM {
            if let Some(slot) = self.huds.get_mut((id.0 - FIRST_HUD_ELEM) as usize) {
                *slot = None;
            }
            return;
        }
        let Some(slot) = self.ents.get_mut(id.0 as usize) else {
            return;
        };
        if slot.take().is_some() && id.0 >= FIRST_MAP_ENTITY {
            self.free_list.push_back(id.0);
        }
    }

    /// Arms `id`'s think. `at_ms` is on the same clock `run_thinks` reads
    /// (`GameHost::level_time_ms`).
    pub fn schedule(&mut self, id: EntId, think: ThinkFn, at_ms: i32) {
        if let Some(e) = self.get_mut(id) {
            e.think = Some(think);
            e.nextthink = at_ms;
        }
    }

    /// `G_RunFrame`'s think pass: fire every entity whose `nextthink` has
    /// come due. Collect first, then act: a think that frees its entity
    /// would otherwise invalidate the walk.
    pub fn run_thinks(&mut self, now_ms: i32) {
        let due: Vec<(EntId, ThinkFn)> = self
            .ents
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let e = e.as_ref()?;
                let think = e.think?;
                // Retail's `nextthink` of 0 is "no think", not "due now".
                (e.nextthink != 0 && e.nextthink <= now_ms).then_some((EntId(i as u32), think))
            })
            .collect();
        for (id, think) in due {
            if let Some(e) = self.get_mut(id) {
                e.think = None;
                e.nextthink = 0;
            }
            match think {
                ThinkFn::Free => self.free(id),
            }
        }
    }

    pub fn get(&self, id: EntId) -> Option<&GEntity> {
        match id.0.checked_sub(FIRST_HUD_ELEM) {
            Some(i) => self.huds.get(i as usize)?.as_ref(),
            None => self.ents.get(id.0 as usize)?.as_ref(),
        }
    }

    pub fn get_mut(&mut self, id: EntId) -> Option<&mut GEntity> {
        match id.0.checked_sub(FIRST_HUD_ELEM) {
            Some(i) => self.huds.get_mut(i as usize)?.as_mut(),
            None => self.ents.get_mut(id.0 as usize)?.as_mut(),
        }
    }

    /// Ascending `g_hudelems` index, live records only. That is the order
    /// retail copies them to a client in (`HudElem_UpdateClient` 0x4bf00
    /// walks the pool from slot 0), and so the order they take on the wire.
    pub fn iter_hud_elems(&self) -> impl Iterator<Item = (EntId, &GEntity)> {
        self.huds
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (EntId(FIRST_HUD_ELEM + i as u32), e)))
    }

    /// Ascending entity number, live slots only: `Scr_GetEntArray`'s walk.
    pub fn iter_inuse(&self) -> impl Iterator<Item = (EntId, &GEntity)> {
        self.ents
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (EntId(i as u32), e)))
    }

    #[cfg(test)]
    pub fn force_num_entities_for_test(&mut self, n: u32) {
        self.num_entities = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_gsc::EntId;

    /// Numbering comes from the retail server, not from `sv_maxclients`:
    /// `G_InitGame` sets `level.num_entities` to 72 and `G_Spawn` hands out
    /// that counter until it reaches `ENTITYNUM_WORLD`
    /// (docs/research/cod11-gsc-object-model.md section 11).
    #[test]
    fn the_numbering_constants_match_retail() {
        assert_eq!(FIRST_MAP_ENTITY, 72);
        assert_eq!(ENTITYNUM_WORLD, 1022);
        assert_eq!(MAX_GENTITIES, 1024);
        assert_eq!(FIRST_HUD_ELEM, MAX_GENTITIES);
    }

    /// `G_Spawn` hands out `level.num_entities` and increments, so the first
    /// spawn is 72 and they climb from there
    /// (docs/research/cod11-gsc-object-model.md section 11).
    #[test]
    fn spawn_numbers_climb_from_seventy_two() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let mut t = ObjectTable::new();
            assert_eq!(t.spawn(cx).unwrap(), EntId(72));
            assert_eq!(t.spawn(cx).unwrap(), EntId(73));
            assert_eq!(t.num_entities(), 74);
        });
    }

    /// A freed slot leaves the iteration and is handed out again by the next
    /// spawn, ahead of the counter. Measured on the retail server: after a
    /// `delete()` had actually taken effect, the next spawn returned the
    /// deleted entity's number and the one after that continued from the
    /// high-water mark (docs/research/cod11-gsc-object-model.md section 14).
    #[test]
    fn a_freed_slot_is_handed_out_again_before_the_counter() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let mut t = ObjectTable::new();
            let a = t.spawn(cx).unwrap();
            let b = t.spawn(cx).unwrap();
            t.free(a);
            assert_eq!(
                t.iter_inuse().map(|(id, _)| id).collect::<Vec<_>>(),
                vec![b]
            );
            assert_eq!(t.spawn(cx).unwrap(), a);
            assert_eq!(t.spawn(cx).unwrap(), EntId(74));
        });
    }

    /// The free list is a FIFO, matching `G_FreeEntity`'s append-at-tail and
    /// `G_Spawn`'s pop-from-head, and a slot freed twice is only queued once.
    #[test]
    fn the_free_list_is_first_in_first_out() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let mut t = ObjectTable::new();
            let a = t.spawn(cx).unwrap();
            let b = t.spawn(cx).unwrap();
            let c = t.spawn(cx).unwrap();
            t.free(b);
            t.free(a);
            t.free(a);
            assert_eq!(t.spawn(cx).unwrap(), b);
            assert_eq!(t.spawn(cx).unwrap(), a);
            assert_eq!(t.spawn(cx).unwrap(), EntId(c.0 + 1));
        });
    }

    /// Iteration is ascending entity number, which is what `getEntArray`'s
    /// order rests on (`Scr_GetEntArray` 0x61980).
    #[test]
    fn iteration_is_ascending_entity_number() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let mut t = ObjectTable::new();
            let ids: Vec<_> = (0..5).map(|_| t.spawn(cx).unwrap()).collect();
            assert_eq!(t.iter_inuse().map(|(id, _)| id).collect::<Vec<_>>(), ids);
        });
    }

    /// `delete()` defers the free: the entity keeps its number and its
    /// place in `iter_inuse` until the think comes due, which is what
    /// `probe_delete` measures on retail. Freeing immediately made the
    /// number available a frame early and dropped the entity out of
    /// `getEntArray` sooner than retail.
    #[test]
    fn a_scheduled_free_keeps_the_entity_until_the_think_is_due() {
        let mut vm = vcod_gsc::Vm::new();
        let mut ents = ObjectTable::new();
        let id = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        ents.schedule(id, ThinkFn::Free, 100);

        ents.run_thinks(50);
        assert!(ents.get(id).is_some(), "freed before the think was due");
        assert_eq!(ents.iter_inuse().count(), 1);

        ents.run_thinks(100);
        assert!(ents.get(id).is_none(), "still live past the think");
        assert_eq!(ents.iter_inuse().count(), 0);

        // The number is back on the free list, so the next spawn reuses it.
        let next = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(next, id);
    }

    /// A think fires once. Retail clears `nextthink` when it runs, so a
    /// scheduled free cannot double-free a slot something else has since
    /// taken.
    #[test]
    fn a_think_fires_once() {
        let mut vm = vcod_gsc::Vm::new();
        let mut ents = ObjectTable::new();
        let id = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        ents.schedule(id, ThinkFn::Free, 100);
        ents.run_thinks(100);
        let reused = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(reused, id);
        ents.run_thinks(200);
        assert!(
            ents.get(reused).is_some(),
            "the think fired twice and freed the reuse"
        );
    }

    /// A client's entity number is its slot, and taking one neither advances
    /// `num_entities` nor touches the free list, so map entities still start
    /// at `FIRST_MAP_ENTITY` whatever clients have connected.
    #[test]
    fn a_client_entity_takes_its_slot_number_and_leaves_map_numbering_alone() {
        let mut vm = vcod_gsc::Vm::new();
        let mut ents = ObjectTable::new();
        let before = ents.num_entities();
        let id = vm.with_cx(|cx| ents.spawn_client(cx, 3).unwrap());
        assert_eq!(id, vcod_gsc::EntId(3));
        assert_eq!(
            ents.num_entities(),
            before,
            "a client moved the map counter"
        );
        let first_map = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(first_map, vcod_gsc::EntId(FIRST_MAP_ENTITY));
    }

    /// Freeing a client returns its slot to nothing: the number is the slot's,
    /// not the allocator's, so it must not land on the free list where a map
    /// entity could take it.
    #[test]
    fn freeing_a_client_does_not_hand_its_number_to_the_map() {
        let mut vm = vcod_gsc::Vm::new();
        let mut ents = ObjectTable::new();
        vm.with_cx(|cx| ents.spawn_client(cx, 2).unwrap());
        ents.free_client(2);
        let first_map = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(first_map, vcod_gsc::EntId(FIRST_MAP_ENTITY));
    }

    /// The table refuses to hand out `ENTITYNUM_WORLD` or anything above it.
    #[test]
    fn the_table_stops_at_entitynum_world() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let mut t = ObjectTable::new();
            t.force_num_entities_for_test(ENTITYNUM_WORLD);
            assert!(t.spawn(cx).is_err());
        });
    }
}
