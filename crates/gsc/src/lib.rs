//! A virtual machine for Call of Duty 1.1 script (`.gsc`), and CoD2 1.3's
//! dialect of it.
//!
//! Language surface and the evidence behind it:
//! docs/research/cod11-gsc-language.md; CoD2's changes,
//! docs/research/cod2-gsc-diff.md section 3.

pub mod ast;
pub mod atom;
pub mod bytecode;
pub mod compile;
mod definite;
pub mod heap;
pub mod lex;
pub mod load;
pub mod parse;
pub mod value;
pub mod vm;

pub use atom::{Atom, Interner};
pub use compile::CompileOptions;
pub use heap::ArrayKey;
pub use load::{LoadError, Loader, LoaderState, ScriptSource, canonical};
// `Thread`/`ThreadState` are not re-exported: no `Vm` API returns one, so
// there is nothing for a consumer to do with either.
pub use value::{ArrayId, EntId, FuncRef, StructId, Value, format_g, format_number};
pub use vm::profile::VmProfile;
pub use vm::sched::ThreadId;
pub use vm::{
    ArrayCarry, Cx, ErrorKind, GameCarry, Host, InstallError, Program, ScriptError, Target,
    ThreadUsage, Vm,
};

/// Which game's compiler a script goes through. The VM runs both; only the
/// front end differs (docs/research/cod2-gsc-diff.md section 3).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Dialect {
    /// CoD 1.1, 1.5 and United Offensive.
    #[default]
    Cod1,
    /// CoD2 1.3: `#include`, `waittillframeend`, `x = thread f()`, bracket
    /// reads on `self` and `level`, definite assignment of locals.
    Cod2,
    /// CoD4 1.7: CoD2's and what the stock corpus adds
    /// (docs/research/cod4-gsc.md section 1).
    Cod4,
}

impl Dialect {
    /// CoD2's front end or a later one built on it.
    pub fn cod2_family(self) -> bool {
        matches!(self, Dialect::Cod2 | Dialect::Cod4)
    }
}
