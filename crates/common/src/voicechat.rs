//! CoD 1.1 MP quick-chat scripts (`mp/allies_chat.voice`, `mp/axis_chat.voice`),
//! the data behind the `j`/`k`/`l` server commands. Format and addresses:
//! docs/research/cod11-quick-chat.md. No stock install ships these files;
//! mods may.

/// Retail aborts on an unknown gender token; vcod accepts any as
/// [`Gender::Other`] (a permissive superset, flagged in the research doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub alias: String,
    pub head_icon: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceChat {
    pub gender: Gender,
    pub categories: Vec<Category>,
}

pub const MAX_CATEGORIES: usize = 64;
pub const MAX_VARIANTS: usize = 64;

impl VoiceChat {
    /// Case-insensitive, like retail's category matcher.
    pub fn category(&self, name: &str) -> Option<&Category> {
        self.categories
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

/// Whitespace-separated tokens with Q3-style `//` and `/* */` comments and
/// double-quoted strings; retail shares its Com_Parse, whose comment and
/// quote behaviour is inferred from that lineage. Tokens carry the line they
/// start on: the optional head-icon slot is read with
/// `GetToken(allowLineBreaks = 0)` in retail, so it only ever comes from the
/// alias's own line.
fn tokenize(text: &str) -> Vec<(usize, &str)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' => i += 1,
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'\r' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            b'"' => {
                let start_line = line;
                i += 1;
                let start = i;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                out.push((start_line, &text[start..i]));
                i += 1;
            }
            _ => {
                let start_line = line;
                let start = i;
                while i < b.len()
                    && !matches!(b[i], b' ' | b'\t' | b'\n')
                    && !(b[i] == b'/' && matches!(b.get(i + 1), Some(b'/' | b'*')))
                {
                    i += 1;
                }
                out.push((start_line, &text[start..i]));
            }
        }
    }
    out
}

struct Toks<'a> {
    t: Vec<(usize, &'a str)>,
    i: usize,
}

impl<'a> Toks<'a> {
    fn next(&mut self) -> Option<(usize, &'a str)> {
        let s = self.t.get(self.i).copied();
        self.i += usize::from(s.is_some());
        s
    }

    /// Same-line peek without consuming, the `GetToken(0)` slot.
    fn same_line(&self, line: usize) -> Option<(usize, &'a str)> {
        match self.t.get(self.i) {
            Some(&(l, s)) if l == line => Some((l, s)),
            _ => None,
        }
    }
}

/// Grammar per the research doc: gender line, then any number of
/// `<category> { <alias> [<headicon>] ... }` blocks. The head icon shares
/// the alias's line. EOF mid-category is accepted, as retail accepts it.
pub fn parse(text: &str) -> Result<VoiceChat, String> {
    let mut t = Toks {
        t: tokenize(text),
        i: 0,
    };
    let gender = match t.next() {
        None => return Err("empty voice chat file".into()),
        // The shared matcher folds case like the category test does.
        Some((_, g)) if g.eq_ignore_ascii_case("male") => Gender::Male,
        Some((_, g)) if g.eq_ignore_ascii_case("female") => Gender::Female,
        Some(_) => Gender::Other,
    };

    let mut categories = Vec::new();
    while categories.len() < MAX_CATEGORIES {
        let Some((_, name)) = t.next() else {
            break;
        };
        match t.next() {
            Some((_, "{")) => {}
            other => {
                return Err(format!(
                    "expected {{ found {:?} after category {name:?}",
                    other.map(|(_, s)| s)
                ));
            }
        }
        let mut cat = Category {
            name: name.to_string(),
            variants: Vec::new(),
        };
        while let Some((alias_line, alias)) = t.next() {
            if alias.eq_ignore_ascii_case("}") {
                break;
            }
            let mut variant = Variant {
                alias: alias.to_string(),
                head_icon: None,
            };
            // A "}" in the icon slot is pushed back by retail's UngetToken
            // and ends the category on the next pass.
            if let Some((_, icon)) = t.same_line(alias_line) {
                if !icon.eq_ignore_ascii_case("}") {
                    variant.head_icon = Some(icon.to_string());
                    t.i += 1;
                }
            }
            if cat.variants.len() < MAX_VARIANTS {
                cat.variants.push(variant);
            }
        }
        categories.push(cat);
    }
    Ok(VoiceChat { gender, categories })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_file() {
        let v = parse("male\ncat\n{\nsay_yes\n}\n").unwrap();
        assert_eq!(v.gender, Gender::Male);
        assert_eq!(v.categories.len(), 1);
        assert_eq!(v.categories[0].name, "cat");
        assert_eq!(v.categories[0].variants.len(), 1);
        assert_eq!(v.categories[0].variants[0].head_icon, None);
    }

    #[test]
    fn gender_forms_map() {
        assert_eq!(parse("male").unwrap().gender, Gender::Male);
        assert_eq!(parse("FEMALE").unwrap().gender, Gender::Female);
        assert_eq!(parse("neutral").unwrap().gender, Gender::Other);
        assert!(parse("").is_err());
    }

    #[test]
    fn head_icon_is_optional_and_brace_stays_unconsumed() {
        let v = parse("male\ncat\n{\nsnd icon\nsnd2\nsnd3\n}\n").unwrap();
        let cat = v.category("CAT").unwrap();
        assert_eq!(cat.variants[0].alias, "snd");
        assert_eq!(cat.variants[0].head_icon.as_deref(), Some("icon"));
        assert_eq!(cat.variants[1].alias, "snd2");
        assert_eq!(cat.variants[1].head_icon, None);
        assert_eq!(cat.variants[2].alias, "snd3");
        // The category ended at the right brace, not at snd3.
        assert_eq!(cat.variants.len(), 3);
        assert_eq!(v.categories.len(), 1);
    }

    #[test]
    fn multiple_categories_with_case_insensitive_lookup() {
        let src = "male ammo { snd_a } health { snd_b1 snd_b2 }\n";
        let v = parse(src).unwrap();
        assert_eq!(v.categories.len(), 2);
        assert_eq!(v.category("HEALTH").unwrap().variants[0].alias, "snd_b1");
        assert!(v.category("grenades").is_none());
    }

    #[test]
    fn quoted_alias_keeps_spaces() {
        let v = parse("male cat { \"two words\" }").unwrap();
        assert_eq!(v.categories[0].variants[0].alias, "two words");
    }

    #[test]
    fn comments_are_skipped() {
        let src = "male // line\n/* block\ncat */ cat { snd // tail\n}\n";
        let v = parse(src).unwrap();
        assert_eq!(v.categories[0].name, "cat");
        assert_eq!(v.categories[0].variants[0].alias, "snd");
    }

    #[test]
    fn extra_variants_are_capped_at_64_but_consumed() {
        let mut src = String::from("male cat {\n");
        for n in 0..70 {
            src.push_str(&format!("s{n}\n"));
        }
        src.push_str("}\n");
        let v = parse(&src).unwrap();
        assert_eq!(v.categories[0].variants.len(), MAX_VARIANTS);
    }

    #[test]
    fn eof_mid_category_is_accepted_like_retail() {
        // Same line: snd_b is snd_a's head icon; the file just ends after.
        let v = parse("male cat { snd_a snd_b").unwrap();
        assert_eq!(v.categories[0].variants.len(), 1);
        assert_eq!(
            v.categories[0].variants[0].head_icon.as_deref(),
            Some("snd_b")
        );
    }

    #[test]
    fn missing_opening_brace_errors() {
        let e = parse("male cat snd_no_brace").unwrap_err();
        assert!(e.contains("expected {"), "{e}");
    }
}
