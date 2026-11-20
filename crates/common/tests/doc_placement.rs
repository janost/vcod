//! Doc comments that sit on the wrong item, in every `.rs` file under
//! `crates/`.
//!
//! The shape: an item moves or something is inserted under its doc, and its
//! `///` block ends up glued onto the next item's with no blank `///` line
//! between. Rustdoc and the editor show both on the second item and the first
//! one goes bare. A block is split wherever a line ends in a full stop and the
//! next opens with a capital or a backtick, and a split is a hit when
//!
//! - glued: its line is under 62 characters and one of the next three items
//!   at the same indentation has no doc, the owner the detached part left;
//! - summary: it closes the block's first line, the block sits on a bare kind
//!   of item (fn, struct, enum, const, static, type) and the next item is an
//!   undocumented fn.
//!
//! A split before a closing note (`Vacuous ...`, `Needs ...`, an env var in
//! backticks), before a continuation (`Or`, `Otherwise`, ...) or after
//! `e.g.` is no hit. A detached comment whose owner is far away or in
//! another file goes unseen; so does one whose owner still has a doc.
//! [`ALLOWED`] names the hits that are a paragraph of the item's own doc.

use std::path::{Path, PathBuf};

/// Hits that are the item's own paragraphs, by file and the line ending the
/// first part. An entry that no longer hits fails the test.
const ALLOWED: &[(&str, &str)] = &[
    (
        "crates/client/src/cod4_sunshadow.rs",
        "uniform the tail of binding 19's.",
    ),
    (
        "crates/client/src/frame_prof.rs",
        "The GPU pass groups the renderer stamps, in file order.",
    ),
    (
        "crates/client/src/renderer/shadow_encode.rs",
        "left (`cod2-client-perf.md` 11).",
    ),
    (
        "crates/client/src/renderer/dynamic.rs",
        "(a split, the dlights), which a run never crosses.",
    ),
    (
        "crates/client/src/play/cod4_sweep.rs",
        "`soak.md` under `VCOD_SWEEP_OUT` (default `tmp/cod4-soak`).",
    ),
    (
        "crates/client/src/probe/observe.rs",
        "what the cmds run there left him.",
    ),
    (
        "crates/cod2/src/net/flags.rs",
        "`eFlags` (entity and playerstate, 24 bits).",
    ),
    (
        "crates/cod4/src/ragdoll.rs",
        "its share of `ragdoll_explode_force` as a change of momentum.",
    ),
    (
        "crates/cod4/src/collision/mod.rs",
        "A hash of every array of `c` the clip reads, each led by its length.",
    ),
    (
        "crates/common/src/animscript.rs",
        "`legsAnim` in both retail deaths, cod11-combat.md section 8).",
    ),
    (
        "crates/common/src/menu_precomp.rs",
        "`text` with its directives carried out and every macro expanded.",
    ),
    (
        "crates/common/src/patch.rs",
        "`ChopWindingInPlace` (0x805046c): `None` when nothing is left in front.",
    ),
    (
        "crates/common/src/collision/masks.rs",
        "what stops a bullet, a blast probe and a falling item.",
    ),
    (
        "crates/common/src/iwi/wavelet.rs",
        "Every 2x2 block of a level, row by row, from one base pixel each.",
    ),
    ("crates/common/src/net/mod.rs", "its helper 0x417b10)."),
    (
        "crates/common/src/net/cod1/flags.rs",
        "from a run whose spawns happened to pair off.",
    ),
    (
        "crates/gsc/src/atom.rs",
        "Looks up any spelling of a folded key without interning it.",
    ),
    (
        "crates/gsc/src/vm/mod.rs",
        "formatting a number for a configstring cannot drift from it.",
    ),
    (
        "crates/gsc/src/vm/sched.rs",
        "reaches its own suspend point to write anything back).",
    ),
    (
        "crates/server/src/nav/build.rs",
        "12.14), and paced mp_decoy's pit under its leaps out (11.8).",
    ),
    (
        "crates/server/src/nav/probe.rs",
        "standing on a lip it overhangs is held by it.",
    ),
    (
        "crates/server/tests/entity_vis_ab.rs",
        "Samples vcod knowingly does not reproduce, as `(map, label, reason)`.",
    ),
    (
        "crates/server/tests/leanhit_ab.rs",
        "The 1.1d `-cmds` recipe's second run, its `ct=` printed by the probe.",
    ),
    (
        "crates/server/tests/moves_ab.rs",
        "Fields a capture's replay may miss, with how many snapshots and why.",
    ),
    (
        "crates/server/tests/playerstate_ab.rs",
        "The spawn-dependent fields, checked for shape rather than for equality.",
    ),
    (
        "crates/server/tests/turret_ab.rs",
        "(`cod11-turrets.md` 13.9).",
    ),
    (
        "crates/server/tests/vehicle_armor_ab.rs",
        "tap, each seat for 12 s, out.",
    ),
];

