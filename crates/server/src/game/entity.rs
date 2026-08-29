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
/// and slots 64..71 are reserved by something not yet identified.
pub const FIRST_MAP_ENTITY: u32 = 72;

/// `EntId(FIRST_HUD_ELEM..)` is a HUD element, which has its own field table,
/// so `getEntityNumber` on one is an error rather than a number.
pub const FIRST_HUD_ELEM: u32 = MAX_GENTITIES;

use crate::game::fields::engine_slot_count;
use vcod_gsc::{Atom, Cx, EntId, ErrorKind, StructId, Value};

/// One script-visible object. `engine` is indexed by the dense slot
/// `fields::route_entity` returns, so two aliased names hit one cell.
/// Liveness is the `Option` in `ObjectTable::ents`, not a flag on the entity:
/// retail's `inuse` byte and a `None` slot would be two representations of
/// one fact, and they disagree the moment anything is freed.
pub struct GEntity {
    pub engine: Vec<Value>,
    /// Script-defined fields, including every radiant key. There is exactly
    /// one key-value store per object and it is the VM's.
    pub script: StructId,
    pub solid: bool,
    pub hidden: bool,
    /// `(model, tag)` pairs from `attach`, in call order; `getAttachSize`
    /// and friends index into this.
    pub attachments: Vec<(Atom, Atom)>,
}

pub struct ObjectTable {
    ents: Vec<Option<GEntity>>,
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
            script,
            solid: true,
            hidden: false,
            attachments: Vec::new(),
        });
        Ok(id)
    }

    /// `G_FreeEntity` (0x66948): clear the slot and put it on the tail of
    /// the free list, but only for numbers above the reserved range (the
    /// `index <= 71` skip at 0x66bb1).
    pub fn free(&mut self, id: EntId) {
        let Some(slot) = self.ents.get_mut(id.0 as usize) else {
            return;
        };
        if slot.take().is_some() && id.0 >= FIRST_MAP_ENTITY {
            self.free_list.push_back(id.0);
        }
    }

    pub fn get(&self, id: EntId) -> Option<&GEntity> {
        self.ents.get(id.0 as usize)?.as_ref()
    }

    pub fn get_mut(&mut self, id: EntId) -> Option<&mut GEntity> {
        self.ents.get_mut(id.0 as usize)?.as_mut()
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
