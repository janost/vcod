//! Every citation of a doc by section number, `<doc>.md 12.4` or
//! `<doc>.md, section 3, "A paragraph"`, in the code, the tools and the
//! docs resolves: the doc exists, each number is a heading's, and a quoted
//! name is a heading, a bold lead, a paragraph's lead or a `name:` label
//! inside that section (anywhere in the doc when no number is given).
//!
//! AGENTS.md keeps section numbers and file names stable because code
//! cites them; this is what catches a renumbered section or a renamed
//! paragraph. It cannot tell whether the cited section says what the
//! citation claims. Short forms (`combat doc 12.4`) are not checked. A
//! second test holds each doc to one heading per section number, so a
//! citation cannot name two sections.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Kinds of file whose comments and text are scanned for citations.
const SCANNED: [&str; 6] = ["rs", "md", "sh", "py", "toml", "java"];

/// The tracked docs: `docs/*.md` and the `research`, `dev` and `features`
/// trees. The other directories under `docs/` are gitignored or local.
fn docs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root.join("docs")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") && path.is_file() {
            out.push(path);
        }
    }
    for dir in ["research", "dev", "features"] {
        walk(&root.join("docs").join(dir), &mut out);
    }
    out.retain(|p| p.extension().is_some_and(|e| e == "md"));
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy();
            if name != "target" && !name.starts_with('.') {
                walk(&path, out);
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SCANNED.contains(&e))
        {
            out.push(path);
        }
    }
}

/// A section number: an optional capital, 1 to 3 digits and an optional
/// small letter, then `.N` parts the same; never followed by a word
/// character. Returns its length in bytes.
fn section_len(s: &[u8]) -> Option<usize> {
    let mut i = 0;
    if s.first().is_some_and(u8::is_ascii_uppercase) {
        i += 1;
    }
    let part = |i: &mut usize| -> bool {
        let start = *i;
        while s.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        if *i == start || *i - start > 3 {
            return false;
        }
        if s.get(*i).is_some_and(u8::is_ascii_lowercase) {
            *i += 1;
        }
        true
    };
    if !part(&mut i) {
        return None;
    }
    while s.get(i) == Some(&b'.') && s.get(i + 1).is_some_and(u8::is_ascii_digit) {
        let mut j = i + 1;
        if !part(&mut j) {
            return None;
        }
        i = j;
    }
    let word = |c: &u8| c.is_ascii_alphanumeric() || *c == b'_';
    (!s.get(i).is_some_and(word)).then_some(i)
}

/// A list of section numbers joined by `, `, ` and `, ` or `, `-` or `–`.
fn section_list(s: &str) -> (Vec<String>, usize) {
    let Some(n) = section_len(s.as_bytes()) else {
        return (Vec::new(), 0);
    };
    let mut ids = vec![s[..n].to_string()];
    let mut end = n;
    'more: loop {
        for sep in [", ", " and ", " or ", "-", "–"] {
            if let Some(rest) = s[end..].strip_prefix(sep)
                && let Some(m) = section_len(rest.as_bytes())
            {
                ids.push(rest[..m].to_string());
                end += sep.len() + m;
                continue 'more;
            }
        }
        return (ids, end);
    }
}

/// `,? "Name"` (straight or curly quotes, up to 80 characters).
fn quoted_name(s: &str) -> Option<String> {
    let s = s.strip_prefix(',').unwrap_or(s).strip_prefix(' ')?;
    let s = s.strip_prefix('"').or_else(|| s.strip_prefix('“'))?;
    let (end, _) = s
        .char_indices()
        .take(81)
        .find(|&(_, c)| c == '"' || c == '”')?;
    (end > 0).then(|| s[..end].to_string())
}

struct Citation {
    doc: String,
    sections: Vec<String>,
    name: Option<String>,
    at: usize,
}

/// The citations in `text`, a file's lines with comment markers stripped
/// and joined by spaces, so a citation wrapped over two comment lines reads
/// as one.
fn citations(text: &str) -> Vec<Citation> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    for (i, _) in text.match_indices(".md") {
        let after = i + 3;
        if bytes
            .get(after)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            continue;
        }
        let mut start = i;
        while start > 0 && {
            let c = bytes[start - 1];
            c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-')
        } {
            start -= 1;
        }
        while start < i && matches!(bytes[start], b'.' | b'-') {
            start += 1;
        }
        if start == i {
            continue;
        }
        let rest = &text[after..];
        let mut p = rest.strip_prefix(',').unwrap_or(rest);
        for word in [" sections", " section", " §", "§"] {
            if let Some(r) = p.strip_prefix(word) {
                p = r;
                break;
            }
        }
        p = p.strip_prefix(' ').unwrap_or(p);
        let (sections, len) = section_list(p);
        let tail = if sections.is_empty() { rest } else { &p[len..] };
        let name = quoted_name(tail);
        if sections.is_empty() && name.is_none() {
            continue;
        }
        out.push(Citation {
            doc: text[start..after].to_string(),
            sections,
            name,
            at: start,
        });
    }
    out
}

