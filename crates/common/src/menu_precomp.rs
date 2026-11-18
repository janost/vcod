//! The menu files' precompiler. CoDMP.exe links Q3's botlib precompiler
//! (its `l_precomp` error strings, cod11-front-end.md 24): `#include`,
//! object- and function-like `#define` with `#` and `##`, `#undef`, and
//! `#if`/`#ifdef`/`#ifndef`/`#elif`/`#else`/`#endif`. [`run`] expands a file's
//! text before `menu::tokenize` reads it.

use std::collections::HashMap;

/// Nested `#include`s and macro expansions stop here, so a cycle ends.
const MAX_DEPTH: u32 = 16;

/// The expanded text stops growing here: stock menus expand to tens of KB,
/// while nested defines grow a small file exponentially.
const MAX_OUTPUT: usize = 4 << 20;

/// `#include`s one file may run, its includes' counted.
const MAX_INCLUDES: u32 = 256;

/// How deep an `#if` expression may nest parentheses, `?:` and prefix
/// operators before it reads 0.
const MAX_EXPR_DEPTH: u32 = 64;

#[derive(Clone)]
enum Macro {
    Object(String),
    Function { params: Vec<String>, body: String },
}

/// One open `#if`: whether its enclosing branch reads, whether this branch
/// reads, whether any branch so far did, and whether `#else` was seen.
struct Cond {
    parent: bool,
    active: bool,
    taken: bool,
    seen_else: bool,
}

struct Pre<'a> {
    include: &'a dyn Fn(&str) -> Option<String>,
    macros: HashMap<String, Macro>,
    conds: Vec<Cond>,
    includes: u32,
    /// Set once the output reached [`MAX_OUTPUT`] and the warning went out.
    full: bool,
}

/// `text` with its directives carried out and every macro expanded.
/// `include` reads an `#include`d file. Defines last to the end of the file,
/// its includes' included, as one botlib source's do.
pub(crate) fn run(text: &str, include: &dyn Fn(&str) -> Option<String>) -> String {
    let mut pre = Pre {
        include,
        macros: HashMap::new(),
        conds: Vec::new(),
        includes: 0,
        full: false,
    };
    let mut out = String::with_capacity(text.len());
    pre.scan(text, 0, true, &mut out);
    if !pre.conds.is_empty() {
        log::warn!("ui: missing #endif");
    }
    out
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The end of the identifier starting at `i`.
fn ident_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_ident(b[i]) {
        i += 1;
    }
    i
}

/// The index past the string opened by the quote at `i` (its closing quote
/// included, or the end): the tokenizer reads no escapes.
fn string_end(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < b.len() && b[j] != b'"' {
        j += 1;
    }
    (j + 1).min(b.len())
}

