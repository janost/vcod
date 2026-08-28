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
                    self.expect(&Tok::LParen)?;
                    let args = self.args_list()?;
                    base = Expr::Call {
                        recv: Some(Box::new(base)),
                        target: CallTarget::Name(name),
                        args,
                        threaded: true,
                    };
                }
                Tok::Ident(name) => {
                    self.bump();
                    self.expect(&Tok::LParen)?;
                    let args = self.args_list()?;
                    base = Expr::Call {
                        recv: Some(Box::new(base)),
                        target: CallTarget::Name(name),
                        args,
                        threaded: false,
                    };
                }
                _ => break,
            }
        }
        Ok(base)
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
        if self.eat(&Tok::LParen) {
            let args = self.args_list()?;
            return Ok(Expr::Call {
                recv: None,
                target: CallTarget::Deref(Box::new(inner)),
                args,
                threaded: false,
            });
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
        if *self.peek() == Tok::ColonColon {
            self.bump();
            let name = self.ident_name()?;
            let file = path.join("/").to_ascii_lowercase();
            if self.eat(&Tok::LParen) {
                let args = self.args_list()?;
                return Ok(Expr::Call {
                    recv: None,
                    target: CallTarget::Path { file, name },
                    args,
                    threaded: false,
                });
            }
            return Ok(Expr::FuncRef { file, name });
        }

        if self.eat(&Tok::LParen) {
            let args = self.args_list()?;
            return Ok(Expr::Call {
                recv: None,
                target: CallTarget::Name(first),
                args,
                threaded: false,
            });
        }
        Ok(Expr::Local(first))
    }
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

    #[test]
    fn empty_array_literal() {
        assert!(matches!(expr("[]"), Expr::EmptyArray));
    }

    #[test]
    fn unary_minus_and_not() {
        assert!(matches!(expr("-x"), Expr::Un(UnOp::Neg, _)));
        assert!(matches!(expr("!x"), Expr::Un(UnOp::Not, _)));
    }
}
