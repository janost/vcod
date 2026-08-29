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
pub struct GEntity {
    pub inuse: bool,
    pub engine: Vec<Value>,
    /// Script-defined fields, including every radiant key. There is exactly
    /// one key-value store per object and it is the VM's.
    pub script: StructId,
    /// Stage 4 attaches a client here; until then a client-routed field is
    /// the same error retail raises for a null `ent->client`.
    pub client: Option<usize>,
    pub solid: bool,
    pub hidden: bool,
    /// `(model, tag)` pairs from `attach`, in call order; `getAttachSize`
    /// and friends index into this.
    pub attachments: Vec<(Atom, Atom)>,
    /// Set by `moveGravity`/`rotateVelocity`; stage 5 integrates it into the
    /// entity's motion once entities go on the wire.
    pub velocity: [f32; 3],
}

pub struct ObjectTable {
    ents: Vec<Option<GEntity>>,
    num_entities: u32,
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
        }
    }

    pub fn num_entities(&self) -> u32 {
        self.num_entities
    }

    /// `G_Spawn`: hand out the counter and increment. Retail only starts
    /// scanning for a freed slot once the counter reaches
    /// `ENTITYNUM_WORLD`; there we raise instead, because a map that
    /// allocates 950 entities is a bug we want to see, not a slot to reuse.
    /// That divergence is documented in the language doc's divergence list.
    pub fn spawn(&mut self, cx: &mut Cx) -> Result<EntId, ErrorKind> {
        if self.num_entities >= ENTITYNUM_WORLD {
            return Err(ErrorKind::BadType("entity table full"));
        }
        let id = EntId(self.num_entities);
        self.num_entities += 1;
        let script = cx.new_struct();
        self.ents[id.0 as usize] = Some(GEntity {
            inuse: true,
            engine: vec![Value::Undefined; engine_slot_count()],
            script,
            client: None,
            solid: true,
            hidden: false,
            attachments: Vec::new(),
            velocity: [0.0, 0.0, 0.0],
        });
        Ok(id)
    }

    pub fn free(&mut self, id: EntId) {
        if let Some(slot) = self.ents.get_mut(id.0 as usize) {
            *slot = None;
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

    /// Freeing a slot below the high-water mark does not renumber anything:
    /// retail only reuses a freed slot once the counter reaches
    /// `ENTITYNUM_WORLD`, which no map load approaches, so a freed slot simply
    /// stops appearing.
    #[test]
    fn a_freed_slot_leaves_the_iteration_and_the_numbering_alone() {
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
            assert_eq!(t.spawn(cx).unwrap(), EntId(74));
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
