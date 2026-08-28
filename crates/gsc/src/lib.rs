//! A virtual machine for Call of Duty 1.1 script (`.gsc`).
//!
//! Language surface and the evidence behind it:
//! docs/research/cod11-gsc-language.md.

pub mod ast;
pub mod atom;
pub mod bytecode;
pub mod compile;
pub mod heap;
pub mod lex;
pub mod parse;
pub mod value;
pub mod vm;

pub use atom::{Atom, Interner, StrAtom};
pub use value::{ArrayId, EntId, FuncRef, StructId, Value};
pub use vm::{ErrorKind, Host, ScriptError, Vm};
