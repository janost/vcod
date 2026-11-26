//! `SV_MapRotate_f`'s walk over `sv_mapRotationCurrent` (cod_lnxded
//! 0x80840c8, cod2_lnxded 0x808c132; cod11-map-cycle.md 5,
//! cod2-map-cycle.md 1.3). Both engines run the same loop and print the
//! same strings; the caller applies the gametype and the ending. UO's
//! adds three words (coduo-diff.md 26.2.1).

/// How the walk reads a word off `sv_mapRotationCurrent`: retail's
/// `NextToken` (cod_lnxded 0x8084014, cod2_lnxded 0x808c0d0), one
/// `Com_Parse` in its default space-delimited mode with the rest written
/// back from where the word ended, delimiter and all (cod11-map-cycle.md
/// 5.1, cod2-map-cycle.md 1.3). The two engines' parsers differ only in a
/// quoted word's escapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Words {
    /// 1.1d, 1.5b and UO (`Com_ParseExt` cod_lnxded 0x808170c): `\"` in
    /// quotes reads as `"`.
    Cod1,
    /// CoD2 (cod2_lnxded 0x80b67f8): `\"` and `\\` both.
    Cod2,
}

/// Which words the walk knows besides `gametype` and `map`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keywords {
    /// 1.1d, 1.5b, CoD2 and CoD4: none.
    Base,
    /// UO 1.51b (coduo_lnxded 0x8088690..0x8088aea): `exec`,
    /// `allow_jeeps` and `allow_tanks`, each taking one word.
    Uo,
}

/// One of UO's three words, in walk order.
#[derive(Debug, PartialEq, Eq)]
pub enum UoWord {
    /// `exec <file>`: `Cmd_ExecuteString("exec <file>")` at once, and the
    /// walk's `map` then queues as `map <m> noautoexec`.
    Exec(String),
    /// `allow_jeeps` or `allow_tanks`: a forced `Cvar_Set` of this cvar.
    Set(&'static str, String),
}

/// `Com_Parse`'s token buffer holds 1023 characters.
const MAX_TOKEN: usize = 0x3ff;

impl Words {
    /// `NextToken`: the word, and `cur` set to what follows it; `None`, and
    /// `cur` emptied, when only blanks and comments were left.
    fn next(self, cur: &mut String) -> Option<String> {
        let (tok, rest) = match self.parse(cur) {
            Some((tok, rest)) => (Some(tok), rest.to_string()),
            None => (None, String::new()),
        };
        *cur = rest;
        tok
    }

    /// One `Com_Parse`: blanks (any byte up to a space, and bytes from 0x80,
    /// which the signed `char` compare reads as negative) and `//` and
    /// `/* */` comments skipped, then a quoted string up to its closing
    /// quote or a run of non-blanks. The rest starts past a closing quote,
    /// else at the blank that ended the word.
    fn parse(self, s: &str) -> Option<(String, &str)> {
        let s = s.split('\0').next().unwrap_or("");
        let b = s.as_bytes();
        let blank = |c: u8| !(b'!'..0x80).contains(&c);
        let mut i = 0;
        loop {
            while i < b.len() && blank(b[i]) {
                i += 1;
            }
            match (b.get(i), b.get(i + 1)) {
                (None, _) => return None,
                (Some(b'/'), Some(b'/')) => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                }
                // Both engines look for the `*/` from the `*` that opened
                // it, so `/*/` is a whole comment.
                (Some(b'/'), Some(b'*')) => {
                    i = match s[i + 1..].find("*/") {
                        Some(at) => i + 1 + at + 2,
                        None => b.len(),
                    };
                }
                _ => break,
            }
        }
        let mut tok = Vec::new();
        let mut push = |c: u8| {
            if tok.len() < MAX_TOKEN {
                tok.push(c);
            }
        };
        let rest = if b[i] == b'"' {
            let mut j = i + 1;
            loop {
                let Some(&c) = b.get(j) else {
                    // Retail's rest starts past the terminator here, in
                    // whatever the string's memory holds next.
                    break "";
                };
                let next = b.get(j + 1).copied();
                if c == b'\\'
                    && (next == Some(b'"') || (self == Words::Cod2 && next == Some(b'\\')))
                {
                    push(next.unwrap_or(0));
                    j += 2;
                } else if c == b'"' {
                    break &s[j + 1..];
                } else {
                    push(c);
                    j += 1;
                }
            }
        } else {
            let mut j = i;
            while j < b.len() && !blank(b[j]) {
                push(b[j]);
                j += 1;
            }
            &s[j..]
        };
        Some((String::from_utf8_lossy(&tok).into_owned(), rest))
    }
}

