//! CoD2's definite-assignment rule: reading a local that is not assigned
//! on every path to the read is a compile error, `uninitialised variable`
//! (docs/research/cod2-gsc-diff.md 3.5). 1.1 reads such a local as
//! undefined, so only a CoD2 compile runs this.
//!
//! A forward pass over the AST. The set of locals assigned so far meets
//! (intersects) where paths join; `None` is a point no path reaches, which
//! meets as everything. A loop body is checked against the set on entry,
//! the smallest of its iterations' sets; the loop's exit is its condition
//! failing (never for a constant true one) and each `break`.

use std::collections::HashSet;

use crate::ast::{ArmLabel, CallTarget, Expr, FuncDef, Stmt, StmtKind};
use crate::compile::CompileError;

type Set = Option<HashSet<String>>;

fn meet(a: Set, b: Set) -> Set {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(a), Some(b)) => Some(a.intersection(&b).cloned().collect()),
    }
}

fn fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// A condition the compiler can read as always true: `while (1)`,
/// `while (true)`.
fn always_true(cond: &Expr) -> bool {
    match cond {
        Expr::Int(n) => *n != 0,
        Expr::Float(f) => *f != 0.0,
        _ => false,
    }
}

pub(crate) fn check(def: &FuncDef) -> Result<(), CompileError> {
    let mut flow = Flow {
        line: def.line,
        breaks: Vec::new(),
        continues: Vec::new(),
    };
    let params = def.params.iter().map(|p| fold(p)).collect();
    flow.block(&def.body, Some(params))?;
    Ok(())
}

struct Flow {
    line: u32,
    /// The sets at each `break`, one frame per enclosing loop or switch.
    breaks: Vec<Vec<Set>>,
    /// The sets at each `continue`, one frame per enclosing loop.
    continues: Vec<Vec<Set>>,
}

impl Flow {
    fn block(&mut self, stmts: &[Stmt], mut st: Set) -> Result<Set, CompileError> {
        for s in stmts {
            st = self.stmt(s, st)?;
        }
        Ok(st)
    }

