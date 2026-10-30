//! Builtin families. Dispatch is by folded name through `Cx::resolve_folded`,
//! matching the engine's own case-insensitive script-function lookup
//! (docs/research/cod11-gsc-language.md, atom identity).

pub mod attach;
pub mod client;
pub mod cod2;
pub mod cod2_fx;
pub mod cod2_trigger;
pub mod cod4;
pub mod cod4_client;
pub mod cod4_combat;
pub mod cod4_names;
pub mod cod4_script;
pub mod cod4_trace;
pub mod cod4_vehicle;
pub mod combat;
pub mod cvar;
pub mod entity;
pub mod env;
pub mod fx;
pub mod hud;
pub mod io;
pub mod math;
pub mod message;
pub mod mover;
pub mod objective;
pub mod precache;
pub mod score;
pub mod set;
pub mod sound;
pub mod uo;
pub mod vehicle;
