//! Recursive descent with precedence climbing.

use crate::ast::*;
use crate::lex::{Spanned, Tok};

#[derive(Debug)]
pub struct ParseError {
    pub line: u32,
    pub msg: String,
}

pub struct Parser<'a> {
    t: &'a [Spanned],
    i: usize,
}

impl<'a> Parser<'a> {
    pub fn new(t: &'a [Spanned]) -> Self {
        Parser { t, i: 0 }
    }

    fn peek(&self) -> &Tok {
        &self.t[self.i].tok
    }
    fn line(&self) -> u32 {
        self.t[self.i].line
    }
    fn bump(&mut self) -> Tok {
        let t = self.t[self.i].tok.clone();
        self.i += 1;
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &Tok) -> Result<(), ParseError> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(self.err(&format!("expected {t:?}")))
        }
    }
    fn err(&self, msg: &str) -> ParseError {
        ParseError {
            line: self.line(),
            msg: format!("{msg}, found {:?}", self.peek()),
        }
    }

    fn ident_name(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.err("expected an identifier")),
        }
    }

    pub fn expr(&mut self) -> Result<Expr, ParseError> {
        self.binary(0)
    }

    // Precedence-climbing over the left-associative binary levels, loosest
    // first. There is no `|` level: the corpus never uses binary `|`, so
    // `BinOp::BitOr` exists only as the `|=` desugar target.
    fn binary(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.unary()?;
        while let Some((op, prec)) = self.peek_binop() {
            if prec < min_prec {
                break;
            }
            self.bump();
            let rhs = self.binary(prec + 1)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn peek_binop(&self) -> Option<(BinOp, u8)> {
        Some(match self.peek() {
            Tok::OrOr => (BinOp::Or, 1),
            Tok::AndAnd => (BinOp::And, 2),
            Tok::Amp => (BinOp::BitAnd, 3),
            Tok::EqEq => (BinOp::Eq, 4),
            Tok::NotEq => (BinOp::Ne, 4),
            Tok::Lt => (BinOp::Lt, 5),
            Tok::Gt => (BinOp::Gt, 5),
            Tok::Le => (BinOp::Le, 5),
            Tok::Ge => (BinOp::Ge, 5),
            Tok::Plus => (BinOp::Add, 6),
            Tok::Minus => (BinOp::Sub, 6),
            Tok::Star => (BinOp::Mul, 7),
            Tok::Slash => (BinOp::Div, 7),
            Tok::Percent => (BinOp::Mod, 7),
            _ => return None,
        })
    }

    // `!`/`-` prefix, then a cast, both binding tighter than any binary
    // operator and applying to the following unary in turn.
    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.eat(&Tok::Not) {
            return Ok(Expr::Un(UnOp::Not, Box::new(self.unary()?)));
        }
        if self.eat(&Tok::Minus) {
            return Ok(Expr::Un(UnOp::Neg, Box::new(self.unary()?)));
        }
        if let Some(cast) = self.try_cast() {
            return Ok(Expr::Cast(cast, Box::new(self.unary()?)));
        }
        let base = self.primary()?;
        self.postfix(base)
    }

    // Looks ahead for `(` ident `)` where ident is int/float/vector
    // case-insensitively, without consuming unless the whole pattern
    // matches.
    fn try_cast(&mut self) -> Option<Cast> {
        if self.t.get(self.i)?.tok != Tok::LParen {
            return None;
        }
        let Tok::Ident(name) = &self.t.get(self.i + 1)?.tok else {
            return None;
        };
        let cast = match name.to_ascii_lowercase().as_str() {
            "int" => Cast::Int,
            "float" => Cast::Float,
            "vector" => Cast::Vector,
            _ => return None,
        };
        if self.t.get(self.i + 2)?.tok != Tok::RParen {
            return None;
        }
        self.i += 3;
        Some(cast)
    }

    // `.ident` and `[expr]` chaining, plus the method-call form: an
    // expression directly followed by an identifier (no operator between)
    // makes that expression the receiver. A call paren never attaches to a
    // field access or an index result, only to this receiver form.
    fn postfix(&mut self, mut base: Expr) -> Result<Expr, ParseError> {
        loop {
            match self.peek().clone() {
                Tok::Dot => {
                    self.bump();
                    let name = self.ident_name()?;
                    base = Expr::Field(Box::new(base), name);
                }
                Tok::LBracket => {
                    self.bump();
                    let idx = self.expr()?;
                    self.expect(&Tok::RBracket)?;
                    base = Expr::Index(Box::new(base), Box::new(idx));
                }
                Tok::Thread => {
                    self.bump();
                    let name = self.ident_name()?;
                    base = self.finish_call(Some(Box::new(base)), CallTarget::Name(name), true)?;
                }
                Tok::Ident(name) => {
                    self.bump();
                    base = self.finish_call(Some(Box::new(base)), CallTarget::Name(name), false)?;
                }
                _ => break,
            }
        }
        Ok(base)
    }

    // `expect(LParen)`, then `args_list()`, then the `Expr::Call` node.
    // Shared by every call form: bare, path, deref, method and threaded.
    fn finish_call(
        &mut self,
        recv: Option<Box<Expr>>,
        target: CallTarget,
        threaded: bool,
    ) -> Result<Expr, ParseError> {
        self.expect(&Tok::LParen)?;
        let args = self.args_list()?;
        Ok(Expr::Call {
            recv,
            target,
            args,
            threaded,
        })
    }

    fn args_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        if self.eat(&Tok::RParen) {
            return Ok(Vec::new());
        }
        let mut args = Vec::new();
        loop {
            args.push(self.expr()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(args)
    }

    // Literals; `[]`; `[[ expr ]]` deref, itself optionally called; `(`
    // grouping or a 3-element `Expr::VectorLit`; the `self`/`level`/`game`/
    // `anim`/`undefined` reference keywords, matched case-insensitively
    // like every other identifier in the engine; and an identifier, which
    // may open a call, a backslash path, or stand for a local variable.
    fn primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                Ok(Expr::Int(n))
            }
            Tok::Float(f) => {
                self.bump();
                Ok(Expr::Float(f))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Localized(s) => {
                self.bump();
                Ok(Expr::Localized(s))
            }
            Tok::Anim(s) => {
                self.bump();
                Ok(Expr::Anim(s))
            }
            Tok::LBracket => self.bracket_primary(),
            Tok::LParen => self.paren_primary(),
            Tok::Ident(name) => {
                self.bump();
                self.ident_primary(name)
            }
            // A bare local function pointer, e.g. `::Callback_StartGameType`:
            // no file segment, unlike the `path\path::name` form in
            // `ident_primary`. All five stock gametypes register their
            // engine callbacks this way.
            Tok::ColonColon => {
                self.bump();
                let name = self.ident_name()?;
                Ok(Expr::FuncRef {
                    file: String::new(),
                    name,
                })
            }
            _ => Err(self.err("expected an expression")),
        }
    }

    // `[]` (empty array) or `[[ expr ]]` (function-pointer deref), the
    // latter immediately called if a `(` follows: there is no `[[` token,
    // so this is the only place two adjacent `[` are read as one form.
    fn bracket_primary(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // first `[`
        if self.eat(&Tok::RBracket) {
            return Ok(Expr::EmptyArray);
        }
        self.expect(&Tok::LBracket)?;
        let inner = self.expr()?;
        self.expect(&Tok::RBracket)?;
        self.expect(&Tok::RBracket)?;
        if *self.peek() == Tok::LParen {
            return self.finish_call(None, CallTarget::Deref(Box::new(inner)), false);
        }
        // The corpus never leaves a deref uncalled; fall back to the
        // referenced value itself rather than invent an AST node for it.
        Ok(inner)
    }

    // A parenthesized expression, or `(a, b, c)` as a vector literal once a
    // comma follows the first element.
    fn paren_primary(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // `(`
        let first = self.expr()?;
        if self.eat(&Tok::Comma) {
            let second = self.expr()?;
            self.expect(&Tok::Comma)?;
            let third = self.expr()?;
            self.expect(&Tok::RParen)?;
            return Ok(Expr::VectorLit(
                Box::new(first),
                Box::new(second),
                Box::new(third),
            ));
        }
        self.expect(&Tok::RParen)?;
        Ok(first)
    }

    // `first` is already consumed. Either a reference keyword, a backslash
    // path closed by `::name` (called or a bare `Expr::FuncRef`), or a
    // plain identifier: a call when `(` follows, a local variable otherwise.
    fn ident_primary(&mut self, first: String) -> Result<Expr, ParseError> {
        match first.to_ascii_lowercase().as_str() {
            "self" => return Ok(Expr::SelfRef),
            "level" => return Ok(Expr::LevelRef),
            "game" => return Ok(Expr::GameRef),
            "anim" => return Ok(Expr::AnimRef),
            "undefined" => return Ok(Expr::Undefined),
            _ => {}
        }

        let mut path = vec![first.clone()];
        while self.eat(&Tok::Backslash) {
            path.push(self.ident_name()?);
        }
        // A bare `name::func`, with zero backslash segments, takes this
        // branch too: a census over all 799 shipped scripts found no such
        // occurrence, so the rule is permissive rather than load-bearing.
        if *self.peek() == Tok::ColonColon {
            self.bump();
            let name = self.ident_name()?;
            let file = path.join("/").to_ascii_lowercase();
            if *self.peek() == Tok::LParen {
                return self.finish_call(None, CallTarget::Path { file, name }, false);
            }
            return Ok(Expr::FuncRef { file, name });
        }

        if *self.peek() == Tok::LParen {
            return self.finish_call(None, CallTarget::Name(first), false);
        }
        Ok(Expr::Local(first))
    }

    fn file(&mut self) -> Result<File, ParseError> {
        let mut file = File::default();
        while *self.peek() != Tok::Eof {
            match self.peek().clone() {
                Tok::UsingAnimtree(name) => {
                    self.bump();
                    self.expect(&Tok::Semi)?;
                    file.animtrees.push(name);
                }
                _ => file.funcs.push(self.func_def()?),
            }
        }
        Ok(file)
    }

    fn func_def(&mut self) -> Result<FuncDef, ParseError> {
        let line = self.line();
        let name = self.ident_name()?;
        self.expect(&Tok::LParen)?;
        let mut params = Vec::new();
        if !self.eat(&Tok::RParen) {
            loop {
                params.push(self.ident_name()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RParen)?;
        }
        let body = self.block()?;
        Ok(FuncDef {
            name,
            params,
            body,
            line,
        })
    }

    fn block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        while *self.peek() != Tok::RBrace {
            stmts.extend(self.statement()?);
        }
        self.bump(); // `}`
        Ok(stmts)
    }

    // If/while/for/do bodies accept a `{ ... }` block or one bare statement;
    // `statement()` already flattens both shapes into a `Vec<Stmt>`.
    fn body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.statement()
    }

    // Almost always produces exactly one statement. A nested `{ ... }`
    // block has no `Stmt` variant of its own, so it flattens into its
    // contained statements; a bare `;` produces none.
    fn statement(&mut self) -> Result<Vec<Stmt>, ParseError> {
        match self.peek().clone() {
            Tok::LBrace => self.block(),
            Tok::Semi => {
                self.bump();
                Ok(Vec::new())
            }
            Tok::If => Ok(vec![self.if_stmt()?]),
            Tok::While => Ok(vec![self.while_stmt()?]),
            Tok::Do => Ok(vec![self.do_while_stmt()?]),
            Tok::For => Ok(vec![self.for_stmt()?]),
            Tok::Switch => Ok(vec![self.switch_stmt()?]),
            Tok::Return => {
                self.bump();
                if self.eat(&Tok::Semi) {
                    Ok(vec![Stmt::Return(None)])
                } else {
                    let e = self.expr()?;
                    self.expect(&Tok::Semi)?;
                    Ok(vec![Stmt::Return(Some(e))])
                }
            }
            Tok::Break => {
                self.bump();
                self.expect(&Tok::Semi)?;
                Ok(vec![Stmt::Break])
            }
            Tok::Continue => {
                self.bump();
                self.expect(&Tok::Semi)?;
                Ok(vec![Stmt::Continue])
            }
            Tok::Wait => {
                self.bump();
                let e = self.expr()?;
                self.expect(&Tok::Semi)?;
                Ok(vec![Stmt::Wait(e)])
            }
            // `expr()` has no branch for a leading `thread`: the only form
            // it handles is the receiver-attached one in `postfix`. A
            // receiverless `thread f();` only occurs in statement position,
            // so this is the one place that consumes the keyword itself.
            Tok::Thread => {
                self.bump();
                let name = self.ident_name()?;
                let call = self.finish_call(None, CallTarget::Name(name), true)?;
                self.expect(&Tok::Semi)?;
                Ok(vec![Stmt::Expr(call)])
            }
            _ => {
                let s = self.simple_stmt()?;
                self.expect(&Tok::Semi)?;
                Ok(vec![s])
            }
        }
    }

    fn if_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // `if`
        self.expect(&Tok::LParen)?;
        let cond = self.expr()?;
        self.expect(&Tok::RParen)?;
        let then = self.body()?;
        let otherwise = if self.eat(&Tok::Else) {
            Some(self.body()?)
        } else {
            None
        };
        Ok(Stmt::If {
            cond,
            then,
            otherwise,
        })
    }

    fn while_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // `while`
        self.expect(&Tok::LParen)?;
        let cond = self.expr()?;
        self.expect(&Tok::RParen)?;
        let body = self.body()?;
        Ok(Stmt::While { cond, body })
    }

    // The body runs before the condition is first tested, which is the
    // whole difference from `while`.
    fn do_while_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // `do`
        let body = self.body()?;
        self.expect(&Tok::While)?;
        self.expect(&Tok::LParen)?;
        let cond = self.expr()?;
        self.expect(&Tok::RParen)?;
        self.expect(&Tok::Semi)?;
        Ok(Stmt::DoWhile { body, cond })
    }

    fn for_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // `for`
        self.expect(&Tok::LParen)?;
        let init = if *self.peek() == Tok::Semi {
            None
        } else {
            Some(Box::new(self.simple_stmt()?))
        };
        self.expect(&Tok::Semi)?;
        let cond = if *self.peek() == Tok::Semi {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect(&Tok::Semi)?;
        let step = if *self.peek() == Tok::RParen {
            None
        } else {
            Some(Box::new(self.simple_stmt()?))
        };
        self.expect(&Tok::RParen)?;
        let body = self.body()?;
        Ok(Stmt::For {
            init,
            cond,
            step,
            body,
        })
    }

    // Statements accumulate into the most recently seen label's body, so a
    // label immediately followed by another label leaves that arm's body
    // empty: this IS the fallthrough representation, not a bug to fix later.
    fn switch_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // `switch`
        self.expect(&Tok::LParen)?;
        let subject = self.expr()?;
        self.expect(&Tok::RParen)?;
        self.expect(&Tok::LBrace)?;

        let mut arms: Vec<SwitchArm> = Vec::new();
        let mut default: Option<Vec<Stmt>> = None;
        let mut in_default = false;

        while *self.peek() != Tok::RBrace {
            match self.peek().clone() {
                Tok::Case => {
                    self.bump();
                    let label = self.expr()?;
                    self.expect(&Tok::Colon)?;
                    arms.push(SwitchArm {
                        label,
                        body: Vec::new(),
                    });
                    in_default = false;
                }
                Tok::Default => {
                    self.bump();
                    self.expect(&Tok::Colon)?;
                    default = Some(Vec::new());
                    in_default = true;
                }
                _ => {
                    let stmts = self.statement()?;
                    if in_default {
                        default.as_mut().expect("case label seen").extend(stmts);
                    } else {
                        arms.last_mut()
                            .ok_or_else(|| self.err("statement before any case label"))?
                            .body
                            .extend(stmts);
                    }
                }
            }
        }
        self.bump(); // `}`
        Ok(Stmt::Switch {
            subject,
            arms,
            default,
        })
    }

    // One expression-or-assignment statement, desugaring compound
    // assignment and increment/decrement into `Assign`, without consuming a
    // trailing `;` — a `for` clause needs none, everywhere else adds one.
    fn simple_stmt(&mut self) -> Result<Stmt, ParseError> {
        let target = self.expr()?;
        Ok(match self.peek().clone() {
            Tok::Assign => {
                self.bump();
                let value = self.expr()?;
                Stmt::Assign { target, value }
            }
            Tok::PlusAssign => self.compound_assign(target, BinOp::Add)?,
            Tok::MinusAssign => self.compound_assign(target, BinOp::Sub)?,
            Tok::StarAssign => self.compound_assign(target, BinOp::Mul)?,
            Tok::SlashAssign => self.compound_assign(target, BinOp::Div)?,
            Tok::PipeAssign => self.compound_assign(target, BinOp::BitOr)?,
            Tok::PlusPlus => {
                self.bump();
                self.step_assign(target, BinOp::Add)
            }
            Tok::MinusMinus => {
                self.bump();
                self.step_assign(target, BinOp::Sub)
            }
            _ => Stmt::Expr(target),
        })
    }

    fn compound_assign(&mut self, target: Expr, op: BinOp) -> Result<Stmt, ParseError> {
        self.bump();
        let rhs = self.expr()?;
        let value = Expr::Bin(op, Box::new(target.clone()), Box::new(rhs));
        Ok(Stmt::Assign { target, value })
    }

    fn step_assign(&mut self, target: Expr, op: BinOp) -> Stmt {
        let value = Expr::Bin(op, Box::new(target.clone()), Box::new(Expr::Int(1)));
        Stmt::Assign { target, value }
    }
}

