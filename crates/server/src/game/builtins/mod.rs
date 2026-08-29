//! Builtin families. Dispatch is by folded name through `Cx::resolve_folded`,
//! matching the engine's own case-insensitive script-function lookup
//! (docs/research/cod11-gsc-language.md, atom identity).

pub mod entity;
pub mod env;
pub mod io;
