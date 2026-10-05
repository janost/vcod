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
pub mod load;
pub mod parse;
pub mod value;
pub mod vm;

pub use atom::{Atom, Interner};
pub use heap::ArrayKey;
pub use load::{LoadError, Loader, ScriptSource, canonical};
// `Thread`/`ThreadState` are not re-exported: no `Vm` API returns one, so
// there is nothing for a consumer to do with either.
pub use value::{ArrayId, EntId, FuncRef, StructId, Value, format_number};
pub use vm::sched::ThreadId;
pub use vm::{ArrayCarry, Cx, ErrorKind, GameCarry, Host, InstallError, ScriptError, Target, Vm};
