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
    /// Pops the object then the value.
    StoreField(Atom),
    /// Pops the key then the object.
    LoadIndex,
    /// Pops the key, the object, then the value. An undefined value is
    /// kept as a key `.size` counts (`probe_undef_call_into_unset`).
    StoreIndex,
    /// `x[k] = undefined` with the literal: pops the key then the object,
    /// which must already be an array, and removes the key
    /// (`probe_array_undef`, `probe_undef_into_unset`).
    ClearIndex,
    /// Auto-vivifying counterpart to `LoadLocal` for an index-assignment
    /// target: an `Undefined` local becomes a new empty array, written back
    /// to the slot; an existing array is left alone; anything else is a
    /// `BadType` error. Either way pushes the array.
    EnsureArrayLocal(u16),
    /// Pushes a local as `LoadLocal` does, a shared array in it copied
    /// first (`Heap::own`) and written back: the local under
    /// `a[k] = undefined`, which does not vivify.
    OwnLocal(u16),
    /// Pops the object (struct or entity); same auto-vivification as
    /// `EnsureArrayLocal`, but writes the new array back to the field.
    EnsureArrayField(Atom),
    /// Pops the key then the array; same auto-vivification as
    /// `EnsureArrayLocal`, but writes the new array back to that element:
    /// the `array[i][j] = v` case.
    EnsureArrayIndex,
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
    /// CoD2's `/`: two ints divide as floats (`Scr_EvalDivide`, Mac
    /// 0x9012c).
    DivFloat,
    /// CoD2's `*`: numbers only, so a vector operand is an error
    /// (`Scr_EvalMultiply`, lnxded 0x807e290).
    MulNumber,
    /// CoD4's `*` and `/`: a number against a vector is cast up to one
    /// (`Scr_CastWeakerPair`) and the pair works per component; two ints
    /// divide as floats (`Scr_EvalMultiply`, `Scr_EvalDivide`;
    /// the stock scripts scale vectors both ways, `_helicopter.gsc` 393,
    /// `_hardpoints.gsc` 287).
    MulVector,
    DivVector,
    Mod,
    Neg,
    Not,
    /// Integer only; a non-integer operand is a `BadType` script error.
    BitAnd,
    BitOr,
    BitXor,
    /// CoD4's `<<` and `>>`, integer only.
    Shl,
    Shr,
    Eq,
    Ne,
    /// A `switch`'s subject: an int or a string passes, any other type
    /// follows `Vm::set_switch_other`. The operand is the switch's end,
    /// where 1.1d and UO go on below developer 1.
    SwitchKey(u32),
    /// A `case` test: the same type and value, no number tolerance and no
    /// string-to-number reading.
    CaseEq,
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
    /// CoD2's `waittillframeend`: suspends behind every thread due this
    /// frame. No operand.
    WaitTillFrameEnd,
    /// Pops the receiver and the event name; binds the extra notify
    /// arguments into `binds` in order.
    WaitTill {
        binds: Box<[u16]>,
    },
    /// CoD4's `waittillmatch`: pops the receiver, the event name and
    /// `argc` values the notify's first arguments must equal, in order.
    WaitTillMatch {
        argc: u8,
    },
    /// Pops the receiver, the event name and `argc` extra values.
    Notify {
        argc: u8,
    },
    /// Pops the receiver and the event name.
    EndOn,

    // Fused ops: never compiled, only placed by [`fuse`] in
    // `Prepared::fast`, each in place of the run of ops it names, which
    // stay in `Function::code` behind it.
    /// `LoadLocal a; LoadLocal b`.
    LoadLocal2(u16, u16),
    /// `StoreLocal a; LoadLocal b`.
    StoreLoad(u16, u16),
    /// `LoadLocal a; Return`.
    ReturnLocal(u16),
    /// `LoadLevel; LoadField f`.
    LevelField(Atom),
    /// `LoadLocal a; LoadField f`.
    LocalField(u16, Atom),
    /// `LoadLocal a; LoadLocal k; LoadIndex`.
    IndexLocals(u16, u16),
    /// `LoadLevel; LoadField f; LoadLocal k; LoadIndex; LoadField g`:
    /// `level.f[k].g`.
    LevelIndexField(Atom, u16, Atom),
    /// `LoadLocal a; LoadLocal k; LoadIndex; LoadField f`.
    IndexLocalsField(u16, u16, Atom),
    /// `Lt; JumpIfFalse t`, and the other three orderings.
    LtJumpIfFalse(u32),
    GtJumpIfFalse(u32),
    LeJumpIfFalse(u32),
    GeJumpIfFalse(u32),
    /// `LoadLocal b; Lt; JumpIfFalse t` and `Const c; Lt; JumpIfFalse t`,
    /// and the same with `Gt`.
    LtLocalJumpIfFalse(u16, u32),
    GtLocalJumpIfFalse(u16, u32),
    LtConstJumpIfFalse(u16, u32),
    GtConstJumpIfFalse(u16, u32),
    /// `StoreLocal a; LoadLocal b; LoadLocal c`.
    StoreLoad2(u16, u16, u16),
    /// `LoadLocal k; LoadIndex`.
    IndexLocal(u16),
    /// `Add; StoreLocal a`.
    AddStore(u16),
    /// `Dup; JumpIfFalse t; Pop`, an `&&`'s left side, and `Dup;
    /// JumpIfTrue t; Pop`, an `||`'s.
    AndJump(u32),
    OrJump(u32),
    /// `AddConst` on a loop's `i` (`dst` and `src` both `i`), with or
    /// without its `Jump`, then the `LtSizeJumpIfFalse` it reaches on `i`:
    /// a loop's step and condition. `body` is the op past the condition,
    /// `exit` its jump, `extra` the run's length less one.
    StepLtSize {
        i: u16,
        c: u16,
        a: u16,
        body: u32,
        exit: u32,
        extra: u8,
    },
    /// `LoadLocal i; LoadLevel; LoadField f; LoadField size; Lt;
    /// JumpIfFalse to`: a loop's `i < level.f.size`.
    LtLevelSizeJumpIfFalse {
        i: u16,
        f: Atom,
        to: u32,
    },
    /// `LoadLocal a; Const c; Lt; JumpIfFalse to`.
    LtLocalConstJumpIfFalse {
        a: u16,
        c: u16,
        to: u32,
    },
    /// `LoadLocal a; LoadLocal b; Sub`.
    SubLocals(u16, u16),
    /// `LoadLocal i; LoadLocal a; LoadField size; Lt; JumpIfFalse t`: a
    /// loop's `i < a.size`.
    LtSizeJumpIfFalse {
        i: u16,
        a: u16,
        to: u32,
    },
    /// `LoadLocal src; Const c; Add; StoreLocal dst`, then with `to`
    /// `Jump to`: a loop's `i++`.
    AddConst {
        dst: u16,
        src: u16,
        c: u16,
        to: Option<u32>,
    },
}

