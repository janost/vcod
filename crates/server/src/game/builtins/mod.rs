//! Builtin families. Dispatch is by folded name through `Cx::resolve_folded`,
//! matching the engine's own case-insensitive script-function lookup
//! (docs/research/cod11-gsc-language.md, atom identity).

pub mod attach;
pub mod client;
pub mod combat;
pub mod cvar;
pub mod entity;
pub mod env;
pub mod fx;
pub mod hud;
pub mod io;
pub mod math;
pub mod mover;
pub mod precache;
pub mod sound;