/// Item kinds whose doc a detached comment leaves missing.
const BARE_KINDS: [&str; 6] = ["fn", "struct", "enum", "const", "static", "type"];

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

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

fn is_doc(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("///") && !t.starts_with("////")
}

fn is_comment(l: &str) -> bool {
    l.trim_start().starts_with("//")
}

fn is_attr(l: &str) -> bool {
    l.trim_start().starts_with("#[")
}

/// A `///` line's text past the marker and one space.
fn doc_text(l: &str) -> &str {
    let t = &l.trim_start()[3..];
    t.strip_prefix(' ').unwrap_or(t)
}

/// The leading identifier of `s`.
fn word(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

/// The keyword of the item a line opens (`fn`, `const`, `impl`, ...), past
/// its visibility and qualifiers.
fn kind(line: &str) -> Option<&str> {
    let mut s = line.trim_start();
    if let Some(rest) = s.strip_prefix("pub") {
        s = match rest.strip_prefix('(') {
            Some(r) => &r[r.find(')')? + 1..],
            None => rest,
        };
        s = s.strip_prefix(' ')?;
    }
    loop {
        let w = word(s);
        let rest = s[w.len()..].trim_start();
        match w {
            "fn" | "struct" | "enum" | "static" | "type" | "trait" | "union" | "mod" | "impl" => {
                return Some(w);
            }
            "const" if matches!(word(rest), "fn" | "async" | "unsafe" | "extern") => s = rest,
            "const" => return Some(w),
            "async" | "unsafe" => s = rest,
            "extern" => s = rest.strip_prefix("\"C\"").unwrap_or(rest).trim_start(),
            _ => return None,
        }
    }
}

/// The items after the one at `k` at its indentation, up to the end of its
/// scope: (item line, whether a comment or doc sits on it).
fn siblings(lines: &[&str], k: usize) -> Vec<(usize, bool)> {
    let depth = indent(lines[k]);
    let mut out = Vec::new();
    let mut m = k + 1;
    while m < lines.len() {
        let l = lines[m];
        let t = l.trim_start();
        if t.is_empty() {
            m += 1;
            continue;
        }
        if indent(l) < depth {
            break;
        }
        if indent(l) > depth || t.starts_with(['}', ')', ']', '{']) || t.starts_with("where") {
            m += 1;
            continue;
        }
        let mut q = m;
        let mut doc = false;
        while q < lines.len() && (is_attr(lines[q]) || is_comment(lines[q])) {
            doc |= is_comment(lines[q]);
            q += 1;
        }
        if q == lines.len() {
            break;
        }
        out.push((q, doc));
        m = q + 1;
    }
    out
}

/// A split no detached comment makes: a closing note, a continuation, or a
/// full stop that ends an abbreviation.
fn benign(a: &str, b: &str) -> bool {
    let abbrev = ["e.g.", "i.e.", "etc."].iter().any(|x| {
        a.strip_suffix(x)
            .is_some_and(|head| !head.ends_with(|c: char| c.is_alphanumeric() || c == '_'))
    });
    let note = ["Vacuous", "Needs ", "Run in release", "Writes no fixture"]
        .iter()
        .any(|x| b.starts_with(x))
        || b.strip_prefix('`').is_some_and(|r| {
            r.starts_with(|c: char| c.is_ascii_uppercase()) && {
                let name = r
                    .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                    .map(|n| &r[n..]);
                name.is_some_and(|tail| tail.starts_with(['=', '`']))
            }
        });
    let cont = ["Or", "Otherwise", "But", "And", "So", "Then"].contains(&word(b));
    abbrev || note || cont
}

/// One hit: the line ending the first part (one-based), the rule, its text.
#[derive(Debug, PartialEq)]
struct Hit {
    line: usize,
    rule: &'static str,
    text: String,
}

fn hits(src: &str) -> Vec<Hit> {
    let lines: Vec<&str> = src.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_doc(lines[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && is_doc(lines[i]) {
            i += 1;
        }
        let block: Vec<&str> = lines[start..i].iter().map(|l| doc_text(l)).collect();
        let mut k = i;
        while k < lines.len() && (lines[k].trim().is_empty() || is_attr(lines[k])) {
            k += 1;
        }
        if k == lines.len() {
            break;
        }
        let near: Vec<(usize, bool)> = siblings(&lines, k).into_iter().take(3).collect();
        let mut bare_near = false;
        for &(q, doc) in &near {
            let Some(kd) = kind(lines[q]) else { break };
            if !doc && BARE_KINDS.contains(&kd) {
                bare_near = true;
                break;
            }
        }
        let next_bare_fn = near
            .first()
            .is_some_and(|&(q, doc)| !doc && kind(lines[q]) == Some("fn"));
        let on_bare = kind(lines[k]).is_some_and(|kd| BARE_KINDS.contains(&kd));
        for (n, pair) in block.windows(2).enumerate() {
            let (a, b) = (pair[0], pair[1]);
            let opens = b.starts_with(|c: char| c.is_uppercase() || c == '`');
            if !a.ends_with('.') || !opens || benign(a, b) {
                continue;
            }
            let rule = if a.chars().count() < 62 && bare_near {
                "glued"
            } else if n == 0 && on_bare && next_bare_fn {
                "summary"
            } else {
                continue;
            };
            out.push(Hit {
                line: start + n + 1,
                rule,
                text: a.to_string(),
            });
        }
    }
    out
}

#[test]
fn no_doc_comment_sits_on_the_wrong_item() {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut seen = vec![false; ALLOWED.len()];
    let mut bad = Vec::new();
    for path in rust_files() {
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let rel = format!(
            "crates/{}",
            path.strip_prefix(&crates)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        );
        for h in hits(&src) {
            match ALLOWED.iter().position(|&(f, t)| f == rel && t == h.text) {
                Some(n) => seen[n] = true,
                None => bad.push(format!("{rel}:{} ({}): {}", h.line, h.rule, h.text)),
            }
        }
    }
    let stale: Vec<String> = ALLOWED
        .iter()
        .zip(&seen)
        .filter(|(_, s)| !**s)
        .map(|((f, t), _)| format!("{f}: {t}"))
        .collect();
    assert!(
        bad.is_empty(),
        "a doc comment runs into the next item's: move the first part onto the \
         item it describes, or add the line to ALLOWED if both parts are this \
         item's (the line ending the first part is shown):\n{}",
        bad.join("\n")
    );
    assert!(
        stale.is_empty(),
        "ALLOWED entries that no longer hit:\n{}",
        stale.join("\n")
    );
}

#[test]
fn the_check_fires_on_the_shapes_it_claims_to_catch() {
    // The first item's doc glued onto the second's, the first left bare.
    let glued = "/// The first one.\n/// The second one.\nfn second() {}\n\nfn first() {}\n";
    assert_eq!(
        hits(glued),
        [Hit {
            line: 1,
            rule: "glued",
            text: "The first one.".into()
        }]
    );

    // The same with a long first line, which only the summary rule sees.
    let long = format!(
        "/// {}.\n/// The second one.\nfn second() {{}}\nfn first() {{}}\n",
        "x".repeat(70)
    );
    assert_eq!(hits(&long).len(), 1);

    // A second paragraph is no hit while every item around has its doc.
    let own = "/// What it does.\n/// The caller locks.\nfn one() {}\n/// Two.\nfn two() {}\n";
    assert!(hits(own).is_empty());

    // Nor is a closing note, even beside a bare item.
    let note = "/// What it does.\n/// Vacuous without `COD_DIR`.\nfn one() {}\nfn two() {}\n";
    assert!(hits(note).is_empty());
}