/// `Debug` only, for `Frame`'s derive (`vm/interp.rs`): nothing formats a
/// `Function` directly.
#[derive(Debug)]
pub struct Function {
    pub file: Atom,
    pub name: Atom,
    pub params: u8,
    pub locals: u16,
    pub code: Vec<Op>,
    /// `Vm::install` fills it, so a compiler leaves it empty.
    pub prepared: Prepared,
    pub consts: Vec<Value>,
    /// Source line per instruction, for error attribution.
    pub lines: Vec<u32>,
    /// Source position per instruction (the compiler's `Pos` rule):
    /// `scriptUsage`'s file, line and caret.
    pub at: Vec<crate::ast::Pos>,
}

/// (pops, pushes) for one instruction, independent of the values involved.
/// `pub(crate)` rather than private: `step_frames` (`vm/interp.rs`) checks
/// the instruction loop against this same table under `#[cfg(test)]`, the
/// only mechanical tripwire keeping this table and the loop in agreement.
pub(crate) fn stack_effect(op: &Op) -> (i32, i32) {
    match op {
        Op::Const(_)
        | Op::LoadLocal(_)
        | Op::LoadSelf
        | Op::LoadLevel
        | Op::LoadGame
        | Op::LoadAnim
        | Op::NewArray
        | Op::Dup => (0, 1),
        Op::StoreLocal(_)
        | Op::Pop
        | Op::JumpIfFalse(_)
        | Op::JumpIfTrue(_)
        | Op::Wait
        | Op::Return => (1, 0),
        Op::LoadField(_)
        | Op::Neg
        | Op::Not
        | Op::CastInt
        | Op::CastFloat
        | Op::CastVector
        | Op::SwitchKey(_)
        | Op::EnsureArrayField(_) => (1, 1),
        Op::EnsureArrayLocal(_) | Op::OwnLocal(_) => (0, 1),
        Op::WaitTillFrameEnd => (0, 0),
        Op::StoreField(_) => (2, 0),
        Op::LoadIndex | Op::EnsureArrayIndex => (2, 1),
        Op::ClearIndex => (2, 0),
        Op::StoreIndex => (3, 0),
        Op::MakeVector => (3, 1),
        Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::DivFloat
        | Op::MulNumber
        | Op::MulVector
        | Op::DivVector
        | Op::Mod
        | Op::BitAnd
        | Op::BitOr
        | Op::BitXor
        | Op::Shl
        | Op::Shr
        | Op::Eq
        | Op::Ne
        | Op::CaseEq
        | Op::Lt
        | Op::Gt
        | Op::Le
        | Op::Ge => (2, 1),
        Op::Call { argc, has_recv, .. } | Op::CallBuiltin { argc, has_recv, .. } => {
            (i32::from(*argc) + i32::from(*has_recv), 1)
        }
        // The pointer value is pushed after the receiver and arguments, and
        // popped first.
        Op::CallPtr { argc, has_recv, .. } => (i32::from(*argc) + i32::from(*has_recv) + 1, 1),
        Op::Jump(_) | Op::ReturnUndef => (0, 0),
        Op::WaitTill { .. } | Op::EndOn => (2, 0),
        Op::Notify { argc } | Op::WaitTillMatch { argc } => (2 + i32::from(*argc), 0),
        // A fused op's whole run.
        Op::LoadLocal2(..) => (0, 2),
        Op::StoreLoad(..) => (1, 1),
        Op::ReturnLocal(_) => (0, 0),
        Op::LevelField(_)
        | Op::LocalField(..)
        | Op::IndexLocals(..)
        | Op::IndexLocalsField(..)
        | Op::LevelIndexField(..) => (0, 1),
        Op::LtJumpIfFalse(_)
        | Op::GtJumpIfFalse(_)
        | Op::LeJumpIfFalse(_)
        | Op::GeJumpIfFalse(_) => (2, 0),
        Op::LtSizeJumpIfFalse { .. }
        | Op::AddConst { .. }
        | Op::StepLtSize { .. }
        | Op::LtLevelSizeJumpIfFalse { .. }
        | Op::LtLocalConstJumpIfFalse { .. } => (0, 0),
        Op::SubLocals(..) => (0, 1),
        Op::LtLocalJumpIfFalse(..)
        | Op::GtLocalJumpIfFalse(..)
        | Op::LtConstJumpIfFalse(..)
        | Op::GtConstJumpIfFalse(..) => (1, 0),
        Op::StoreLoad2(..) => (1, 2),
        Op::IndexLocal(_) => (1, 1),
        Op::AddStore(_) => (2, 0),
        // A run that jumps leaves the left side, one that falls through
        // pops it.
        Op::AndJump(_) | Op::OrJump(_) => (1, 0),
    }
}

