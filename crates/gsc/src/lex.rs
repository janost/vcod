//! Source text to tokens. The grammar this covers is in
//! docs/research/cod11-gsc-language.md.

#[derive(Clone, PartialEq, Debug)]
pub enum Tok {
    Ident(String),
    Int(i32),
    Float(f32),
    Str(String),
    Localized(String),
    Anim(String),
    UsingAnimtree(String),
    /// Bare `#animtree`, no parens: distinct from `UsingAnimtree`, which
    /// always carries a name.
    AnimtreeRef,
    If,
    Else,
    While,
    Do,
    For,
    Switch,
    Case,
    Default,
    Break,
    Continue,
    Return,
    Thread,
    Wait,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PipeAssign,
    PlusPlus,
    MinusMinus,
    EqEq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,
    AndAnd,
    OrOr,
    Not,
    Amp,
    Dot,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Backslash,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Eof,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Spanned {
    pub tok: Tok,
    pub line: u32,
}

#[derive(Debug)]
pub struct LexError {
    pub line: u32,
    pub msg: String,
}

pub fn lex(src: &str) -> Result<Vec<Spanned>, LexError> {
    Lexer {
        b: src.as_bytes(),
        i: 0,
        line: 1,
    }
    .run()
}

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
    line: u32,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_cont(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Whether the token before a `%` could be the end of an operand, which is
/// the one-token lookback that tells `i%count` (modulo) from `%anim_name`
/// (an animation reference). Both spellings occur; see `run`.
fn ends_an_expression(prev: Option<&Tok>) -> bool {
    matches!(
        prev,
        Some(Tok::Ident(_) | Tok::Int(_) | Tok::Float(_) | Tok::RParen | Tok::RBracket)
    )
}

/// `true` and `false` are the integers 1 and 0, and unlike every keyword
/// they are case-sensitive: `probe_bool` measured `TRUE` reading back as an
/// undefined local on retail while `true` read back as 1.
fn bool_literal(text: &str) -> Option<Tok> {
    match text {
        "true" => Some(Tok::Int(1)),
        "false" => Some(Tok::Int(0)),
        _ => None,
    }
}

// Case-insensitive: the engine matches keywords and identifiers alike
// without regard to case.
fn keyword(text: &str) -> Option<Tok> {
    Some(match text.to_ascii_lowercase().as_str() {
        "if" => Tok::If,
        "else" => Tok::Else,
        "while" => Tok::While,
        "do" => Tok::Do,
        "for" => Tok::For,
        "switch" => Tok::Switch,
        "case" => Tok::Case,
        "default" => Tok::Default,
        "break" => Tok::Break,
        "continue" => Tok::Continue,
        "return" => Tok::Return,
        "thread" => Tok::Thread,
        "wait" => Tok::Wait,
        _ => return None,
    })
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        self.b.get(self.i + off).copied()
    }

    fn err(&self, line: u32, msg: impl Into<String>) -> LexError {
        LexError {
            line,
            msg: msg.into(),
        }
    }

    fn expect(&mut self, c: u8, line: u32) -> Result<(), LexError> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(line, format!("expected '{}'", c as char)))
        }
    }

    // Whitespace, `//` and `/* */` comments, and `/# #/` developer blocks,
    // repeated until none remain. Developer blocks are dropped whole, as a
    // release engine drops them.
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match (self.peek(), self.peek_at(1)) {
                (Some(c), _) if c.is_ascii_whitespace() => {
                    if c == b'\n' {
                        self.line += 1;
                    }
                    self.i += 1;
                }
                (Some(b'/'), Some(b'/')) => {
                    self.i += 2;
                    while !matches!(self.peek(), None | Some(b'\n')) {
                        self.i += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    let open_line = self.line;
                    self.i += 2;
                    loop {
                        match self.peek() {
                            None => return Err(self.err(open_line, "unterminated block comment")),
                            Some(b'*') if self.peek_at(1) == Some(b'/') => {
                                self.i += 2;
                                break;
                            }
                            Some(b'\n') => {
                                self.line += 1;
                                self.i += 1;
                            }
                            Some(_) => self.i += 1,
                        }
                    }
                }
                (Some(b'/'), Some(b'#')) => {
                    let open_line = self.line;
                    self.i += 2;
                    loop {
                        match self.peek() {
                            None => return Err(self.err(open_line, "unterminated developer block")),
                            Some(b'#') if self.peek_at(1) == Some(b'/') => {
                                self.i += 2;
                                break;
                            }
                            Some(b'\n') => {
                                self.line += 1;
                                self.i += 1;
                            }
                            Some(_) => self.i += 1,
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    // Reads a `"`-delimited string starting at the opening quote, handling
    // `\\`, `\"` and `\n` escapes. Any other `\x` is not a defined escape
    // and keeps both bytes.
    fn read_string(&mut self) -> Result<String, LexError> {
        let open_line = self.line;
        self.i += 1; // opening quote
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => return Err(self.err(open_line, "unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return String::from_utf8(out)
                        .map_err(|_| self.err(open_line, "string is not valid utf-8"));
                }
                Some(b'\\') => {
                    self.i += 1;
                    match self.peek() {
                        None => return Err(self.err(open_line, "unterminated string")),
                        Some(b'\\') => {
                            out.push(b'\\');
                            self.i += 1;
                        }
                        Some(b'"') => {
                            out.push(b'"');
                            self.i += 1;
                        }
                        Some(b'n') => {
                            out.push(b'\n');
                            self.i += 1;
                        }
                        // Any other `\x` is not a defined escape: keep both
                        // bytes, so a diagnostic string quoting a script
                        // path like `\shared::` does not lose the `\`.
                        Some(other) => {
                            out.push(b'\\');
                            out.push(other);
                            self.i += 1;
                        }
                    }
                }
                Some(b'\n') => {
                    self.line += 1;
                    out.push(b'\n');
                    self.i += 1;
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn read_ident(&mut self) -> String {
        let start = self.i;
        self.i += 1;
        while matches!(self.peek(), Some(c) if is_ident_cont(c)) {
            self.i += 1;
        }
        String::from_utf8(self.b[start..self.i].to_vec()).expect("ident bytes are ascii")
    }

    // A digit, or `.` followed by a digit; no exponents, no hex, no sign.
    // Never fails: a literal past i32::MAX saturates there.
    fn read_number(&mut self) -> Tok {
        let start = self.i;
        let mut has_dot = false;
        if self.peek() == Some(b'.') {
            has_dot = true;
            self.i += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        if !has_dot
            && self.peek() == Some(b'.')
            && matches!(self.peek_at(1), Some(c) if c.is_ascii_digit())
        {
            has_dot = true;
            self.i += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).expect("digits are ascii");
        if has_dot {
            return Tok::Float(text.parse().expect("well-formed float literal"));
        }
        // A literal past i32::MAX (maps/carride.gsc:1606,
        // maps/redsquare.gsc:1547 both use one as an effectively-infinite
        // sentinel) saturates rather than erroring or widening to a float,
        // which is what retail does: `-2147483648` is `-2147483647` there,
        // magnitude clamped before the unary minus
        // (tests/fixtures/semantics/retail-captures.txt, `# probe_lexer`).
        Tok::Int(text.parse().unwrap_or(i32::MAX))
    }

    // `#using_animtree("name")` or bare `#animtree`; any other `#`
    // directive is an error.
    fn read_directive(&mut self) -> Result<Tok, LexError> {
        let line = self.line;
        self.i += 1; // '#'
        let word = if matches!(self.peek(), Some(c) if is_ident_start(c)) {
            self.read_ident()
        } else {
            return Err(self.err(line, "unexpected '#'"));
        };
        if word.eq_ignore_ascii_case("animtree") {
            return Ok(Tok::AnimtreeRef);
        }
        if !word.eq_ignore_ascii_case("using_animtree") {
            return Err(self.err(line, format!("unknown directive '#{word}'")));
        }
        self.skip_trivia()?;
        self.expect(b'(', line)?;
        self.skip_trivia()?;
        if self.peek() != Some(b'"') {
            return Err(self.err(self.line, "expected a string after #using_animtree("));
        }
        let name = self.read_string()?;
        self.skip_trivia()?;
        self.expect(b')', line)?;
        Ok(Tok::UsingAnimtree(name))
    }

    fn two_char_op(&mut self) -> Option<Tok> {
        let tok = match (self.peek()?, self.peek_at(1)) {
            (b':', Some(b':')) => Tok::ColonColon,
            (b'=', Some(b'=')) => Tok::EqEq,
            (b'!', Some(b'=')) => Tok::NotEq,
            (b'<', Some(b'=')) => Tok::Le,
            (b'>', Some(b'=')) => Tok::Ge,
            (b'&', Some(b'&')) => Tok::AndAnd,
            (b'|', Some(b'|')) => Tok::OrOr,
            (b'+', Some(b'=')) => Tok::PlusAssign,
            (b'-', Some(b'=')) => Tok::MinusAssign,
            (b'*', Some(b'=')) => Tok::StarAssign,
            (b'/', Some(b'=')) => Tok::SlashAssign,
            (b'|', Some(b'=')) => Tok::PipeAssign,
            (b'+', Some(b'+')) => Tok::PlusPlus,
            (b'-', Some(b'-')) => Tok::MinusMinus,
            _ => return None,
        };
        self.i += 2;
        Some(tok)
    }

    fn one_char_op(&mut self) -> Option<Tok> {
        let tok = match self.peek()? {
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'=' => Tok::Assign,
            b'<' => Tok::Lt,
            b'>' => Tok::Gt,
            b'!' => Tok::Not,
            b'&' => Tok::Amp,
            b'.' => Tok::Dot,
            b',' => Tok::Comma,
            b';' => Tok::Semi,
            b':' => Tok::Colon,
            b'\\' => Tok::Backslash,
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'{' => Tok::LBrace,
            b'}' => Tok::RBrace,
            b'[' => Tok::LBracket,
            b']' => Tok::RBracket,
            _ => return None,
        };
        self.i += 1;
        Some(tok)
    }

    fn run(mut self) -> Result<Vec<Spanned>, LexError> {
        let mut out: Vec<Spanned> = Vec::new();
        loop {
            self.skip_trivia()?;
            let line = self.line;
            let Some(c) = self.peek() else { break };

            let tok = if c == b'#' {
                self.read_directive()?
            } else if c == b'&' && self.peek_at(1) == Some(b'"') {
                self.i += 1;
                Tok::Localized(self.read_string()?)
            } else if c == b'%' {
                self.i += 1;
                // `%name` is an animation reference, but retail also accepts
                // the tight `i%count` spelling of modulo
                // (tests/fixtures/semantics/retail-captures.txt,
                // `# probe_lexer`), so a `%` that follows something able to
                // end an expression is the operator.
                if matches!(self.peek(), Some(c2) if is_ident_start(c2))
                    && !ends_an_expression(out.last().map(|s| &s.tok))
                {
                    Tok::Anim(self.read_ident())
                } else {
                    Tok::Percent
                }
            } else if c == b'"' {
                Tok::Str(self.read_string()?)
            } else if c.is_ascii_digit()
                || (c == b'.' && matches!(self.peek_at(1), Some(d) if d.is_ascii_digit()))
            {
                self.read_number()
            } else if is_ident_start(c) {
                let text = self.read_ident();
                match bool_literal(&text) {
                    Some(t) => t,
                    None => keyword(&text).unwrap_or(Tok::Ident(text)),
                }
            } else if let Some(t) = self.two_char_op() {
                t
            } else if let Some(t) = self.one_char_op() {
                t
            } else {
                return Err(self.err(line, format!("unexpected byte {c:#04x}")));
            };
            out.push(Spanned { tok, line });
        }
        out.push(Spanned {
            tok: Tok::Eof,
            line: self.line,
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|s| s.tok).collect()
    }

    #[test]
    fn literals_and_identifiers() {
        assert_eq!(
            toks(r#"main 42 1.5 "hi" ;"#),
            vec![
                Tok::Ident("main".into()),
                Tok::Int(42),
                Tok::Float(1.5),
                Tok::Str("hi".into()),
                Tok::Semi,
                Tok::Eof,
            ]
        );
    }

    /// `true` and `false` are the integers 1 and 0, and only in lowercase:
    /// `probe_bool` measured `TRUE` reading back undefined on retail, which
    /// is what an unassigned local reads as.
    #[test]
    fn bool_literals_are_ints_and_case_sensitive() {
        assert_eq!(toks("true false"), vec![Tok::Int(1), Tok::Int(0), Tok::Eof]);
        assert_eq!(
            toks("TRUE False"),
            vec![
                Tok::Ident("TRUE".to_string()),
                Tok::Ident("False".to_string()),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn keywords_are_their_own_tokens() {
        assert_eq!(
            toks("if else while do for switch case default break continue return thread wait"),
            vec![
                Tok::If,
                Tok::Else,
                Tok::While,
                Tok::Do,
                Tok::For,
                Tok::Switch,
                Tok::Case,
                Tok::Default,
                Tok::Break,
                Tok::Continue,
                Tok::Return,
                Tok::Thread,
                Tok::Wait,
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn comments_and_developer_blocks_are_skipped() {
        // A /# #/ block is developer-only; a release engine drops it whole.
        assert_eq!(
            toks("a // line\n /* block */ b /# dev(); #/ c"),
            vec![
                Tok::Ident("a".into()),
                Tok::Ident("b".into()),
                Tok::Ident("c".into()),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn string_escapes() {
        assert_eq!(toks(r#""a\\b""#), vec![Tok::Str("a\\b".into()), Tok::Eof]);
        assert_eq!(toks(r#""a\"b""#), vec![Tok::Str("a\"b".into()), Tok::Eof]);
        assert_eq!(toks(r#""a\nb""#), vec![Tok::Str("a\nb".into()), Tok::Eof]);
    }

    // Diagnostic strings quote script paths like `\shared::`; an escape
    // outside {\\, \", \n} is not defined, so both bytes are kept rather
    // than silently dropping the backslash.
    #[test]
    fn unrecognized_escapes_keep_the_backslash() {
        assert_eq!(toks(r#""a\sb""#), vec![Tok::Str("a\\sb".into()), Tok::Eof]);
    }

    #[test]
    fn sigils_and_paths() {
        assert_eq!(
            toks(r#"&"OBJ_KEY" %pb_stand_alert maps\mp\_load::main"#),
            vec![
                Tok::Localized("OBJ_KEY".into()),
                Tok::Anim("pb_stand_alert".into()),
                Tok::Ident("maps".into()),
                Tok::Backslash,
                Tok::Ident("mp".into()),
                Tok::Backslash,
                Tok::Ident("_load".into()),
                Tok::ColonColon,
                Tok::Ident("main".into()),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn brackets_are_never_glued() {
        // a[b[c]] must not lex a `]]` token, so `[` and `]` stay single.
        assert_eq!(
            toks("a[b[c]]"),
            vec![
                Tok::Ident("a".into()),
                Tok::LBracket,
                Tok::Ident("b".into()),
                Tok::LBracket,
                Tok::Ident("c".into()),
                Tok::RBracket,
                Tok::RBracket,
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn every_operator() {
        assert_eq!(
            toks("+ - * / % = += -= *= /= |= ++ -- == != < > <= >= && || & ! . , ( ) { }"),
            vec![
                Tok::Plus,
                Tok::Minus,
                Tok::Star,
                Tok::Slash,
                Tok::Percent,
                Tok::Assign,
                Tok::PlusAssign,
                Tok::MinusAssign,
                Tok::StarAssign,
                Tok::SlashAssign,
                Tok::PipeAssign,
                Tok::PlusPlus,
                Tok::MinusMinus,
                Tok::EqEq,
                Tok::NotEq,
                Tok::Lt,
                Tok::Gt,
                Tok::Le,
                Tok::Ge,
                Tok::AndAnd,
                Tok::OrOr,
                Tok::Amp,
                Tok::Not,
                Tok::Dot,
                Tok::Comma,
                Tok::LParen,
                Tok::RParen,
                Tok::LBrace,
                Tok::RBrace,
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn using_animtree_directive() {
        assert_eq!(
            toks(r#"#using_animtree("30cal");"#),
            vec![Tok::UsingAnimtree("30cal".into()), Tok::Semi, Tok::Eof]
        );
    }

    #[test]
    fn an_unterminated_string_names_its_line() {
        let e = lex("a\nb\n\"oops").unwrap_err();
        assert_eq!(e.line, 3);
    }

    // Distinct from `#using_animtree("name")`: no parens, and it appears in
    // expression position, e.g. `self UseAnimTree(#animtree);`.
    #[test]
    fn bare_animtree_directive_is_its_own_token() {
        assert_eq!(toks("#animtree"), vec![Tok::AnimtreeRef, Tok::Eof]);
    }

    // A literal past i32::MAX must not panic the lexer, and two stock
    // scripts genuinely ship one as an effectively-infinite sentinel value
    // (maps/carride.gsc:1606, maps/redsquare.gsc:1547). It saturates at
    // i32::MAX, which is how retail reaches -2147483647 for `-2147483648`
    // (tests/fixtures/semantics/retail-captures.txt, `# probe_lexer`).
    #[test]
    fn an_oversized_int_literal_saturates_at_i32_max() {
        assert_eq!(toks("99999999999"), vec![Tok::Int(i32::MAX), Tok::Eof]);
        assert_eq!(toks("2147483648"), vec![Tok::Int(i32::MAX), Tok::Eof]);
        // The anti-panic property is not scoped to one digit width past
        // i32::MAX; pin it against a pathological input too.
        let t = toks(&"9".repeat(100));
        assert_eq!(t, vec![Tok::Int(i32::MAX), Tok::Eof]);
    }

    /// Retail accepts the tight `i%count` spelling of modulo, so `%` before
    /// an identifier is an animation reference only where an operand cannot
    /// already have ended (tests/fixtures/semantics/retail-captures.txt,
    /// `# probe_lexer`).
    #[test]
    fn a_tight_modulo_lexes_as_modulo_and_an_anim_reference_still_lexes() {
        assert_eq!(
            toks("i%count"),
            vec![
                Tok::Ident("i".into()),
                Tok::Percent,
                Tok::Ident("count".into()),
                Tok::Eof,
            ]
        );
        assert_eq!(toks("i%count"), toks("i % count"));
        // After `)` and `]` too, both of which end an operand.
        assert_eq!(toks("(a)%b")[3], Tok::Percent);
        assert_eq!(toks("a[0]%b")[4], Tok::Percent);
        assert_eq!(toks("7%count")[1], Tok::Percent);
        // Anywhere an operand cannot have ended, `%name` is still an anim.
        assert_eq!(
            toks("f(%pb_stand_alert)")[2],
            Tok::Anim("pb_stand_alert".into())
        );
        assert_eq!(
            toks("x = %pb_stand_alert")[2],
            Tok::Anim("pb_stand_alert".into())
        );
        assert_eq!(toks("f(a, %pb_run)")[4], Tok::Anim("pb_run".into()));
    }
}