/// The index past a comment starting at `i`, or `None` when none does.
/// A line comment ends before its newline.
fn comment_end(b: &[u8], i: usize) -> Option<usize> {
    match (b[i], b.get(i + 1)) {
        (b'/', Some(b'/')) => {
            let mut j = i;
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        (b'/', Some(b'*')) => {
            let mut j = i + 2;
            while j + 1 < b.len() && !(b[j] == b'*' && b[j + 1] == b'/') {
                j += 1;
            }
            Some((j + 2).min(b.len()))
        }
        _ => None,
    }
}

impl Pre<'_> {
    fn active(&self) -> bool {
        self.conds.last().is_none_or(|c| c.active)
    }

    /// Copies `text` to `out`, expanding macros, and with `directives` runs
    /// a `#` outside a string or comment as a directive to the end of its
    /// line, as botlib reads a `#` token anywhere. An inactive `#if` branch
    /// writes nothing but its newlines.
    fn scan(&mut self, text: &str, depth: u32, directives: bool, out: &mut String) {
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if out.len() > MAX_OUTPUT {
                if !self.full {
                    log::warn!("ui: expansion passed {MAX_OUTPUT} bytes; the rest is dropped");
                    self.full = true;
                }
                return;
            }
            let c = b[i];
            if directives && c == b'#' {
                let end = directive_end(b, i + 1);
                let line = strip_comments(&text[i + 1..end]).replace("\\\n", " ");
                self.directive(&line, depth, out);
                i = end;
                continue;
            }
            let active = self.active();
            let next = if c == b'"' {
                string_end(b, i)
            } else if let Some(end) = comment_end(b, i) {
                end
            } else if is_ident_start(c) && (i == 0 || !(is_ident(b[i - 1]) || b[i - 1] == b'.')) {
                let end = ident_end(b, i);
                if active {
                    let mut at = end;
                    self.expand_ident(&text[i..end], text, &mut at, depth, out);
                    i = at;
                    continue;
                }
                end
            } else {
                // Step a whole UTF-8 character.
                let mut j = i + 1;
                while j < b.len() && (b[j] & 0xc0) == 0x80 {
                    j += 1;
                }
                j
            };
            if active {
                out.push_str(&text[i..next]);
            } else if c == b'\n' {
                out.push('\n');
            }
            i = next;
        }
    }

    /// The macro `name` names expanded into `out`, its arguments read from
    /// `src` at `*at` and `*at` moved past them; `name` itself when it names
    /// none, or a function-like one with no `(` next.
    fn expand_ident(
        &mut self,
        name: &str,
        src: &str,
        at: &mut usize,
        depth: u32,
        out: &mut String,
    ) {
        let Some(m) = self.macros.get(name).cloned() else {
            out.push_str(name);
            return;
        };
        if depth >= MAX_DEPTH {
            log::warn!("ui: define {name} nests too deep");
            out.push_str(name);
            return;
        }
        let body = match m {
            Macro::Object(body) => body,
            Macro::Function { params, body } => {
                let Some((args, end)) = read_args(src, *at) else {
                    log::warn!("ui: define {name} missing parms");
                    out.push_str(name);
                    return;
                };
                *at = end;
                let args = if params.is_empty() && args.len() == 1 && args[0].is_empty() {
                    Vec::new()
                } else {
                    args
                };
                if args.len() != params.len() {
                    log::warn!(
                        "ui: define {name} takes {} parms, given {}",
                        params.len(),
                        args.len()
                    );
                }
                substitute(&params, &args, &body)
            }
        };
        // The expansion is read again, so the macros it names expand too.
        self.scan(&body, depth + 1, false, out);
    }

    /// One directive line, the `#` stripped.
    fn directive(&mut self, line: &str, depth: u32, out: &mut String) {
        let line = line.trim();
        let b = line.as_bytes();
        let word_end = ident_end(b, 0);
        let (word, rest) = (&line[..word_end], line[word_end..].trim());
        match word {
            "if" | "ifdef" | "ifndef" => {
                let parent = self.active();
                let value = parent
                    && match word {
                        "ifdef" => self.macros.contains_key(first_ident(rest)),
                        "ifndef" => !self.macros.contains_key(first_ident(rest)),
                        _ => self.eval(rest, depth),
                    };
                self.conds.push(Cond {
                    parent,
                    active: value,
                    taken: value,
                    seen_else: false,
                });
            }
            "elif" => {
                let (parent, taken) = match self.conds.last() {
                    Some(c) if !c.seen_else => (c.parent, c.taken),
                    _ => {
                        log::warn!("ui: misplaced #elif");
                        return;
                    }
                };
                let value = parent && !taken && self.eval(rest, depth);
                let c = self.conds.last_mut().expect("checked above");
                c.active = value;
                c.taken |= value;
            }
            "else" => match self.conds.last_mut() {
                Some(c) if !c.seen_else => {
                    c.active = c.parent && !c.taken;
                    c.taken = true;
                    c.seen_else = true;
                }
                _ => log::warn!("ui: misplaced #else"),
            },
            "endif" => {
                if self.conds.pop().is_none() {
                    log::warn!("ui: misplaced #endif");
                }
            }
            _ if !self.active() => {}
            "include" => {
                let path = rest.trim_matches(|c| matches!(c, '"' | '<' | '>'));
                if self.includes >= MAX_INCLUDES {
                    log::warn!("ui: more than {MAX_INCLUDES} includes; skipping {path}");
                    return;
                }
                self.includes += 1;
                match (self.include)(path) {
                    Some(inc) if depth < 8 => self.scan(&inc, depth + 1, true, out),
                    _ => log::warn!("ui: cannot include {path}"),
                }
            }
            "define" => self.define(rest),
            "undef" => {
                self.macros.remove(first_ident(rest));
            }
            // `#pragma`, `#line` and `#error` read nothing here.
            _ => {}
        }
    }

    /// `#define NAME body` or `#define NAME(a, b) body`: parameters only
    /// when the `(` touches the name. A body's own name is dropped from it
    /// ("recursive define (removed recursion)"), and a second define of a
    /// name replaces the first.
    fn define(&mut self, rest: &str) {
        let b = rest.as_bytes();
        if b.first().is_none_or(|&c| !is_ident_start(c)) {
            log::warn!("ui: #define without name");
            return;
        }
        let name_end = ident_end(b, 0);
        let name = &rest[..name_end];
        let (params, body) = if b.get(name_end) == Some(&b'(') {
            let Some(close) = rest[name_end..].find(')') else {
                log::warn!("ui: define parameters not terminated");
                return;
            };
            let inner = &rest[name_end + 1..name_end + close];
            let params: Vec<String> = if inner.trim().is_empty() {
                Vec::new()
            } else {
                inner.split(',').map(|p| p.trim().to_string()).collect()
            };
            (Some(params), &rest[name_end + close + 1..])
        } else {
            (None, &rest[name_end..])
        };
        let body = drop_ident(body.trim(), name);
        let m = match params {
            Some(params) => Macro::Function { params, body },
            None => Macro::Object(body),
        };
        self.macros.insert(name.to_string(), m);
    }

    /// An `#if`/`#elif` expression: `defined NAME` and `defined(NAME)` read
    /// 1 or 0, macros expand, a name left over reads 0.
    fn eval(&mut self, expr: &str, depth: u32) -> bool {
        let b = expr.as_bytes();
        let mut resolved = String::new();
        let mut i = 0;
        while i < b.len() {
            if is_ident_start(b[i]) && (i == 0 || !is_ident(b[i - 1])) {
                let end = ident_end(b, i);
                if &expr[i..end] == "defined" {
                    let mut j = end;
                    while j < b.len() && (b[j] == b' ' || b[j] == b'(') {
                        j += 1;
                    }
                    let name_end = ident_end(b, j);
                    let known = self.macros.contains_key(&expr[j..name_end]);
                    resolved.push_str(if known { " 1 " } else { " 0 " });
                    i = name_end;
                    while i < b.len() && (b[i] == b' ' || b[i] == b')') {
                        i += 1;
                    }
                    continue;
                }
                let mut at = end;
                self.expand_ident(&expr[i..end], expr, &mut at, depth, &mut resolved);
                i = at;
                continue;
            }
            resolved.push(b[i] as char);
            i += 1;
        }
        let mut p = Expr {
            b: resolved.as_bytes(),
            i: 0,
            depth: 0,
        };
        let v = p.ternary();
        p.skip();
        if p.i < p.b.len() {
            log::warn!("ui: syntax error in #if/#elif: {expr}");
        }
        v != 0
    }
}