/// The ops `Vm::run_ops` runs (`Prepared::fast`): `code` with a fused op
/// in place of each op that starts one of the runs the fused ops name,
/// longest first. The ops a run covers stay behind it, so a jump into the
/// run's middle lands where it always did, and a fused op that cannot take
/// its fast path (a type it does not handle, a budget too short for its
/// whole run) runs its first op alone. `size` is the interner's `size`.
/// The runs are the spawn wave's commonest (cod4-server-frame.md 9.2).
pub(crate) fn fuse(code: &[Op], size: Atom) -> Vec<Op> {
    (0..code.len())
        .map(|i| fused_at(code, i, size).unwrap_or_else(|| code[i].clone()))
        .collect()
}

/// A `JumpIfFalse`'s target.
fn jump_target(op: Option<&Op>) -> u32 {
    match op {
        Some(&Op::JumpIfFalse(t)) => t,
        _ => unreachable!("matched as a JumpIfFalse"),
    }
}

/// `LoadLocal i; LoadLocal a; LoadField size; Lt; JumpIfFalse to` at
/// `code[at]`, as `(i, a, to)`.
fn lt_size_at(code: &[Op], at: usize, size: Atom) -> Option<(u16, u16, u32)> {
    match code.get(at..at + 5)? {
        &[
            Op::LoadLocal(i),
            Op::LoadLocal(a),
            Op::LoadField(f),
            Op::Lt,
            Op::JumpIfFalse(to),
        ] if f == size => Some((i, a, to)),
        _ => None,
    }
}

