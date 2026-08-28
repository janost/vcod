//! A virtual machine for Call of Duty 1.1 script (`.gsc`).
//!
//! Language surface and the evidence behind it:
//! docs/research/cod11-gsc-language.md.

pub mod atom;
pub mod value;

pub use atom::{Atom, Interner};
pub use value::{ArrayId, EntId, FuncRef, StructId, Value};