/// The end of a directive line from `i`: the next newline, a backslash
/// before it carrying the line on (botlib's `PC_ReadLine`), a block
/// comment read through as space.
fn directive_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'\n' {
        if b[i] == b'\\' && b.get(i + 1) == Some(&b'\n') {
            i += 2;
            continue;
        }
        if b[i] == b'"' {
            i = string_end(b, i);
            continue;
        }
        if let Some(end) = comment_end(b, i) {
            i = end;
            continue;
        }
        i += 1;
    }
    i
}

/// A directive line with its comments read as space, as botlib's lexer
/// skips them: a define's body ends before a trailing `//`.
fn strip_comments(line: &str) -> String {
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let end = string_end(b, i);
            out.push_str(&line[i..end]);
            i = end;
        } else if let Some(end) = comment_end(b, i) {
            out.push(' ');
            i = end;
        } else {
            let start = i;
            i += 1;
            while i < b.len() && (b[i] & 0xc0) == 0x80 {
                i += 1;
            }
            out.push_str(&line[start..i]);
        }
    }
    out
}

fn first_ident(s: &str) -> &str {
    &s[..ident_end(s.as_bytes(), 0)]
}

/// `body` without the identifier `name` outside strings.
fn drop_ident(body: &str, name: &str) -> String {
    let b = body.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let end = string_end(b, i);
            out.push_str(&body[i..end]);
            i = end;
        } else if is_ident_start(b[i]) && (i == 0 || !is_ident(b[i - 1])) {
            let end = ident_end(b, i);
            if &body[i..end] != name {
                out.push_str(&body[i..end]);
            }
            i = end;
        } else {
            let start = i;
            i += 1;
            while i < b.len() && (b[i] & 0xc0) == 0x80 {
                i += 1;
            }
            out.push_str(&body[start..i]);
        }
    }
    out
}

