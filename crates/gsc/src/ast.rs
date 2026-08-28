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
    /// Bare `#animtree`, no parens: the tree name set by an earlier
    /// `#using_animtree("name");` in this file, read back in expression
    /// position, e.g. `self UseAnimTree(#animtree);`.
    AnimtreeRef,
    VectorLit(Box<Expr>, Box<Expr>, Box<Expr>),
    EmptyArray,
    Local(String),
    SelfRef,
    LevelRef,
    GameRef,
    AnimRef,
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// A function pointer value. `file` is empty for a bare `::name`
    /// pointer, resolved against this file's own functions first, same
    /// rule as `CallTarget::Name`.
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

/// A `case` label, or `default`, which is not a value test but a fallback:
/// its position among the arms is semantically load-bearing exactly like a
/// `case`'s, since a subject that matches nothing falls into `default`
/// wherever it lexically sits and keeps falling through from there.
#[derive(Clone, PartialEq, Debug)]
pub enum ArmLabel {
    Case(Expr),
    Default,
}

#[derive(Clone, PartialEq, Debug)]
pub struct SwitchArm {
    pub label: ArmLabel,
    /// Empty when this case falls through to the next.
    pub body: Vec<Stmt>,
}

/// A statement paired with its source line, so a compile error or an
/// eventual runtime `ScriptError` can name the line that produced it.
#[derive(Clone, PartialEq, Debug)]
pub struct Stmt {
    pub line: u32,
    pub kind: StmtKind,
}

#[derive(Clone, PartialEq, Debug)]
pub enum StmtKind {
    Expr(Expr),
    /// `target = value`. `target` is a `Local`, `Field` or `Index`.
    Assign {
        target: Expr,
        value: Expr,
    },
    If {
        cond: Expr,
        then: Vec<Stmt>,
        otherwise: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    /// `do { ... } while (cond);`. One occurrence in the whole corpus,
    /// `animscripts/predict.gsc`, and it still has to parse.
    DoWhile {
        body: Vec<Stmt>,
        cond: Expr,
    },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Box<Stmt>>,
        body: Vec<Stmt>,
    },
    /// Arms in source order; `default` is one of them (`ArmLabel::Default`),
    /// not a separate field, because its lexical position matters.
    Switch {
        subject: Expr,
        arms: Vec<SwitchArm>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    /// `wait <expr>;`, seconds.
    Wait(Expr),
}

#[derive(Clone, PartialEq, Debug)]
pub struct FuncDef {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub line: u32,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct File {
    pub animtrees: Vec<String>,
    pub funcs: Vec<FuncDef>,
}