/// The fused op for the run starting at `code[at]`.
fn fused_at(code: &[Op], at: usize, size: Atom) -> Option<Op> {
    use Op::*;
    let run = &code[at..];
    let op = |k: usize| run.get(k);
    Some(match (op(0)?, op(1)?) {
        (&LoadLocal(src), &Const(c)) if matches!((op(2), op(3)), (Some(Add), Some(&StoreLocal(d))) if d == src) =>
        {
            // The condition straight after the step, or where its `Jump`
            // lands.
            let (cond, extra) = match op(4) {
                Some(&Jump(t)) => (t as usize, 9),
                _ => (at + 4, 8),
            };
            match lt_size_at(code, cond, size) {
                Some((i, a, exit)) if i == src => StepLtSize {
                    i,
                    c,
                    a,
                    body: cond as u32 + 5,
                    exit,
                    extra,
                },
                _ => {
                    let to = match op(4) {
                        Some(&Jump(t)) => Some(t),
                        _ => None,
                    };
                    AddConst {
                        dst: src,
                        src,
                        c,
                        to,
                    }
                }
            }
        }
        (&LoadLocal(i), LoadLevel) => match run.get(2..6)? {
            &[LoadField(f), LoadField(s), Lt, JumpIfFalse(to)] if s == size && f != size => {
                LtLevelSizeJumpIfFalse { i, f, to }
            }
            _ => return None,
        },
        (&LoadLocal(a), &Const(c))
            if matches!((op(2), op(3)), (Some(Lt), Some(JumpIfFalse(_)))) =>
        {
            LtLocalConstJumpIfFalse {
                a,
                c,
                to: jump_target(op(3)),
            }
        }
        (&LoadLocal(a), &LoadLocal(b)) if matches!(op(2), Some(Sub)) => SubLocals(a, b),
        (&LoadLocal(i), &LoadLocal(a))
            if matches!(
                (op(2), op(3), op(4)),
                (Some(&LoadField(f)), Some(Lt), Some(JumpIfFalse(_))) if f == size
            ) =>
        {
            let Some(&JumpIfFalse(to)) = op(4) else {
                unreachable!()
            };
            LtSizeJumpIfFalse { i, a, to }
        }
        (&LoadLocal(src), &Const(c))
            if matches!((op(2), op(3)), (Some(Add), Some(StoreLocal(_)))) =>
        {
            let Some(&StoreLocal(dst)) = op(3) else {
                unreachable!()
            };
            let to = match op(4) {
                Some(&Jump(t)) => Some(t),
                _ => None,
            };
            AddConst { dst, src, c, to }
        }
        (&LoadLocal(a), &LoadLocal(k)) if matches!(op(2), Some(LoadIndex)) => match op(3) {
            Some(&LoadField(f)) => IndexLocalsField(a, k, f),
            _ => IndexLocals(a, k),
        },
        (&LoadLocal(k), LoadIndex) => IndexLocal(k),
        (&LoadLocal(b), Lt) if matches!(op(2), Some(JumpIfFalse(_))) => {
            LtLocalJumpIfFalse(b, jump_target(op(2)))
        }
        (&LoadLocal(b), Gt) if matches!(op(2), Some(JumpIfFalse(_))) => {
            GtLocalJumpIfFalse(b, jump_target(op(2)))
        }
        (&Const(c), Lt) if matches!(op(2), Some(JumpIfFalse(_))) => {
            LtConstJumpIfFalse(c, jump_target(op(2)))
        }
        (&Const(c), Gt) if matches!(op(2), Some(JumpIfFalse(_))) => {
            GtConstJumpIfFalse(c, jump_target(op(2)))
        }
        (&StoreLocal(a), &LoadLocal(b)) if matches!(op(2), Some(LoadLocal(_))) => {
            let Some(&LoadLocal(c)) = op(2) else {
                unreachable!()
            };
            StoreLoad2(a, b, c)
        }
        (Dup, &JumpIfFalse(t)) if matches!(op(2), Some(Pop)) => AndJump(t),
        (Dup, &JumpIfTrue(t)) if matches!(op(2), Some(Pop)) => OrJump(t),
        (Add, &StoreLocal(a)) => AddStore(a),
        (&LoadLocal(a), &LoadLocal(b)) => LoadLocal2(a, b),
        (&LoadLocal(a), &LoadField(f)) => LocalField(a, f),
        (&LoadLocal(a), Return) => ReturnLocal(a),
        (&StoreLocal(a), &LoadLocal(b)) => StoreLoad(a, b),
        (LoadLevel, &LoadField(f)) => match run.get(2..5) {
            Some(&[LoadLocal(k), LoadIndex, LoadField(g)]) if f != size => LevelIndexField(f, k, g),
            _ => LevelField(f),
        },
        (Lt, &JumpIfFalse(t)) => LtJumpIfFalse(t),
        (Gt, &JumpIfFalse(t)) => GtJumpIfFalse(t),
        (Le, &JumpIfFalse(t)) => LeJumpIfFalse(t),
        (Ge, &JumpIfFalse(t)) => GeJumpIfFalse(t),
        _ => return None,
    })
}