/// A call's arguments from `at`, past whitespace and newlines to its `(`:
/// split at commas outside nested parentheses and strings, each trimmed,
/// with the index past the closing `)`. `None` without a `(` or a `)`.
fn read_args(src: &str, at: usize) -> Option<(Vec<String>, usize)> {
    let b = src.as_bytes();
    let mut i = at;
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    if b.get(i) != Some(&b'(') {
        return None;
    }
    i += 1;
    let mut args = Vec::new();
    let mut start = i;
    let mut nest = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i = string_end(b, i);
                continue;
            }
            b'(' => nest += 1,
            b')' if nest > 0 => nest -= 1,
            b')' => {
                args.push(src[start..i].trim().to_string());
                return Some((args, i + 1));
            }
            b',' if nest == 0 => {
                args.push(src[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[derive(PartialEq)]
enum Piece {
    Space,
    Tok(String),
    Param(usize),
    Hash,
    Paste,
}

/// A function-like body with its parameters replaced by `args`: `#param`
/// is the argument's tokens joined with no space between them, quoted
/// (`PC_StringizeTokens`), and `a ## b` joins the tokens either side.
fn substitute(params: &[String], args: &[String], body: &str) -> String {
    let arg = |k: usize| args.get(k).map_or("", String::as_str);
    let b = body.as_bytes();
    let mut pieces = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            pieces.push(Piece::Space);
        } else if c == b'#' && b.get(i + 1) == Some(&b'#') {
            pieces.push(Piece::Paste);
            i += 2;
        } else if c == b'#' {
            pieces.push(Piece::Hash);
            i += 1;
        } else if c == b'"' {
            let end = string_end(b, i);
            pieces.push(Piece::Tok(body[i..end].to_string()));
            i = end;
        } else if is_ident_start(c) {
            let end = ident_end(b, i);
            let name = &body[i..end];
            pieces.push(match params.iter().position(|p| p == name) {
                Some(k) => Piece::Param(k),
                None => Piece::Tok(name.to_string()),
            });
            i = end;
        } else {
            let start = i;
            i += 1;
            while i < b.len() && (b[i] & 0xc0) == 0x80 {
                i += 1;
            }
            pieces.push(Piece::Tok(body[start..i].to_string()));
        }
    }
    // `#` before a parameter: its argument stringized; each other
    // parameter its argument.
    let mut joined: Vec<Piece> = Vec::new();
    let mut k = 0;
    while k < pieces.len() {
        match pieces[k] {
            Piece::Hash => {
                let mut n = k + 1;
                while pieces.get(n) == Some(&Piece::Space) {
                    n += 1;
                }
                if let Some(&Piece::Param(p)) = pieces.get(n) {
                    joined.push(Piece::Tok(format!("\"{}\"", stringize(arg(p)))));
                    k = n + 1;
                    continue;
                }
                log::warn!("ui: stringizing operator without define parameter");
                joined.push(Piece::Tok("#".into()));
            }
            Piece::Param(p) => joined.push(Piece::Tok(arg(p).to_string())),
            _ => joined.push(std::mem::replace(&mut pieces[k], Piece::Space)),
        }
        k += 1;
    }
    // `##`: the tokens either side become one.
    let mut out: Vec<Piece> = Vec::new();
    let mut paste = false;
    for p in joined {
        match p {
            Piece::Paste => {
                while out.last() == Some(&Piece::Space) {
                    out.pop();
                }
                paste = true;
            }
            Piece::Space if paste => {}
            Piece::Tok(t) if paste => {
                match out.last_mut() {
                    Some(Piece::Tok(prev)) => prev.push_str(&t),
                    _ => out.push(Piece::Tok(t)),
                }
                paste = false;
            }
            p => out.push(p),
        }
    }
    let mut s = String::new();
    for p in out {
        match p {
            Piece::Tok(t) => s.push_str(&t),
            _ => s.push(' '),
        }
    }
    s
}

/// An argument's tokens with the whitespace between them gone; quoted
/// strings keep theirs.
fn stringize(arg: &str) -> String {
    let b = arg.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let end = string_end(b, i);
            out.push_str(&arg[i..end]);
            i = end;
        } else if b[i].is_ascii_whitespace() {
            i += 1;
        } else {
            let start = i;
            i += 1;
            while i < b.len() && (b[i] & 0xc0) == 0x80 {
                i += 1;
            }
            out.push_str(&arg[start..i]);
        }
    }
    out
}

