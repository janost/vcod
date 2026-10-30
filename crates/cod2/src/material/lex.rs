//! The tokenizer the `.techset`, `.tech` and `.sm` grammars share: words,
//! numbers, quoted strings (quotes remembered, a techset label is one) and
//! punctuation, `==` and `&&` as one token; `//` and `/* */` comments skipped.

use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Word,
    Quoted,
    Punct,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Tok<'a> {
    pub kind: Kind,
    pub text: &'a str,
    pub line: u32,
}

pub(crate) struct Lexer<'a> {
    toks: Vec<Tok<'a>>,
    pos: usize,
}

fn is_word_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Result<Self> {
        let b = src.as_bytes();
        let mut toks = Vec::new();
        let mut i = 0;
        let mut line = 1u32;
        while i < b.len() {
            let c = b[i];
            if c == b'\n' {
                line += 1;
                i += 1;
            } else if c.is_ascii_whitespace() {
                i += 1;
            } else if b[i..].starts_with(b"//") {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            } else if b[i..].starts_with(b"/*") {
                let Some(end) = src[i + 2..].find("*/") else {
                    bail!("line {line}: unterminated comment");
                };
                line += src[i..i + 2 + end].matches('\n').count() as u32;
                i += end + 4;
            } else if c == b'"' {
                let Some(end) = src[i + 1..].find('"') else {
                    bail!("line {line}: unterminated string");
                };
                toks.push(Tok {
                    kind: Kind::Quoted,
                    text: &src[i + 1..i + 1 + end],
                    line,
                });
                line += src[i + 1..i + 1 + end].matches('\n').count() as u32;
                i += end + 2;
            } else if is_word_start(c)
                || c.is_ascii_digit()
                || (c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
            {
                let start = i;
                i += 1;
                if is_word_start(c) {
                    while i < b.len() && is_word(b[i]) {
                        i += 1;
                    }
                } else {
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                        i += 1;
                    }
                }
                toks.push(Tok {
                    kind: Kind::Word,
                    text: &src[start..i],
                    line,
                });
            } else {
                let n = if b[i..].starts_with(b"==") || b[i..].starts_with(b"&&") {
                    2
                } else {
                    src[i..].chars().next().map_or(1, char::len_utf8)
                };
                toks.push(Tok {
                    kind: Kind::Punct,
                    text: &src[i..i + n],
                    line,
                });
                i += n;
            }
        }
        Ok(Self { toks, pos: 0 })
    }

    pub fn peek(&self) -> Option<Tok<'a>> {
        self.toks.get(self.pos).copied()
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    pub fn next(&mut self) -> Result<Tok<'a>> {
        let Some(t) = self.peek() else {
            bail!("unexpected end of file");
        };
        self.pos += 1;
        Ok(t)
    }

    /// The next token when its text is `text`, consumed.
    pub fn eat(&mut self, text: &str) -> bool {
        if self
            .peek()
            .is_some_and(|t| t.kind != Kind::Quoted && t.text == text)
        {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub fn expect(&mut self, text: &str) -> Result<()> {
        let t = self.next()?;
        if t.kind == Kind::Quoted || t.text != text {
            bail!("line {}: expected '{text}', found '{}'", t.line, t.text);
        }
        Ok(())
    }

    pub fn word(&mut self) -> Result<&'a str> {
        let t = self.next()?;
        if t.kind != Kind::Word {
            bail!("line {}: expected a name, found '{}'", t.line, t.text);
        }
        Ok(t.text)
    }

    pub fn quoted(&mut self) -> Result<&'a str> {
        let t = self.next()?;
        if t.kind != Kind::Quoted {
            bail!(
                "line {}: expected a quoted name, found '{}'",
                t.line,
                t.text
            );
        }
        Ok(t.text)
    }

    pub fn number<T: std::str::FromStr>(&mut self) -> Result<T> {
        let t = self.next()?;
        match t.text.parse() {
            Ok(v) if t.kind == Kind::Word => Ok(v),
            _ => bail!("line {}: expected a number, found '{}'", t.line, t.text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_punctuation_and_operators() {
        let lx = Lexer::new("a==b&&c: \"x y\"; // c\n/* d\n */ -1.5 v[0]").unwrap();
        let got: Vec<_> = lx.toks.iter().map(|t| t.text).collect();
        assert_eq!(
            got,
            [
                "a", "==", "b", "&&", "c", ":", "x y", ";", "-1.5", "v", "[", "0", "]"
            ]
        );
        assert_eq!(lx.toks[6].kind, Kind::Quoted);
        assert_eq!(lx.toks.last().unwrap().line, 3);
    }
}