fn visit(
    entry_depth: &mut [Option<i32>],
    queue: &mut std::collections::VecDeque<usize>,
    len: usize,
    from: usize,
    succ: usize,
    depth: i32,
) -> Result<(), String> {
    if succ >= len {
        return Err(format!("instruction {from} falls off the end of code"));
    }
    match entry_depth[succ] {
        Some(existing) if existing != depth => Err(format!(
            "instruction {succ} is reached at stack depth {depth} from instruction {from}, \
             but at depth {existing} from another path"
        )),
        Some(_) => Ok(()),
        None => {
            entry_depth[succ] = Some(depth);
            queue.push_back(succ);
            Ok(())
        }
    }
}

/// Walks every instruction reachable from entry, along every jump and
/// fallthrough edge, and confirms the operand stack depth on entry to each
/// one agrees no matter which edge reached it, and never goes negative.
/// This is the check that catches a miscompile that leaves a value behind
/// on one path but not another: exactly the shape of bug a jump-target or
/// terminator error produces, and the reason to run it on every function a
/// loader accepts, not just in the compiler's own tests. Answers the
/// deepest the operand stack gets.
pub fn stack_depth(f: &Function) -> Result<usize, String> {
    let entry = entry_depths(f)?;
    Ok(deepest(f, &entry))
}

/// The deepest `f` takes the operand stack, from each op's depth on entry.
fn deepest(f: &Function, entry: &[Option<i32>]) -> usize {
    f.code
        .iter()
        .zip(entry)
        .filter_map(|(op, d)| {
            let d = (*d)?;
            let (pop, push) = stack_effect(op);
            Some(d.max(d - pop + push))
        })
        .max()
        .unwrap_or(0) as usize
}

/// What `Vm::install` works out for a function before it runs, from
/// [`stack_depth`]'s walk and [`fuse`].
#[derive(Debug, Default)]
pub struct Prepared {
    /// `code` with its runs fused, what the interpreter runs.
    pub fast: Vec<Op>,
    /// Each op's operand stack depth on entry, the one every path agrees
    /// on (0 for an op no path reaches): where an op that failed leaves the
    /// stack under the error recovery (`Vm::set_recover_errors`).
    pub depths: Vec<u32>,
    /// The deepest the stack gets, which each frame's stack is sized to.
    pub max_stack: u32,
}

/// [`Prepared`] for `f`, or [`stack_depth`]'s refusal. `size` is the
/// interner's `size`.
pub fn prepare(f: &Function, size: Atom) -> Result<Prepared, String> {
    let entry = entry_depths(f)?;
    Ok(Prepared {
        fast: fuse(&f.code, size),
        depths: entry.iter().map(|d| d.unwrap_or(0) as u32).collect(),
        max_stack: deepest(f, &entry) as u32,
    })
}

