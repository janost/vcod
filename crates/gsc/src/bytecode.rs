//! Per-function bytecode. A thread's resumption point is an index into
//! `Function::code`, which is what makes `wait` and `waittill` cheap.

use crate::atom::Atom;
use crate::value::{FuncRef, Value};

#[derive(Clone, PartialEq, Debug)]
pub enum Op {
    Const(u16),
    LoadLocal(u16),
    StoreLocal(u16),
    /// Pops the object, pushes the field.
    LoadField(Atom),
    /// Pops the value then the object.
    StoreField(Atom),
    /// Pops the key then the object.
    LoadIndex,
    /// Pops the value, the key, then the object.
    StoreIndex,
    LoadSelf,
    LoadLevel,
    LoadGame,
    LoadAnim,
    NewArray,
    MakeVector,
    Call {
        func: FuncRef,
        argc: u8,
        has_recv: bool,
        threaded: bool,
    },
    CallBuiltin {
        name: Atom,
        argc: u8,
        has_recv: bool,
    },
    /// Pops the function value after its arguments.
    CallPtr {
        argc: u8,
        has_recv: bool,
        threaded: bool,
    },
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Not,
    /// Integer only; a non-integer operand is a `BadType` script error.
    BitAnd,
    BitOr,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    CastInt,
    CastFloat,
    CastVector,
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    Pop,
    Dup,
    Return,
    ReturnUndef,
    /// Pops seconds.
    Wait,
    /// Pops the receiver and the event name; binds the extra notify
    /// arguments into `binds` in order.
    WaitTill {
        binds: Box<[u16]>,
    },
    /// Pops the receiver, the event name and `argc` extra values.
    Notify {
        argc: u8,
    },
    /// Pops the receiver and the event name.
    EndOn,
}

pub struct Function {
    pub file: Atom,
    pub name: Atom,
    pub params: u8,
    pub locals: u16,
    pub code: Vec<Op>,
    pub consts: Vec<Value>,
    /// Source line per instruction, for error attribution.
    pub lines: Vec<u32>,
}
