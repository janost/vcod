//! What one game's VM does differently: the compiler's dialect and the
//! scheduling rules the host sets once a level loads (`Vm::apply_profile`).
//! The host owns the choice; the VM only reads the flags.

use crate::Dialect;

/// One game's VM rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VmProfile {
    /// The compiler front end the level's scripts go through.
    pub dialect: Dialect,
    /// A vector prints with `g` formatting (`Interner::vectors_g`).
    pub vectors_g: bool,
    /// One notify's waiters resume newest first ([`super::Vm::set_newest_waiter_first`]).
    pub newest_waiter_first: bool,
    /// An `endon` a nested spawn's notify fires ends the waiting thread
    /// mid-step ([`super::Vm::set_kill_mid_step`]).
    pub kill_mid_step: bool,
    /// `x = thread f()` takes the thread's value ([`super::Vm::set_thread_returns`]).
    pub thread_returns: bool,
    /// A `wait` counts whole server frames ([`super::Vm::set_frame_waits`]).
    pub frame_waits: bool,
    /// A function's `endon`s end when it returns
    /// ([`super::Vm::set_endons_end_with_call`]).
    pub endons_end_with_call: bool,
}

impl VmProfile {
    /// CoD 1.1, 1.5 and United Offensive.
    pub const COD1: VmProfile = VmProfile {
        dialect: Dialect::Cod1,
        vectors_g: false,
        newest_waiter_first: false,
        kill_mid_step: false,
        thread_returns: false,
        frame_waits: false,
        endons_end_with_call: false,
    };

    /// CoD2 1.3 (docs/research/cod2-gsc-diff.md).
    pub const COD2: VmProfile = VmProfile {
        dialect: Dialect::Cod2,
        vectors_g: true,
        newest_waiter_first: true,
        kill_mid_step: true,
        thread_returns: true,
        frame_waits: false,
        endons_end_with_call: false,
    };

    /// CoD4 1.7: CoD2's rules until a retail probe says otherwise
    /// (docs/research/cod4-gsc.md 4), but for its frame-counted `wait` and
    /// its call-scoped `endon`.
    pub const COD4: VmProfile = VmProfile {
        dialect: Dialect::Cod4,
        frame_waits: true,
        endons_end_with_call: true,
        ..VmProfile::COD2
    };
}

impl super::Vm {
    /// Sets every rule `p` names. The dialect is the loader's, not the VM's.
    pub fn apply_profile(&mut self, p: &VmProfile) {
        self.interner_mut().vectors_g = p.vectors_g;
        self.set_newest_waiter_first(p.newest_waiter_first);
        self.set_kill_mid_step(p.kill_mid_step);
        self.set_thread_returns(p.thread_returns);
        self.set_frame_waits(p.frame_waits);
        self.set_endons_end_with_call(p.endons_end_with_call);
    }
}