    fn stmt(&mut self, s: &Stmt, st: Set) -> Result<Set, CompileError> {
        // Code no path reaches reads nothing.
        let Some(mut set) = st else {
            return Ok(None);
        };
        self.line = s.line;
        Ok(match &s.kind {
            StmtKind::Expr(e) | StmtKind::Wait(e) => {
                self.expr(e, &mut set)?;
                Some(set)
            }
            StmtKind::WaitTillFrameEnd => Some(set),
            StmtKind::Assign { target, value } => {
                self.expr(value, &mut set)?;
                self.target(target, &mut set)?;
                Some(set)
            }
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                self.expr(cond, &mut set)?;
                let a = self.block(then, Some(set.clone()))?;
                let b = match otherwise {
                    Some(o) => self.block(o, Some(set))?,
                    None => Some(set),
                };
                meet(a, b)
            }
            StmtKind::While { cond, body } => {
                self.expr(cond, &mut set)?;
                let exit = (!always_true(cond)).then(|| set.clone());
                let (_, breaks, _) = self.looped(body, Some(set))?;
                breaks.into_iter().fold(exit, meet)
            }
            StmtKind::DoWhile { body, cond } => {
                let (end, breaks, continues) = self.looped(body, Some(set))?;
                let mut at_cond = continues.into_iter().fold(end, meet);
                if let Some(c) = at_cond.as_mut() {
                    self.expr(cond, c)?;
                }
                let exit = if always_true(cond) { None } else { at_cond };
                breaks.into_iter().fold(exit, meet)
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                let mut st = match init {
                    Some(i) => self.stmt(i, Some(set))?,
                    None => Some(set),
                };
                if let (Some(c), Some(s)) = (cond, st.as_mut()) {
                    self.expr(c, s)?;
                }
                let exit = match cond {
                    Some(c) if !always_true(c) => st.clone(),
                    _ => None,
                };
                let (end, breaks, continues) = self.looped(body, st)?;
                if let Some(step) = step {
                    self.stmt(step, continues.into_iter().fold(end, meet))?;
                }
                breaks.into_iter().fold(exit, meet)
            }
            StmtKind::Switch { subject, arms } => {
                self.expr(subject, &mut set)?;
                self.breaks.push(Vec::new());
                // Each arm is entered by its label's jump or by falling out
                // of the arm before it.
                let mut fall: Set = None;
                for arm in arms {
                    fall = self.block(&arm.body, meet(Some(set.clone()), fall))?;
                }
                let breaks = self.breaks.pop().unwrap_or_default();
                let has_default = arms.iter().any(|a| matches!(a.label, ArmLabel::Default));
                let mut out = breaks.into_iter().fold(fall, meet);
                if !has_default {
                    out = meet(out, Some(set));
                }
                out
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e, &mut set)?;
                }
                None
            }
            StmtKind::Break => {
                if let Some(f) = self.breaks.last_mut() {
                    f.push(Some(set));
                }
                None
            }
            StmtKind::Continue => {
                if let Some(f) = self.continues.last_mut() {
                    f.push(Some(set));
                }
                None
            }
        })
    }

    /// A loop body from `st`: its end, its breaks and its continues.
    #[allow(clippy::type_complexity)]
    fn looped(
        &mut self,
        body: &[Stmt],
        st: Set,
    ) -> Result<(Set, Vec<Set>, Vec<Set>), CompileError> {
        self.breaks.push(Vec::new());
        self.continues.push(Vec::new());
        let end = self.block(body, st);
        let breaks = self.breaks.pop().unwrap_or_default();
        let continues = self.continues.pop().unwrap_or_default();
        Ok((end?, breaks, continues))
    }

    /// An assignment target. `x[i] = v` creates `x` when it is undefined
    /// (`probe_autoviv`), so the root local of an index chain counts as
    /// assigned rather than read.
    fn target(&mut self, t: &Expr, set: &mut HashSet<String>) -> Result<(), CompileError> {
        match t {
            Expr::Local(n) => {
                set.insert(fold(n));
            }
            Expr::Index(obj, idx) => {
                self.expr(idx, set)?;
                match &**obj {
                    Expr::Local(_) | Expr::Index(..) => self.target(obj, set)?,
                    other => self.expr(other, set)?,
                }
            }
            other => self.expr(other, set)?,
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr, set: &mut HashSet<String>) -> Result<(), CompileError> {
        match e {
            Expr::Local(n) => {
                if !set.contains(&fold(n)) {
                    return Err(CompileError {
                        line: self.line,
                        msg: format!("uninitialised variable '{n}'"),
                    });
                }
            }
            Expr::VectorLit(a, b, c) => {
                self.expr(a, set)?;
                self.expr(b, set)?;
                self.expr(c, set)?;
            }
            Expr::Field(o, _) | Expr::Un(_, o) | Expr::Cast(_, o) => self.expr(o, set)?,
            Expr::Index(o, i) | Expr::Bin(_, o, i) => {
                self.expr(o, set)?;
                self.expr(i, set)?;
            }
            Expr::Call {
                recv, target, args, ..
            } => {
                if let Some(r) = recv {
                    self.expr(r, set)?;
                }
                if let CallTarget::Deref(p) = target {
                    self.expr(p, set)?;
                }
                // `waittill`'s arguments past the event name are the locals
                // it binds.
                let binds =
                    matches!(target, CallTarget::Name(n) if n.eq_ignore_ascii_case("waittill"));
                for (i, a) in args.iter().enumerate() {
                    match a {
                        Expr::Local(n) if binds && i > 0 => {
                            set.insert(fold(n));
                        }
                        _ => self.expr(a, set)?,
                    }
                }
            }
            Expr::Undefined
            | Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::Localized(_)
            | Expr::Anim(_)
            | Expr::AnimtreeRef
            | Expr::EmptyArray
            | Expr::SelfRef
            | Expr::LevelRef
            | Expr::GameRef
            | Expr::AnimRef
            | Expr::FuncRef { .. } => {}
        }
        Ok(())
    }
}
