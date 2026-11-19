//! Research doc citations resolve. Code comments and docs cite a research
//! doc by section number (`cod4-fastfile.md 9`, `combat.md section 12.4`) or
//! by a quoted title (`bsp-ibsp59-format.md, "The leaf walk"`); a renumbered
//! or renamed section leaves every such pointer dangling, and no reader
//! notices. This checks, over `crates/`, `docs/`, `tools/` and the top-level
//! markdown:
//!
//! - a numbered citation names a section the doc has;
//! - a quoted title is in one of the doc's headings, bold leads, label
//!   lines or list items' first lines (case, backticks, a line wrap and a comment marker
//!   inside the quote aside);
//! - no doc gives two sections one number.
//!
//! A section number is a heading's leading number in its open top
//! section (`## 9.`, then `### 9a.`, `### 9.2`, `#### 9.2.1`), so a
//! heading titled by a version (`### 1.5 permanent bans`) is not one.
//! Short forms (`combat doc 12.4`) are not checked. It needs no game data,
//! so it runs in CI.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A research doc's section numbers, with the repeated ones, and the
/// lower-cased texts a quoted title may open.
#[derive(Default)]
struct Doc {
    sections: BTreeSet<String>,
    repeated: BTreeSet<String>,
    titles: Vec<String>,
}

fn fold(s: &str) -> String {
    s.replace('`', "")
        .split_whitespace()
        .filter(|w| !matches!(*w, "//" | "///" | "//!" | "#" | "*"))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A heading's leading section number (`9`, `9.2`, `12.3a`, `9a`) and the
/// title after it; an empty number for a heading without one.
fn section_number(rest: &str) -> (String, &str) {
    let mut end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    if end > 0 && rest[end..].starts_with(|c: char| c.is_ascii_lowercase()) {
        let after = rest[end + 1..].chars().next();
        if after.is_none_or(|c| c == '.' || c == ' ') {
            end += 1;
        }
    }
    let num = rest[..end].trim_end_matches('.');
    let valid = !num.is_empty()
        && num.starts_with(|c: char| c.is_ascii_digit())
        && num.split('.').all(|p| !p.is_empty());
    if !valid {
        return (String::new(), rest);
    }
    (num.to_string(), rest[end..].trim_start_matches('.').trim())
}

fn parse_doc(text: &str) -> Doc {
    let mut d = Doc::default();
    // The open top section's number.
    let mut top: Option<String> = None;
    let mut fence = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fence = !fence;
        }
        if fence {
            continue;
        }
        let depth = line.bytes().take_while(|&b| b == b'#').count();
        if depth >= 2 && line.as_bytes().get(depth) == Some(&b' ') {
            let rest = line[depth..].trim();
            let (num, title) = section_number(rest);
            let parts: Vec<&str> = num.split('.').collect();
            // `## 9.` opens a top section; `### 9.2` or `### 9.2.1` sits in it.
            let is_section = match parts.len() {
                0 => false,
                1 => rest[num.len()..].starts_with('.'),
                _ => top.as_deref() == Some(parts[0]),
            };
            if depth == 2 {
                top = None;
            }
            if is_section && !num.is_empty() {
                if depth == 2 && parts.len() == 1 {
                    top = Some(num.clone());
                }
                if !d.sections.insert(num.clone()) {
                    d.repeated.insert(num);
                }
            }
            d.titles.push(fold(rest));
            d.titles.push(fold(title));
            continue;
        }
        let item = line.trim_start();
        // A paragraph's own label line (`Per gsc call:`).
        if item.ends_with(':') && item.split_whitespace().count() <= 8 {
            d.titles.push(fold(item.trim_end_matches(':')));
        }
        if let Some(rest) = item.strip_prefix("- ").or_else(|| item.strip_prefix("* ")) {
            d.titles.push(fold(rest.trim_start_matches("**")));
        }
        let mut at = 0;
        while let Some(i) = line[at..].find("**") {
            let open_at = at + i + 2;
            let Some(j) = line[open_at..].find("**") else {
                break;
            };
            d.titles.push(fold(&line[open_at..open_at + j]));
            at = open_at + j + 2;
        }
    }
    d
}