/// Each op's depth on entry, `None` for an op no path reaches.
fn entry_depths(f: &Function) -> Result<Vec<Option<i32>>, String> {
    if f.code.is_empty() {
        return Ok(Vec::new());
    }
    let mut entry_depth: Vec<Option<i32>> = vec![None; f.code.len()];
    let mut queue = std::collections::VecDeque::new();
    entry_depth[0] = Some(0);
    queue.push_back(0usize);

    while let Some(ip) = queue.pop_front() {
        let depth_in = entry_depth[ip].expect("queued positions always have a recorded depth");
        let op = &f.code[ip];
        let (pop, push) = stack_effect(op);
        if depth_in < pop {
            return Err(format!(
                "stack underflow at instruction {ip} ({op:?}): depth {depth_in}, needs {pop}"
            ));
        }
        let depth_out = depth_in - pop + push;

        match op {
            // Terminal: function exits, no successor. Also the check for a
            // miscompile that leaves a value behind (or drops one) evenly
            // on every straight-line path, which no join disagreement would
            // ever catch: `Return` must leave exactly its return value, and
            // `ReturnUndef` must leave nothing.
            Op::Return if depth_in != 1 => {
                return Err(format!(
                    "instruction {ip} (Return) exits with stack depth {depth_in}, expected 1"
                ));
            }
            Op::ReturnUndef if depth_in != 0 => {
                return Err(format!(
                    "instruction {ip} (ReturnUndef) exits with stack depth {depth_in}, expected 0"
                ));
            }
            Op::Return | Op::ReturnUndef => {}
            Op::Jump(t) => visit(
                &mut entry_depth,
                &mut queue,
                f.code.len(),
                ip,
                *t as usize,
                depth_out,
            )?,
            Op::JumpIfFalse(t) | Op::JumpIfTrue(t) | Op::SwitchKey(t) => {
                // A skipped switch leaves no key behind.
                let skip = if matches!(op, Op::SwitchKey(_)) { 1 } else { 0 };
                visit(
                    &mut entry_depth,
                    &mut queue,
                    f.code.len(),
                    ip,
                    *t as usize,
                    depth_out - skip,
                )?;
                visit(
                    &mut entry_depth,
                    &mut queue,
                    f.code.len(),
                    ip,
                    ip + 1,
                    depth_out,
                )?;
            }
            _ => visit(
                &mut entry_depth,
                &mut queue,
                f.code.len(),
                ip,
                ip + 1,
                depth_out,
            )?,
        }
    }
    Ok(entry_depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func(code: Vec<Op>) -> Function {
        let lines = vec![1; code.len()];
        let at = vec![Default::default(); code.len()];
        Function {
            file: Atom(0),
            name: Atom(0),
            params: 0,
            locals: 0,
            code,
            prepared: Default::default(),
            consts: Vec::new(),
            lines,
            at,
        }
    }

    #[test]
    fn a_balanced_function_passes() {
        let f = func(vec![Op::Const(0), Op::Pop, Op::ReturnUndef]);
        assert!(stack_depth(&f).is_ok());
    }

    #[test]
    fn a_bare_pop_with_nothing_pushed_underflows() {
        let f = func(vec![Op::Pop, Op::ReturnUndef]);
        assert!(stack_depth(&f).is_err());
    }

    /// A value pushed on every path and never popped or returned: no join
    /// ever disagrees on the depth, so only a terminal-depth check catches
    /// this leak.
    #[test]
    fn return_undef_with_a_value_still_on_the_stack_is_rejected() {
        let f = func(vec![Op::Const(0), Op::ReturnUndef]);
        assert!(stack_depth(&f).is_err());
    }

    /// The mirror leak for `Return`: it must exit with exactly its return
    /// value on the stack, not that value plus something left behind.
    #[test]
    fn return_with_an_extra_value_still_on_the_stack_is_rejected() {
        let f = func(vec![Op::Const(0), Op::Const(0), Op::Return]);
        assert!(stack_depth(&f).is_err());
    }

    /// The shape of the switch double-pop bug this checker exists to catch:
    /// one path into an instruction leaves a value behind, another path
    /// into the very same instruction does not.
    #[test]
    fn two_paths_disagreeing_on_depth_at_the_same_instruction_is_rejected() {
        // 0: Const        (depth 0 -> 1)
        // 1: JumpIfTrue 4  (depth 1 -> 0, taken: jump to 4 at depth 0)
        // 2: Const        (depth 0 -> 1, not taken: falls through)
        // 3: Jump 4        (falls into 4 at depth 1)
        // 4: Pop           (reached at depth 0 from ip 1, depth 1 from ip 3)
        // 5: ReturnUndef
        let f = func(vec![
            Op::Const(0),
            Op::JumpIfTrue(4),
            Op::Const(0),
            Op::Jump(4),
            Op::Pop,
            Op::ReturnUndef,
        ]);
        assert!(stack_depth(&f).is_err());
    }

    #[test]
    fn a_jump_target_past_the_end_of_code_is_rejected() {
        let f = func(vec![Op::Jump(5), Op::ReturnUndef]);
        assert!(stack_depth(&f).is_err());
    }
}