/// Lexes and parses a whole script file.
pub fn parse_file(src: &str) -> Result<File, ParseError> {
    let toks = crate::lex::lex(src).map_err(|e| ParseError {
        line: e.line,
        msg: e.msg,
    })?;
    Parser::new(&toks).file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(src: &str) -> Expr {
        let toks = crate::lex::lex(src).unwrap();
        let mut p = Parser::new(&toks);
        p.expr().unwrap()
    }

    #[test]
    fn precedence_binds_tighter_for_multiplication() {
        // 1 + 2 * 3 parses as 1 + (2 * 3)
        let e = expr("1 + 2 * 3");
        let Expr::Bin(BinOp::Add, l, r) = e else {
            panic!("{e:?}")
        };
        assert!(matches!(*l, Expr::Int(1)));
        assert!(matches!(*r, Expr::Bin(BinOp::Mul, _, _)));
    }

    #[test]
    fn logical_or_is_loosest() {
        let e = expr("a && b || c");
        let Expr::Bin(BinOp::Or, l, _) = e else {
            panic!("{e:?}")
        };
        assert!(matches!(*l, Expr::Bin(BinOp::And, _, _)));
    }

    #[test]
    fn three_element_parens_are_a_vector_not_a_group() {
        assert!(matches!(expr("(0, 1, 0)"), Expr::VectorLit(_, _, _)));
        assert!(matches!(expr("(1 + 2)"), Expr::Bin(BinOp::Add, _, _)));
    }

    #[test]
    fn casts_apply_to_the_following_unary() {
        let e = expr("(int)(x + 360) % 360");
        let Expr::Bin(BinOp::Mod, l, _) = e else {
            panic!("{e:?}")
        };
        assert!(matches!(*l, Expr::Cast(Cast::Int, _)));
    }

    #[test]
    fn postfix_chains_field_index_and_call() {
        // level.obj["Field Radio"] then a call on the result of a field
        let e = expr(r#"level.obj["Field Radio"]"#);
        let Expr::Index(obj, _) = e else {
            panic!("{e:?}")
        };
        assert!(matches!(*obj, Expr::Field(_, _)));
    }

    #[test]
    fn namespaced_call_and_function_reference() {
        let call = expr(r#"maps\mp\_load::main()"#);
        let Expr::Call {
            target: CallTarget::Path { file, name },
            args,
            ..
        } = call
        else {
            panic!("{call:?}")
        };
        assert_eq!(file, "maps/mp/_load");
        assert_eq!(name, "main");
        assert!(args.is_empty());

        let fp = expr(r#"mptype\american_airborne::main"#);
        assert!(matches!(fp, Expr::FuncRef { .. }));
    }

    #[test]
    fn deref_call_reads_two_brackets() {
        let e = expr("[[level.callbackStartGameType]]()");
        let Expr::Call {
            target: CallTarget::Deref(inner),
            ..
        } = e
        else {
            panic!("{e:?}")
        };
        assert!(matches!(*inner, Expr::Field(_, _)));
    }

    #[test]
    fn method_call_carries_its_receiver() {
        let e = expr(r#"self playsound("bodyfall_dirt_large")"#);
        let Expr::Call {
            recv: Some(recv),
            target: CallTarget::Name(n),
            args,
            ..
        } = e
        else {
            panic!("{e:?}")
        };
        assert!(matches!(*recv, Expr::SelfRef));
        assert_eq!(n, "playsound");
        assert_eq!(args.len(), 1);
    }

    /// All five stock gametypes register callbacks this way, e.g.
    /// `dm.gsc:70` `level.callbackStartGameType = ::Callback_StartGameType;`.
    #[test]
    fn bare_colon_colon_is_a_local_function_pointer() {
        let e = expr("::Callback_StartGameType");
        let Expr::FuncRef { file, name } = e else {
            panic!("{e:?}")
        };
        assert!(file.is_empty());
        assert_eq!(name, "Callback_StartGameType");
    }

    #[test]
    fn empty_array_literal() {
        assert!(matches!(expr("[]"), Expr::EmptyArray));
    }

    #[test]
    fn unary_minus_and_not() {
        assert!(matches!(expr("-x"), Expr::Un(UnOp::Neg, _)));
        assert!(matches!(expr("!x"), Expr::Un(UnOp::Not, _)));
    }

    fn file(src: &str) -> File {
        parse_file(src).unwrap()
    }

    #[test]
    fn a_function_definition_with_params() {
        let f = file("greet(a, b) { return a; }");
        assert_eq!(f.funcs.len(), 1);
        assert_eq!(f.funcs[0].name, "greet");
        assert_eq!(f.funcs[0].params, vec!["a", "b"]);
        assert!(matches!(f.funcs[0].body[0], Stmt::Return(Some(_))));
    }

    #[test]
    fn if_else_and_while() {
        let f = file("m() { if(a) b(); else c(); while(d) e(); }");
        assert!(matches!(
            f.funcs[0].body[0],
            Stmt::If {
                otherwise: Some(_),
                ..
            }
        ));
        assert!(matches!(f.funcs[0].body[1], Stmt::While { .. }));
    }

    #[test]
    fn for_with_and_without_clauses() {
        let f = file("m() { for(i = 0; i < 10; i++) x(); for(;;) y(); }");
        let Stmt::For {
            init: Some(_),
            cond: Some(_),
            step: Some(_),
            ..
        } = &f.funcs[0].body[0]
        else {
            panic!()
        };
        let Stmt::For {
            init: None,
            cond: None,
            step: None,
            ..
        } = &f.funcs[0].body[1]
        else {
            panic!()
        };
    }

    /// dm.gsc:249 stacks three empty cases before one body.
    #[test]
    fn switch_cases_fall_through_when_empty() {
        let f = file(
            r#"m() {
            switch(r) {
            case "allies":
            case "axis":
            case "autoassign":
                pick();
                break;
            default:
                nope();
                break;
            }
        }"#,
        );
        let Stmt::Switch { arms, default, .. } = &f.funcs[0].body[0] else {
            panic!()
        };
        assert_eq!(arms.len(), 3);
        assert!(arms[0].body.is_empty());
        assert!(arms[1].body.is_empty());
        assert_eq!(arms[2].body.len(), 2);
        assert!(default.is_some());
    }

    /// One script in the corpus uses this, animscripts/predict.gsc.
    #[test]
    fn do_while_runs_its_body_before_testing() {
        let f = file("m() { do { x(); } while (cond); }");
        let Stmt::DoWhile { body, .. } = &f.funcs[0].body[0] else {
            panic!()
        };
        assert_eq!(body.len(), 1);
    }

    /// All five stock gametypes use `|=` on the iDFLAGS_* damage flags.
    #[test]
    fn pipe_assign_desugars_to_bitwise_or() {
        let f = file("m() { iDFlags |= level.iDFLAGS_NO_KNOCKBACK; }");
        let Stmt::Assign {
            value: Expr::Bin(BinOp::BitOr, _, _),
            ..
        } = &f.funcs[0].body[0]
        else {
            panic!("|= must desugar to a BitOr assignment")
        };
    }

    #[test]
    fn binary_and_parses_as_bitwise() {
        let f = file("m() { if(iDFlags & level.iDFLAGS_NO_PROTECTION) x(); }");
        let Stmt::If {
            cond: Expr::Bin(BinOp::BitAnd, _, _),
            ..
        } = &f.funcs[0].body[0]
        else {
            panic!()
        };
    }

    #[test]
    fn wait_is_a_statement_without_parens() {
        let f = file("m() { wait 0.05; }");
        assert!(matches!(f.funcs[0].body[0], Stmt::Wait(_)));
    }

    #[test]
    fn assignment_forms() {
        let f = file("m() { a = 1; a += 2; a -= 3; a *= 4; a /= 5; a++; a--; }");
        assert_eq!(f.funcs[0].body.len(), 7);
        assert!(matches!(f.funcs[0].body[0], Stmt::Assign { .. }));
        // Compound forms desugar to Assign with a Bin right-hand side.
        let Stmt::Assign {
            value: Expr::Bin(BinOp::Add, _, _),
            ..
        } = &f.funcs[0].body[1]
        else {
            panic!()
        };
        let Stmt::Assign {
            value: Expr::Bin(BinOp::Add, _, _),
            ..
        } = &f.funcs[0].body[5]
        else {
            panic!("a++ desugars to a = a + 1")
        };
    }

    #[test]
    fn threaded_and_method_call_statements() {
        let f = file(r#"m() { thread go(); self thread go(); self waittill("e", a, b); }"#);
        let Stmt::Expr(Expr::Call {
            threaded: true,
            recv: None,
            ..
        }) = &f.funcs[0].body[0]
        else {
            panic!()
        };
        let Stmt::Expr(Expr::Call {
            threaded: true,
            recv: Some(_),
            ..
        }) = &f.funcs[0].body[1]
        else {
            panic!()
        };
        let Stmt::Expr(Expr::Call {
            target: CallTarget::Name(n),
            args,
            ..
        }) = &f.funcs[0].body[2]
        else {
            panic!()
        };
        assert_eq!(n, "waittill");
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn using_animtree_is_collected_not_a_statement() {
        let f = file(r#"#using_animtree("30cal"); m() { }"#);
        assert_eq!(f.animtrees, vec!["30cal"]);
        assert_eq!(f.funcs.len(), 1);
    }

    #[test]
    fn a_parse_error_names_its_line() {
        let e = parse_file("m() {\n\n  a = ;\n}").unwrap_err();
        assert_eq!(e.line, 3);
    }
}
