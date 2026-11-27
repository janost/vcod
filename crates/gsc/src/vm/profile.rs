//! What one game's VM does differently: the compiler's dialect and the
//! scheduling rules the host sets once a level loads (`Vm::apply_profile`).
//! The host owns the choice; the VM only reads the flags.

use crate::Dialect;

/// How `==` and `!=` test two numbers: `|a - b| < 1e-6` as floats, the
/// difference rounded one of two ways (cod11-gsc-object-model.md 9.3.1).
/// A NaN difference is never equal; [`VmProfile::nan_ne`] says what `!=`
/// reads then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberEq {
    /// The difference stays on the x87 stack at 80 bits (1.1d).
    Kept,
    /// The difference is stored as a float first (1.5b, UO, CoD2, CoD4).
    Stored,
}

/// What a `switch` on a value other than an int or a string does
/// (cod11-gsc-object-model.md 9.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchOther {
    /// `cannot switch on <type>`; below developer 1 the switch is skipped
    /// whole (1.1d, UO).
    ErrorSkips,
    /// `cannot switch on <type>`; below developer 1 no case matches, so
    /// `default` runs (CoD2, CoD4).
    ErrorNoMatch,
    /// No error: the switch takes the key of the thread step's last
    /// switch, and before one matches no case (1.5b).
    LastKey,
}

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
    /// A bracket read on a struct reads undefined
    /// ([`super::Vm::set_struct_index_reads_undefined`]).
    pub struct_index_reads_undefined: bool,
    /// How `==` rounds the difference of two numbers ([`super::Vm::set_number_eq`]).
    pub number_eq: NumberEq,
    /// `!=` holds when the difference of two numbers is NaN, as
    /// `!(a == b)` does ([`super::Vm::set_nan_ne`]).
    pub nan_ne: bool,
    /// A `switch` on a float, a vector or undefined ([`super::Vm::set_switch_other`]).
    pub switch_other: SwitchOther,
}

impl VmProfile {
    /// CoD 1.1.
    pub const COD1: VmProfile = VmProfile {
        dialect: Dialect::Cod1,
        vectors_g: false,
        newest_waiter_first: false,
        kill_mid_step: false,
        thread_returns: false,
        frame_waits: false,
        endons_end_with_call: false,
        struct_index_reads_undefined: false,
        number_eq: NumberEq::Kept,
        nan_ne: false,
        switch_other: SwitchOther::ErrorSkips,
    };

    /// CoD 1.5: 1.1's rules but for its bracket read on a struct, its `==`
    /// and its `switch` on another type.
    pub const COD1_15: VmProfile = VmProfile {
        struct_index_reads_undefined: true,
        number_eq: NumberEq::Stored,
        switch_other: SwitchOther::LastKey,
        ..VmProfile::COD1
    };

    /// United Offensive 1.51: 1.1's rules but for its `==`.
    pub const COD1_UO: VmProfile = VmProfile {
        number_eq: NumberEq::Stored,
        ..VmProfile::COD1
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
        struct_index_reads_undefined: false,
        number_eq: NumberEq::Stored,
        nan_ne: true,
        switch_other: SwitchOther::ErrorNoMatch,
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
        self.set_struct_index_reads_undefined(p.struct_index_reads_undefined);
        self.set_number_eq(p.number_eq);
        self.set_nan_ne(p.nan_ne);
        self.set_switch_other(p.switch_other);
    }
}