/// How a walk ended.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// `map <m>`: the engine runs `map m`.
    Map(String),
    /// No word left: `No map specified in sv_mapRotation`.
    NoMap,
    /// `gametype` with nothing after it.
    NoGametype,
    /// `map` with nothing after it.
    NoMapAfterMap,
    /// UO: `exec`, `allow_jeeps` or `allow_tanks` with nothing after it;
    /// the line it prints.
    NoValue(&'static str),
}

impl End {
    /// The line retail prints before its `map_restart`; `None` for a map.
    pub fn warning(&self) -> Option<&'static str> {
        match self {
            End::Map(_) => None,
            End::NoMap => Some("No map specified in sv_mapRotation - forcing map_restart."),
            End::NoGametype => Some(
                "No gametype specified after 'gametype' keyword in sv_mapRotation - forcing map_restart.",
            ),
            End::NoMapAfterMap => Some(
                "No map specified after 'map' keyword in sv_mapRotation - forcing map_restart.",
            ),
            End::NoValue(line) => Some(line),
        }
    }
}

/// One `map_rotate`'s walk.
#[derive(Debug, PartialEq, Eq)]
pub struct Walk {
    /// The last `gametype` read, which retail has written to `g_gametype`
    /// by the end.
    pub gametype: Option<String>,
    /// The words skipped as `Unknown keyword '%s' in sv_mapRotation.`, in
    /// order; they print before the ending's line.
    pub unknown: Vec<String>,
    /// UO's words, in order; always empty under [`Keywords::Base`].
    pub uo: Vec<UoWord>,
    pub end: End,
}

impl Walk {
    /// UO: an `exec` anywhere in the walk queues its map as `map <m>
    /// noautoexec` (0x8088a73).
    pub fn noautoexec(&self) -> bool {
        self.uo.iter().any(|w| matches!(w, UoWord::Exec(_)))
    }
}

/// `SV_MapRotate_f`'s loop: `current` refilled from `rotation` when it is
/// the empty string, and again when its first word comes back none; `gametype <g>`
/// sets the gametype and reads on, `map <m>` ends the walk, any other word
/// is skipped, unless `keywords` knows it. `current` keeps what the walk
/// left.
pub fn rotate(rotation: &str, current: &mut String, words: Words, keywords: Keywords) -> Walk {
    if current.is_empty() {
        *current = rotation.to_string();
    }
    let mut word = words.next(current);
    if word.is_none() {
        *current = rotation.to_string();
        word = words.next(current);
    }
    let mut walk = Walk {
        gametype: None,
        unknown: Vec::new(),
        uo: Vec::new(),
        end: End::NoMap,
    };
    while let Some(w) = word {
        if w.eq_ignore_ascii_case("gametype") {
            match words.next(current) {
                Some(g) => walk.gametype = Some(g),
                None => {
                    walk.end = End::NoGametype;
                    return walk;
                }
            }
        } else if w.eq_ignore_ascii_case("map") {
            walk.end = match words.next(current) {
                Some(m) => End::Map(m),
                None => End::NoMapAfterMap,
            };
            return walk;
        } else if let Some((_, cvar, missing)) = UO_WORDS
            .into_iter()
            .find(|(k, ..)| keywords == Keywords::Uo && w.eq_ignore_ascii_case(k))
        {
            let Some(v) = words.next(current) else {
                walk.end = End::NoValue(missing);
                return walk;
            };
            walk.uo.push(match cvar {
                Some(cvar) => UoWord::Set(cvar, v),
                None => UoWord::Exec(v),
            });
        } else {
            walk.unknown.push(w);
        }
        word = words.next(current);
    }
    walk
}