/// `#if`'s integer expression, C's precedence, `?:` included.
struct Expr<'a> {
    b: &'a [u8],
    i: usize,
    /// The nesting [`Expr::nest`] counts, against [`MAX_EXPR_DEPTH`].
    depth: u32,
}

impl Expr<'_> {
    fn skip(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, op: &str) -> bool {
        self.skip();
        let o = op.as_bytes();
        if self.b[self.i..].starts_with(o) {
            // `<` is not `<=` or `<<`, `&` not `&&`, `|` not `||`.
            let next = self.b.get(self.i + o.len()).copied();
            let longer = match op {
                "<" => matches!(next, Some(b'=' | b'<')),
                ">" => matches!(next, Some(b'=' | b'>')),
                "&" => next == Some(b'&'),
                "|" => next == Some(b'|'),
                "!" => next == Some(b'='),
                _ => false,
            };
            if !longer {
                self.i += o.len();
                return true;
            }
        }
        false
    }

    /// `f` one nesting level down; past [`MAX_EXPR_DEPTH`] the rest of the
    /// expression is dropped and reads 0.
    fn nest(&mut self, f: impl FnOnce(&mut Self) -> i64) -> i64 {
        if self.depth >= MAX_EXPR_DEPTH {
            if self.i < self.b.len() {
                log::warn!("ui: #if/#elif nests past {MAX_EXPR_DEPTH} levels");
                self.i = self.b.len();
            }
            return 0;
        }
        self.depth += 1;
        let v = f(self);
        self.depth -= 1;
        v
    }

    fn ternary(&mut self) -> i64 {
        self.nest(Self::ternary_at)
    }

    fn ternary_at(&mut self) -> i64 {
        let c = self.binary(0);
        if self.eat("?") {
            let a = self.ternary();
            if !self.eat(":") {
                log::warn!("ui: ? without : in #if/#elif");
            }
            let b = self.ternary();
            return if c != 0 { a } else { b };
        }
        c
    }

    fn binary(&mut self, level: usize) -> i64 {
        const LEVELS: &[&[&str]] = &[
            &["||"],
            &["&&"],
            &["|"],
            &["^"],
            &["&"],
            &["==", "!="],
            &["<=", ">=", "<", ">"],
            &["<<", ">>"],
            &["+", "-"],
            &["*", "/", "%"],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut v = self.binary(level + 1);
        'more: loop {
            for op in LEVELS[level] {
                if self.eat(op) {
                    let r = self.binary(level + 1);
                    v = match *op {
                        "||" => i64::from(v != 0 || r != 0),
                        "&&" => i64::from(v != 0 && r != 0),
                        "|" => v | r,
                        "^" => v ^ r,
                        "&" => v & r,
                        "==" => i64::from(v == r),
                        "!=" => i64::from(v != r),
                        "<=" => i64::from(v <= r),
                        ">=" => i64::from(v >= r),
                        "<" => i64::from(v < r),
                        ">" => i64::from(v > r),
                        "<<" => v.wrapping_shl(r as u32),
                        ">>" => v.wrapping_shr(r as u32),
                        "+" => v.wrapping_add(r),
                        "-" => v.wrapping_sub(r),
                        "*" => v.wrapping_mul(r),
                        _ if r == 0 => {
                            log::warn!("ui: divide by zero in #if/#elif");
                            0
                        }
                        "/" => v.wrapping_div(r),
                        _ => v.wrapping_rem(r),
                    };
                    continue 'more;
                }
            }
            return v;
        }
    }

    fn unary(&mut self) -> i64 {
        if self.eat("!") {
            return i64::from(self.nest(Self::unary) == 0);
        }
        if self.eat("~") {
            return !self.nest(Self::unary);
        }
        if self.eat("-") {
            return self.nest(Self::unary).wrapping_neg();
        }
        if self.eat("+") {
            return self.nest(Self::unary);
        }
        if self.eat("(") {
            let v = self.ternary();
            if !self.eat(")") {
                log::warn!("ui: too many ( in #if/#elif");
            }
            return v;
        }
        self.skip();
        let start = self.i;
        while self.i < self.b.len()
            && (self.b[self.i].is_ascii_alphanumeric() || self.b[self.i] == b'.')
        {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.b[start..self.i]).unwrap_or("");
        if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            return i64::from_str_radix(hex, 16).unwrap_or(0);
        }
        t.parse::<i64>()
            .or_else(|_| t.parse::<f64>().map(|f| f as i64))
            .unwrap_or_else(|_| {
                log::warn!("ui: can't evaluate {t:?} in #if/#elif");
                0
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre(text: &str) -> String {
        run(text, &|_| None)
    }

    fn words(text: &str) -> Vec<String> {
        crate::menu::tokenize(&pre(text))
    }

    #[test]
    fn an_object_define_expands_after_its_line() {
        assert_eq!(
            words("X\n#define X .98 .96 .39 1\nfocusColor X"),
            ["X", "focusColor", ".98", ".96", ".39", "1"]
        );
        assert_eq!(words("#define T \"a b\"\ntext T"), ["text", "a b"]);
        // Bodies name further defines, which expand in turn.
        assert_eq!(words("#define A B\n#define B 2\nrect A"), ["rect", "2"]);
    }

    #[test]
    fn a_function_define_takes_its_arguments() {
        let text = "#define BUTTON(y, label, act) itemDef { rect 10 y 100 20 text label action { act } }\n\
                    BUTTON(190, \"@MENU_JOIN_GAME\", open joinserver ; close main)";
        assert_eq!(
            words(text),
            [
                "itemDef",
                "{",
                "rect",
                "10",
                "190",
                "100",
                "20",
                "text",
                "@MENU_JOIN_GAME",
                "action",
                "{",
                "open",
                "joinserver",
                ";",
                "close",
                "main",
                "}",
                "}"
            ]
        );
        // Arguments run over lines and nest parentheses.
        assert_eq!(words("#define F(a) x a\nF(\n(1, 2)\n)"), ["x", "(1,", "2)"]);
        // A space before the `(` makes it the body.
        assert_eq!(words("#define G (a) b\nG"), ["(a)", "b"]);
        // With no `(` after it the name stays.
        assert_eq!(words("#define H(a) a\nH ;"), ["H", ";"]);
    }

    #[test]
    fn stringize_and_paste() {
        assert_eq!(words("#define S(a) #a\nS(x  y)"), ["xy"]);
        assert_eq!(words("#define P(a, b) a ## b\nP(ui_, net)"), ["ui_net"]);
        assert_eq!(
            words("#define C(n) name menu_ ## n text #n\nC(main)"),
            ["name", "menu_main", "text", "main"]
        );
    }

    #[test]
    fn undef_and_recursion() {
        assert_eq!(words("#define X 1\n#undef X\nX"), ["X"]);
        // A body's own name is dropped from it.
        assert_eq!(words("#define X X 2\nX"), ["2"]);
        // A second define replaces the first.
        assert_eq!(words("#define X 1\n#define X 2\nX"), ["2"]);
    }

    #[test]
    fn conditionals_pick_one_branch() {
        let text = "#define PC\n#ifdef PC\na\n#else\nb\n#endif\n#ifndef PC\nc\n#endif\n\
                    #if defined(PC) && 2 * 3 == 6\nd\n#elif 1\ne\n#else\nf\n#endif\n\
                    #if 0\n#define Z 1\n#if 1\ng\n#endif\n#else\nh\n#endif\nZ";
        assert_eq!(words(text), ["a", "d", "h", "Z"]);
        assert_eq!(
            words("#define V 3\n#if V > 2 ? 0 : 1\nx\n#endif"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn strings_and_comments_stay_whole() {
        assert_eq!(
            words("#define X 1\ntext \"X #define\" // X\n/* X\n#define Y */ X"),
            ["text", "X #define", "1"]
        );
        // A continued directive line.
        assert_eq!(words("#define L a \\\n b\nL"), ["a", "b"]);
        // A body ends before its comment.
        assert_eq!(
            words("#define A 3 // right edge\n#define B 2 /* c */\nrect A B x"),
            ["rect", "3", "2", "x"]
        );
    }

    /// Fifteen levels of an eight-wide define would expand one use of the
    /// top to 8^15 copies; the output stops at the cap.
    #[test]
    fn nested_wide_defines_stop_at_the_output_cap() {
        let mut text = String::from("#define L0 abcdefgh\n");
        for n in 1..16 {
            let prev = format!("L{} ", n - 1).repeat(8);
            text.push_str(&format!("#define L{n} {prev}\n"));
        }
        text.push_str("L15 L15");
        let start = std::time::Instant::now();
        let out = pre(&text);
        assert!(out.len() <= MAX_OUTPUT + 64, "{}", out.len());
        assert!(out.len() > MAX_OUTPUT);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    /// A file that includes itself on every line stops at the include cap.
    #[test]
    fn self_includes_stop_at_the_include_cap() {
        let body = "#include \"self.h\"\n".repeat(100);
        let reads = std::cell::Cell::new(0u32);
        let inc = |_: &str| {
            reads.set(reads.get() + 1);
            Some(body.clone())
        };
        run(&body, &inc);
        assert_eq!(reads.get(), MAX_INCLUDES);
    }

    /// 100,000 `(`, or as many `!`, read 0 past the depth cap instead of
    /// recursing once each.
    #[test]
    fn a_deep_if_expression_reads_zero() {
        for open in ["(", "!", "-"] {
            let text = format!("#if {}1\nx\n#endif\ny", open.repeat(100_000));
            assert_eq!(words(&text), ["y"], "{open}");
        }
        let ok = format!("#if {}1{}\nx\n#endif", "(".repeat(30), ")".repeat(30));
        assert_eq!(words(&ok), ["x"]);
    }

    #[test]
    fn include_shares_the_defines() {
        let inc = |p: &str| (p == "defs.h").then(|| "#define W 640\n".to_string());
        let out = run("#include \"defs.h\"\nrect 0 0 W 480", &inc);
        assert_eq!(
            crate::menu::tokenize(&out),
            ["rect", "0", "0", "640", "480"]
        );
    }
}
