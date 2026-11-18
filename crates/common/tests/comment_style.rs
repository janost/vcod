//! Dashes used as punctuation in `//` comments under `crates/`.
//!
//! The house style writes a colon, a comma, a full stop or parentheses where
//! prose would reach for a dash, so a `//` comment may not hold an em dash
//! (U+2014) or its ASCII stand-in, a spaced `--`. A comment line that quotes
//! a `cargo` command keeps its `-- <args>`. String literals are skipped: the
//! window titles the player sees keep their em dash.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `crates/`, sorted, skipping build output.
fn rust_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().unwrap().to_string_lossy();
            if path.is_dir() {
                if name != "target" && !name.starts_with('.') {
                    walk(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    assert!(
        files.len() > 100,
        "only {} .rs files under {}",
        files.len(),
        root.display()
    );
    files
}

/// Each `//` comment in `src` as (one-based line, text from the `//` to the
/// line's end). Skips strings, raw strings, char literals and block comments,
/// so a `//` inside a literal is not taken for a comment.
fn line_comments(src: &str) -> Vec<(usize, String)> {
    let s: Vec<char> = src.chars().collect();
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let (mut i, mut line, mut out) = (0, 1, Vec::new());
    // Advances `i` past `s[i]`, counting lines.
    let step = |i: &mut usize, line: &mut usize| {
        if s[*i] == '\n' {
            *line += 1;
        }
        *i += 1;
    };
    while i < s.len() {
        let next = s.get(i + 1).copied();
        match s[i] {
            '/' if next == Some('/') => {
                let end = s[i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(s.len(), |n| i + n);
                out.push((line, s[i..end].iter().collect()));
                i = end;
            }
            '/' if next == Some('*') => {
                let mut depth = 0;
                loop {
                    if s[i] == '/' && s.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if s[i] == '*' && s.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        step(&mut i, &mut line);
                    }
                }
            }
            'r' if (i == 0
                || !ident(s[i - 1])
                || (s[i - 1] == 'b' && (i < 2 || !ident(s[i - 2]))))
                && {
                    let hashes = s[i + 1..].iter().take_while(|&&c| c == '#').count();
                    s.get(i + 1 + hashes) == Some(&'"')
                } =>
            {
                let hashes = s[i + 1..].iter().take_while(|&&c| c == '#').count();
                i += hashes + 2;
                while !(s[i] == '"'
                    && s[i + 1..]
                        .iter()
                        .take(hashes)
                        .filter(|&&c| c == '#')
                        .count()
                        == hashes)
                {
                    step(&mut i, &mut line);
                }
                i += hashes + 1;
            }
            '"' => {
                i += 1;
                while s[i] != '"' {
                    if s[i] == '\\' {
                        step(&mut i, &mut line);
                    }
                    step(&mut i, &mut line);
                }
                i += 1;
            }
            // A char literal; a lifetime has no closing quote two chars on.
            '\'' if next == Some('\\') => {
                i += 3;
                while s[i] != '\'' {
                    i += 1;
                }
                i += 1;
            }
            '\'' if s.get(i + 2) == Some(&'\'') => i += 3,
            _ => step(&mut i, &mut line),
        }
    }
    out
}

/// Whether a `//` comment's text uses a dash as punctuation.
fn dashed(comment: &str) -> bool {
    let body = comment.trim_start_matches(['/', '!']).trim_end();
    let spaced =
        body.contains(" -- ") || body.ends_with(" --") || body.trim_start().starts_with("-- ");
    body.contains('\u{2014}') || (spaced && !body.contains("cargo "))
}

#[test]
fn no_comment_uses_a_dash_as_punctuation() {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut hits = Vec::new();
    for path in rust_files() {
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        for (line, text) in line_comments(&src) {
            if dashed(&text) {
                let rel = path
                    .strip_prefix(&crates)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                hits.push(format!("crates/{rel}:{line}: {}", text.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "{} comment lines use a dash as punctuation; write a colon, comma, full stop or parentheses:\n{}",
        hits.len(),
        hits.join("\n")
    );
}

#[test]
fn the_check_fires_on_the_shapes_it_claims_to_catch() {
    let src = "let a = \"x -- y // z \u{2014} w\";\n\
               let b = r#\"// a -- b\"#;\n\
               let c = '\"'; // fine: a colon\n\
               /* x -- y */ fn f<'a>(_: &'a str) {}\n\
               // one -- two\n\
               /// three \u{2014} four\n\
               //! ends on a dash --\n\
               // `cargo test -p vcod-common -- comment_style`\n";
    let found: Vec<usize> = line_comments(src)
        .into_iter()
        .filter(|(_, t)| dashed(t))
        .map(|(l, _)| l)
        .collect();
    assert_eq!(found, [5, 6, 7]);
}