/// UO's words (`strcasecmp`): the keyword, the cvar it sets (`None` for
/// `exec`) and the line a missing value prints (0x80e36bb..0x80e3857).
const UO_WORDS: [(&str, Option<&str>, &str); 3] = [
    (
        "exec",
        None,
        "No value specified after 'exec' keyword in sv_mapRotation - forcing map_restart.",
    ),
    (
        "allow_jeeps",
        Some("scr_allow_jeeps"),
        "No value specified after 'allow_jeeps' keyword in sv_mapRotation - forcing map_restart.",
    ),
    (
        "allow_tanks",
        Some("scr_allow_tanks"),
        "No value specified after 'allow_tanks' keyword in sv_mapRotation - forcing map_restart.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn next(words: Words, s: &str) -> (Option<String>, String) {
        let mut cur = s.to_string();
        (words.next(&mut cur), cur)
    }

    fn word(w: &str, rest: &str) -> (Option<String>, String) {
        (Some(w.to_string()), rest.to_string())
    }

    /// cod11-map-cycle.md 5.1, cod2-map-cycle.md 1.3: the rest starts at the
    /// blank that ended the word (retail printed `" map mp_brecourt"`), and
    /// blanks or comments alone read as no word and empty the cursor.
    #[test]
    fn a_word_leaves_its_delimiter_in_the_rest() {
        for w in [Words::Cod1, Words::Cod2] {
            assert_eq!(
                next(w, "map mp_harbor map x"),
                word("map", " mp_harbor map x")
            );
            assert_eq!(next(w, "  \tmap\n x"), word("map", "\n x"));
            assert_eq!(next(w, "mp_harbor"), word("mp_harbor", ""));
            assert_eq!(next(w, "mp_harbor  "), word("mp_harbor", "  "));
            assert_eq!(next(w, "   "), (None, String::new()));
            assert_eq!(next(w, ""), (None, String::new()));
            assert_eq!(next(w, "// map x\nmap y"), word("map", " y"));
            assert_eq!(next(w, "/*/ map */ dm"), word("map", " */ dm"));
            assert_eq!(next(w, "/* open"), (None, String::new()));
            // Bytes from 0x80 are blanks to the signed compare.
            assert_eq!(next(w, "\u{a0}dm"), word("dm", ""));
        }
    }

    /// A quoted word runs to its closing quote and the rest starts past it;
    /// `""` is a word. 1.1 unescapes `\"` only, CoD2 `\\` too.
    #[test]
    fn a_quoted_word_ends_past_its_quote() {
        for w in [Words::Cod1, Words::Cod2] {
            assert_eq!(next(w, "\"mp a\"b c"), word("mp a", "b c"));
            assert_eq!(next(w, " \"\" map"), word("", " map"));
            assert_eq!(next(w, "\"a\\\"b\""), word("a\"b", ""));
            assert_eq!(next(w, "\"open"), word("open", ""));
        }
        assert_eq!(next(Words::Cod1, "\"a\\\\b\""), word("a\\\\b", ""));
        assert_eq!(next(Words::Cod2, "\"a\\\\b\""), word("a\\b", ""));
    }

    /// Retail's two-map walk (both engines, live): the cursor keeps the
    /// blank before the next `map`, an empty `""` map name is a map, and a
    /// cursor of blanks only is not refilled up front but reads no word and
    /// is refilled then.
    #[test]
    fn the_walk_keeps_retail_s_cursor() {
        for w in [Words::Cod1, Words::Cod2] {
            let rot = "gametype dm map mp_harbor map mp_brecourt";
            let mut cur = String::new();
            let walk = rotate(rot, &mut cur, w, Keywords::Base);
            assert_eq!(walk.end, End::Map("mp_harbor".into()));
            assert_eq!(walk.gametype.as_deref(), Some("dm"));
            assert_eq!(cur, " map mp_brecourt");
            assert_eq!(
                rotate(rot, &mut cur, w, Keywords::Base).end,
                End::Map("mp_brecourt".into())
            );
            assert_eq!(cur, "");
            let mut cur = "   ".to_string();
            assert_eq!(
                rotate(rot, &mut cur, w, Keywords::Base).end,
                End::Map("mp_harbor".into())
            );
            let mut cur = String::new();
            assert_eq!(
                rotate("map \"\" x", &mut cur, w, Keywords::Base).end,
                End::Map(String::new())
            );
            assert_eq!(cur, " x");
            let mut cur = String::new();
            let walk = rotate("\"\" map x", &mut cur, w, Keywords::Base);
            assert_eq!(walk.unknown, vec![String::new()]);
        }
    }

    #[test]
    fn the_three_endings() {
        let end = |rot: &str| rotate(rot, &mut String::new(), Words::Cod2, Keywords::Base).end;
        assert_eq!(end(""), End::NoMap);
        assert_eq!(end("bogus"), End::NoMap);
        assert_eq!(end("gametype"), End::NoGametype);
        assert_eq!(end("gametype tdm map"), End::NoMapAfterMap);
    }

    fn uo(rot: &str, cur: &mut String) -> Walk {
        rotate(rot, cur, Words::Cod1, Keywords::Uo)
    }

    /// coduo-diff.md 26.2.1, the live walk: `exec` takes one word, any
    /// `exec` queues the map as `noautoexec`, a missing value is a restart.
    #[test]
    fn uo_exec_takes_a_word_and_marks_the_map() {
        let mut cur = String::new();
        let rot = "EXEC rot_a exec rot_b map mp_arnhem map mp_foy exec";
        let walk = uo(rot, &mut cur);
        assert_eq!(
            walk.uo,
            vec![UoWord::Exec("rot_a".into()), UoWord::Exec("rot_b".into())]
        );
        assert!(walk.noautoexec());
        assert_eq!(walk.end, End::Map("mp_arnhem".into()));
        let walk = uo(rot, &mut cur);
        assert!(!walk.noautoexec());
        assert_eq!(walk.end, End::Map("mp_foy".into()));
        let walk = uo(rot, &mut cur);
        assert_eq!(
            walk.end.warning(),
            Some(
                "No value specified after 'exec' keyword in sv_mapRotation - forcing map_restart."
            )
        );
    }

    #[test]
    fn uo_allow_jeeps_sets_scr_allow_jeeps() {
        let walk = uo("allow_jeeps 0 Allow_Jeeps \"\" map x", &mut String::new());
        assert_eq!(
            walk.uo,
            vec![
                UoWord::Set("scr_allow_jeeps", "0".into()),
                UoWord::Set("scr_allow_jeeps", String::new())
            ]
        );
        assert!(!walk.noautoexec());
        let end = uo("allow_jeeps", &mut String::new()).end;
        assert_eq!(
            end.warning(),
            Some(
                "No value specified after 'allow_jeeps' keyword in sv_mapRotation - forcing map_restart."
            )
        );
    }

    #[test]
    fn uo_allow_tanks_sets_scr_allow_tanks() {
        let walk = uo("allow_tanks 0 gametype sd map x", &mut String::new());
        assert_eq!(walk.uo, vec![UoWord::Set("scr_allow_tanks", "0".into())]);
        assert_eq!(walk.gametype.as_deref(), Some("sd"));
        let end = uo("allow_tanks", &mut String::new()).end;
        assert_eq!(
            end.warning(),
            Some(
                "No value specified after 'allow_tanks' keyword in sv_mapRotation - forcing map_restart."
            )
        );
    }

    /// 1.1d and 1.5b have none of UO's words (1.5b live: each word and
    /// value an `Unknown keyword`); neither have CoD2 and CoD4.
    #[test]
    fn base_keywords_skip_uo_s_words() {
        let walk = rotate(
            "exec rot_a allow_jeeps 0 allow_tanks 0 map mp_harbor",
            &mut String::new(),
            Words::Cod1,
            Keywords::Base,
        );
        assert_eq!(
            walk.unknown,
            ["exec", "rot_a", "allow_jeeps", "0", "allow_tanks", "0"]
        );
        assert!(walk.uo.is_empty());
        assert_eq!(walk.end, End::Map("mp_harbor".into()));
    }
}