/// A doc's lines and its numbered sections: id to the line range the
/// heading opens, up to the next heading of its level or above.
struct Doc {
    lines: Vec<String>,
    headings: Vec<(usize, String)>,
    sections: HashMap<String, (usize, usize)>,
    /// Section numbers more than one heading opens with.
    repeated: Vec<String>,
}

fn index(text: &str) -> Doc {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut heads = Vec::new();
    let mut fence = false;
    for (i, l) in lines.iter().enumerate() {
        if l.starts_with("```") {
            fence = !fence;
            continue;
        }
        let level = l.bytes().take_while(|&c| c == b'#').count();
        if fence || !(1..=6).contains(&level) || !l[level..].starts_with([' ', '\t']) {
            continue;
        }
        heads.push((i, level, l[level..].trim().to_string()));
    }
    let mut sections = HashMap::new();
    let mut repeated = Vec::new();
    for (k, (i, level, title)) in heads.iter().enumerate() {
        let end = heads[k + 1..]
            .iter()
            .find(|h| h.1 <= *level)
            .map_or(lines.len(), |h| h.0);
        let token = title.split_whitespace().next().unwrap_or("");
        let id = token.strip_suffix('.').unwrap_or(token);
        if section_len(id.as_bytes()) == Some(id.len())
            && sections.insert(id.to_string(), (*i, end)).is_some()
        {
            repeated.push(id.to_string());
        }
    }
    let headings = heads.into_iter().map(|(i, _, t)| (i, t)).collect();
    Doc {
        lines,
        headings,
        sections,
        repeated,
    }
}

