//! Call of Duty 2 1.3: asset formats and wire, each module grounded in its
//! `docs/research/cod2-*.md`. Everything here builds into the engine's
//! neutral types; nothing CoD1 reads from this crate.

pub mod collision;
pub mod d3dbsp;
pub mod efx;
pub mod font;
pub mod hudelem;
pub mod lightgrid;
pub mod map;
pub mod material;
pub mod materials;
pub mod model;
pub mod net;
pub mod outdoor;
pub mod player;
pub mod pmove;
pub mod rootdelta;
pub mod trajectory;
pub mod water;
pub mod weapon;
pub mod world;
pub mod xmodel;
