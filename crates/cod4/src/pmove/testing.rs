//! The flat floor the pmove tests stand on, shared with the client's
//! predictor tests.

use glam::Vec3;

use super::{ENTITYNUM_WORLD, PmWorld, Tr};

/// A floor at z 0, solid below; its contacts carry `surface_flags`.
pub struct Floor {
    pub surface_flags: u32,
}

/// A floor with no surface type.
pub const FLOOR: Floor = Floor { surface_flags: 0 };

/// Asphalt (surface type 22), so the footsteps carry a surface.
pub const ASPHALT: Floor = Floor {
    surface_flags: 22 << 20,
};

impl PmWorld for Floor {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, _maxs: Vec3, _pass: u32, _mask: u32) -> Tr {
        let (a, b) = (start.z + mins.z, end.z + mins.z);
        let mut t = Tr::default();
        if a < 0.0 {
            t.fraction = 0.0;
            t.startsolid = true;
            t.allsolid = b < 0.0;
        } else if b < 0.125 && b < a {
            t.fraction = ((a - 0.125) / (a - b)).max(0.0);
        } else {
            return t;
        }
        t.normal = Vec3::Z;
        t.walkable = true;
        t.entity_num = ENTITYNUM_WORLD as u32;
        t.contents = 1;
        t.surface_flags = self.surface_flags;
        t
    }
}

#[cfg(any(test, feature = "testing"))]
/// Stand-in mantle clips for a replay with no zones: a capture that never
/// mantles asks the same traces with any clips installed.
pub fn stand_in_mantle() -> &'static super::mantle::MantleAnims {
    static ANIMS: std::sync::OnceLock<super::mantle::MantleAnims> = std::sync::OnceLock::new();
    ANIMS.get_or_init(|| super::mantle::synthetic(500, 400))
}
