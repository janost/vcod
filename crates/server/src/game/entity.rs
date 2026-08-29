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

#[cfg(test)]
mod tests {
    use super::*;

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
}