/// Every file under `dir` with one of `exts`, symlinks and `target/` left
/// out.
fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            walk(&p, exts, out);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(p);
        }
    }
}

/// What follows a doc name: section numbers (two for a `9.9..9.13`
/// range) or a quoted title.
enum Cite {
    Sections(Vec<String>),
    Title(String),
}

fn cite_after(rest: &str) -> Option<Cite> {
    let rest = rest.strip_prefix(',').unwrap_or(rest);
    let rest = rest.strip_prefix(' ')?;
    if let Some(q) = rest.strip_prefix('"') {
        return Some(Cite::Title(q[..q.find('"')?].to_string()));
    }
    let rest = ["sections ", "section ", "§ ", "§"]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
        .unwrap_or(rest);
    if let Some((a, b)) = rest.split_once("..") {
        let (a, b) = (section_number(a).0, section_number(b).0);
        let digits = |n: &str| n.split('.').count();
        if !a.is_empty() && a.len() == rest.find("..").unwrap() && digits(&a) == digits(&b) {
            return Some(Cite::Sections(vec![a, b]));
        }
    }
    let (num, _) = section_number(rest);
    let next = rest[num.len()..].chars().next();
    if num.is_empty() || next.is_some_and(|c| c == '-' || c.is_alphanumeric()) {
        return None;
    }
    Some(Cite::Sections(vec![num]))
}

#[test]
fn every_research_doc_citation_resolves() {
    let root = repo();
    let mut docs = BTreeMap::new();
    let dir = root.join("docs/research");
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|e| e == "md") {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            docs.insert(name, parse_doc(&std::fs::read_to_string(&p).unwrap()));
        }
    }
    assert!(docs.len() > 20, "research docs under {}", dir.display());

    let mut bad = Vec::new();
    for (name, d) in &docs {
        for n in &d.repeated {
            bad.push(format!("docs/research/{name}: section {n} twice"));
        }
    }
    let mut files = Vec::new();
    for sub in ["crates", "docs", "tools"] {
        walk(
            &root.join(sub),
            &["rs", "md", "sh", "py", "txt"],
            &mut files,
        );
    }
    files.extend(["AGENTS.md", "README.md"].map(|f| root.join(f)));
    let (mut sections, mut titles) = (0, 0);
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let mut at = 0;
        while let Some(i) = text[at..].find(".md") {
            let end = at + i + 3;
            at = end;
            let start = text[..end - 3]
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'))
                .map_or(0, |s| s + 1);
            let name = &text[start..end];
            let Some(doc) = docs.get(name) else { continue };
            if text[..start].ends_with('/') && !text[..start].ends_with("research/") {
                continue;
            }
            let line = text[..end].matches('\n').count() + 1;
            let here = || format!("{}:{line}", f.strip_prefix(&root).unwrap_or(f).display());
            match cite_after(&text[end..]) {
                Some(Cite::Sections(ns)) => {
                    sections += 1;
                    for n in ns.iter().filter(|n| !doc.sections.contains(*n)) {
                        bad.push(format!("{}: {name} has no section {n}", here()));
                    }
                }
                Some(Cite::Title(t)) => {
                    titles += 1;
                    let t = fold(&t);
                    let t = t.trim_end_matches('.');
                    if !doc.titles.iter().any(|h| h.contains(t)) {
                        bad.push(format!("{}: {name} has no \"{t}\"", here()));
                    }
                }
                None => {}
            }
        }
    }
    assert!(
        sections > 1000 && titles > 100,
        "{sections} numbered, {titles} titled"
    );
    assert!(
        bad.is_empty(),
        "{} citations do not resolve:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
