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

/// `Debug` only, for `Frame`'s derive (`vm/interp.rs`) -- nothing formats a
/// `Function` directly.
#[derive(Debug)]
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
        Op::LoadField(_) | Op::Neg | Op::Not | Op::CastInt | Op::CastFloat | Op::CastVector => {
            (1, 1)
        }
        Op::StoreField(_) => (2, 0),
        Op::LoadIndex => (2, 1),
        Op::StoreIndex => (3, 0),
        Op::MakeVector => (3, 1),
        Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Mod
        | Op::BitAnd
        | Op::BitOr
        | Op::Eq
        | Op::Ne
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
        Op::Notify { argc } => (2 + i32::from(*argc), 0),
    }
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
/// on one path but not another — exactly the shape of bug a jump-target or
/// terminator error produces, and the reason to run it on every function a
/// loader accepts, not just in the compiler's own tests.
pub fn stack_depth(f: &Function) -> Result<(), String> {
    if f.code.is_empty() {
        return Ok(());
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
                ))
            }
            Op::ReturnUndef if depth_in != 0 => {
                return Err(format!(
                    "instruction {ip} (ReturnUndef) exits with stack depth {depth_in}, expected 0"
                ))
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
            Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => {
                visit(
                    &mut entry_depth,
                    &mut queue,
                    f.code.len(),
                    ip,
                    *t as usize,
                    depth_out,
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func(code: Vec<Op>) -> Function {
        let lines = vec![1; code.len()];
        Function {
            file: Atom(0),
            name: Atom(0),
            params: 0,
            locals: 0,
            code,
            consts: Vec::new(),
            lines,
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