fn norm(s: &str) -> String {
    let s = s
        .replace('`', "")
        .replace(['“', '”'], "\"")
        .replace('’', "'")
        .to_lowercase();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A list marker (`-`, `*`, `+`, `1.`) and the space after it, if `l` opens
/// with one.
fn strip_marker(l: &str) -> Option<&str> {
    let t = l.trim_start();
    let rest = t
        .strip_prefix(['-', '*', '+'])
        .or_else(|| {
            t.trim_start_matches(|c: char| c.is_ascii_digit())
                .strip_prefix('.')
        })
        .filter(|_| !t.starts_with('.'))?;
    rest.starts_with([' ', '\t']).then(|| rest.trim_start())
}

/// Paragraphs and list items in `lines`, each joined into one line.
fn blocks(lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for l in lines {
        let blank = l.trim().is_empty();
        if blank || strip_marker(l).is_some() || l.starts_with('#') || l.starts_with('|') {
            if !cur.is_empty() {
                out.push(cur.join(" "));
            }
            cur.clear();
            if !blank {
                cur.push(l);
            }
        } else {
            cur.push(l);
        }
    }
    if !cur.is_empty() {
        out.push(cur.join(" "));
    }
    out
}

/// Whether `t` is `n`, or opens with `n` and goes on with one of `seps`.
fn opens_with(t: &str, n: &str, seps: &[&str]) -> bool {
    t.strip_prefix(n)
        .is_some_and(|r| r.is_empty() || seps.iter().any(|s| r.starts_with(s)))
}

/// `t` without a dated entry's `2026-10-12c, ` prefix.
fn strip_date(t: &str) -> &str {
    let b = t.as_bytes();
    let digits = |r: std::ops::Range<usize>| {
        r.into_iter()
            .all(|i| b.get(i).is_some_and(u8::is_ascii_digit))
    };
    if digits(0..4)
        && b.get(4) == Some(&b'-')
        && digits(5..7)
        && b.get(7) == Some(&b'-')
        && digits(8..10)
    {
        let i = if b.get(10).is_some_and(u8::is_ascii_lowercase) {
            11
        } else {
            10
        };
        if let Some(r) = t[i..].strip_prefix(", ") {
            return r;
        }
    }
    t
}

/// Whether `name` labels something in lines `lo..hi` of `doc`: a heading
/// whose title opens with it, a paragraph or list item whose lead (up to
/// its first `. ` or `: `) is it or lists it, a sentence that opens
/// `name:`, or a bold span that opens with it.
fn names(doc: &Doc, name: &str, (lo, hi): (usize, usize)) -> bool {
    let n = norm(name);
    for (i, title) in &doc.headings {
        if !(lo..hi).contains(i) {
            continue;
        }
        let first = title.split_whitespace().next().unwrap_or("");
        let id = first.strip_suffix('.').unwrap_or(first);
        let numbered = section_len(id.as_bytes()) == Some(id.len());
        let t = norm(if numbered {
            &title[first.len()..]
        } else {
            title
        });
        if opens_with(&t, &n, &[" ", ",", ":", ".", ";"]) {
            return true;
        }
    }
    let lines = &doc.lines[lo..hi];
    let label = format!("{n}:");
    for b in blocks(lines) {
        let t = strip_marker(&b).unwrap_or(&b);
        let t = norm(&t.trim_start_matches('#').replace('*', ""));
        let cut = [". ", ": "]
            .iter()
            .filter_map(|s| t.find(s))
            .min()
            .unwrap_or(t.len());
        let lead = strip_date(t[..cut].trim_end_matches(['.', ':']));
        let mut parts = lead.split(", ").flat_map(|p| p.split(" and "));
        if opens_with(lead, &n, &[",", " ("]) || parts.any(|p| p == n) {
            return true;
        }
        if t.match_indices(&label)
            .any(|(k, _)| k == 0 || t[..k].ends_with(". "))
        {
            return true;
        }
    }
    lines.join(" ").split("**").skip(1).step_by(2).any(|bold| {
        let b = norm(bold);
        opens_with(b.trim_end_matches(['.', ':']), &n, &[",", " (", ":", "."])
    })
}

/// Why `c` does not resolve in any of `docs`, or `None` when it does.
fn unresolved(c: &Citation, docs: &[&Doc]) -> Option<String> {
    let mut why = String::new();
    for doc in docs {
        if let Some(missing) = c.sections.iter().find(|s| !doc.sections.contains_key(*s)) {
            why = format!("no section {missing}");
            continue;
        }
        let ranges: Vec<(usize, usize)> = if c.sections.is_empty() {
            vec![(0, doc.lines.len())]
        } else {
            c.sections.iter().map(|s| doc.sections[s]).collect()
        };
        match &c.name {
            Some(n) if !ranges.iter().any(|&r| names(doc, n, r)) => {
                why = format!("nothing named {n:?} there");
            }
            _ => return None,
        }
    }
    Some(why)
}

/// Comment markers a code line opens with.
fn strip_comment(l: &str) -> &str {
    let t = l.trim_start();
    let t = ["//!", "///", "//", "#", "*", ">"]
        .iter()
        .find_map(|m| {
            t.strip_prefix(m)
                .filter(|r| !(*m == "#" && r.starts_with('#')))
        })
        .unwrap_or(t);
    t.strip_prefix(' ').unwrap_or(t)
}

#[test]
fn every_doc_citation_resolves() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut doc_paths = docs(&root);
    doc_paths.push(root.join("AGENTS.md"));
    let mut by_name: HashMap<String, Vec<Doc>> = HashMap::new();
    for p in &doc_paths {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(p).unwrap();
        by_name.entry(name).or_default().push(index(&text));
    }

    let mut files = doc_paths.clone();
    for dir in ["crates", "tools"] {
        walk(&root.join(dir), &mut files);
    }
    files.push(root.join("README.md"));
    files.sort();

    let mut seen = 0;
    let mut bad = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let is_md = path.extension().is_some_and(|e| e == "md");
        let mut starts = Vec::new();
        let mut joined = String::new();
        for l in text.lines() {
            starts.push(joined.len());
            joined.push_str(if is_md { l.trim() } else { strip_comment(l) });
            joined.push(' ');
        }
        for c in citations(&joined) {
            seen += 1;
            let docs: Vec<&Doc> = by_name
                .get(&c.doc)
                .map_or(Vec::new(), |d| d.iter().collect());
            let why = if docs.is_empty() {
                Some("no such doc".to_string())
            } else {
                unresolved(&c, &docs)
            };
            if let Some(why) = why {
                let line = starts.partition_point(|&s| s <= c.at);
                let rel = path.strip_prefix(&root).unwrap_or(path).display();
                let name = c.name.as_ref().map(|n| format!(" {n:?}"));
                let cited = format!(
                    "{} {}{}",
                    c.doc,
                    c.sections.join(", "),
                    name.unwrap_or_default()
                );
                bad.push(format!("{rel}:{line}: {cited}: {why}"));
            }
        }
    }
    assert!(seen > 3000, "the scan found only {seen} citations");
    assert!(
        bad.is_empty(),
        "{} of {seen} doc citations do not resolve:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn no_doc_numbers_two_sections_alike() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut bad = Vec::new();
    for p in docs(&root) {
        let doc = index(&std::fs::read_to_string(&p).unwrap());
        for id in &doc.repeated {
            let rel = p.strip_prefix(&root).unwrap_or(&p).display();
            bad.push(format!("{rel}: section {id} twice"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
