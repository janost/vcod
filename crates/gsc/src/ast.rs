//! The parsed shape of a script file. Names stay as source text here;
//! interning happens during compilation.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    /// Binary `&`, and the target of the `|=` desugar. Integer only.
    BitAnd,
    BitOr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cast {
    Int,
    Float,
    Vector,
}

/// What a call names. `Name` is resolved at compile time against the file's
/// own functions first, then the builtin table, which is the engine's rule.
#[derive(Clone, PartialEq, Debug)]
pub enum CallTarget {
    Name(String),
    Path { file: String, name: String },
    Deref(Box<Expr>),
}

#[derive(Clone, PartialEq, Debug)]
pub enum Expr {
    Undefined,
    Int(i32),
    Float(f32),
    Str(String),
    Localized(String),
    Anim(String),
    VectorLit(Box<Expr>, Box<Expr>, Box<Expr>),
    EmptyArray,
    Local(String),
    SelfRef,
    LevelRef,
    GameRef,
    AnimRef,
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    FuncRef {
        file: String,
        name: String,
    },
    Call {
        recv: Option<Box<Expr>>,
        target: CallTarget,
        args: Vec<Expr>,
        threaded: bool,
    },
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Un(UnOp, Box<Expr>),
    Cast(Cast, Box<Expr>),
}
