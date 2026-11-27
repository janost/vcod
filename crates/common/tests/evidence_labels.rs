//! The evidence labels in `docs/research/*.md` and `docs/protocol-1.1.md`,
//! checked for the two shapes a grep can see.
//!
//! `docs/research/README.md` asks for one label per claim: VERIFIED for what was read out
//! of a binary, an asset or a capture, a branch condition or instruction order
//! read at a named address range included; INFERRED for a function's identity,
//! intent, flow read without addresses, and another game's or engine's source
//! shape. Five review rounds across four implementers went on enforcing labels
//! by reading, so the two blunt shapes are enforced here instead:
//!
//! - a per-section blanket past the document's header, in the spellings
//!   `BLANKETS` lists (`VERIFIED throughout`, "every number below is
//!   VERIFIED") and the escape-clause forms (`VERIFIED unless marked`,
//!   `all VERIFIED`, "every result ... is VERIFIED"), which the rule forbids
//!   outright: a section is a mix of
//!   claims and a blanket covers the ones it should not;
//! - one clause carrying both labels, which is how a single claim ends up
//!   opening VERIFIED and closing INFERRED and is really neither. `UNVERIFIED`
//!   is a third label the docs use and does not count as VERIFIED here: it
//!   pairs with INFERRED without contradiction, so a clause carrying those two
//!   is legitimate and is left alone.
//!
//! - a label in a section heading (`## 16. ..., VERIFIED`), which covers the
//!   whole section the same as a blanket;
//! - a section of 400 bytes or more whose body opens with a bare `VERIFIED`
//!   and holds no other label: one word over every claim that follows.
//!
//! **What it cannot catch**, and a reader still has to:
//!
//! - a VERIFIED claim that narrates sequencing ("followed by", "then") or a
//!   branch condition ("gated on that field being non-null") with no address
//!   range behind it, or that rests on another game's source shape. A grep
//!   cannot tell it from the same sentence read at a cited range;
//! - a claim carrying no label at all;
//! - a document-level default ("everything here is VERIFIED unless labelled
//!   otherwise"), the most common blanket left in the corpus. The rule
//!   sanctions it where the document's provenance is uniform and every
//!   exception is labelled, which is a judgement no grep makes, so this
//!   check neither flags it nor vouches for it;
//! - whether a VERIFIED claim is actually true.
//!
//! Passing this proves the docs are free of two mistakes, not that their
//! labels are right.
//!
//! It lives here because the research docs belong to the workspace rather
//! than to any one crate, and this crate's tests need no game data, so it
//! runs in CI.

use std::path::PathBuf;

/// Every `docs/research/*.md`, sorted, then `docs/protocol-1.1.md`, read from
/// the repo the crate sits in.
fn research_docs() -> Vec<(String, String)> {
    let dir = PathBuf::from(vcod_common::manifest_dir!()).join("../../docs/research");
    let mut docs: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"));
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, text)
        })
        .collect();
    docs.sort();
    assert!(!docs.is_empty(), "no research docs under {}", dir.display());
    let wire = dir.join("../protocol-1.1.md");
    let text = std::fs::read_to_string(&wire).unwrap_or_else(|e| panic!("read {wire:?}: {e}"));
    docs.push(("protocol-1.1.md".into(), text));
    docs
}

/// One-based line of a byte offset.
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// `text` cut into clauses, as (byte offset, clause). A clause ends at `.`,
/// `;`, `!` or `?` followed by whitespace or end of input, at a table cell
/// wall, or at a blank line. Sentence-ending punctuation is not enough on its
/// own: `spawn.rs` and `0.5` would each split mid-sentence and hide a clause
/// that carries both labels.
fn clauses(text: &str) -> Vec<(usize, &str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..bytes.len() {
        let ends_here = match bytes[i] {
            b'.' | b';' | b'!' | b'?' => bytes.get(i + 1).is_none_or(u8::is_ascii_whitespace),
            b'|' => true,
            b'\n' => bytes.get(i + 1) == Some(&b'\n'),
            _ => false,
        };
        if ends_here {
            out.push((start, &text[start..=i]));
            start = i + 1;
        }
    }
    if start < bytes.len() {
        out.push((start, &text[start..]));
    }
    out
}

/// The first `n` characters of `s` on one line, for a readable failure.
fn excerpt(s: &str, n: usize) -> String {
    let flat: Vec<&str> = s.split_whitespace().collect();
    let joined = flat.join(" ");
    match joined.char_indices().nth(n) {
        Some((cut, _)) => format!("{}...", &joined[..cut]),
        None => joined,
    }
}

/// True when `s` carries the VERIFIED label. `UNVERIFIED` is a third label
/// this repo uses, in five research docs, and it is not this one: an
/// occurrence preceded by `UN` does not count. "Not established" and "read
/// off control flow" are compatible readings, so a clause carrying UNVERIFIED
/// and INFERRED is legitimate prose and must not be reported.
fn has_verified(s: &str) -> bool {
    s.match_indices("VERIFIED")
        .any(|(i, _)| !s[..i].ends_with("UN"))
}

/// The spellings of a label that covers more than the one table or list it
/// leads: `VERIFIED throughout` (so `UNVERIFIED throughout` too), "every
/// number below is VERIFIED", "everything here". A lead-in that labels the
/// table or list right under it is allowed (`docs/research/README.md`).
const BLANKETS: [&str; 6] = [
    "VERIFIED throughout",
    "INFERRED throughout",
    " below is VERIFIED",
    " below is INFERRED",
    "verything here is VERIFIED",
    "verything here is INFERRED",
];

/// Phrases that turn a label into a default with exceptions: a clause carrying
/// a label and one of these covers every claim around it, which is the
/// blanket in other words (`VERIFIED unless marked:`, `all VERIFIED:`).
const ESCAPES: [&str; 5] = [
    "unless marked",
    "unless labelled",
    "unless noted",
    "unless a step says",
    "except where",
];

/// True when `clause` is a label with an escape clause, `all VERIFIED`, or
/// "every result/claim ... VERIFIED". The lead-in for one list reads "each
/// VERIFIED" instead, which stays allowed.
fn is_escape_blanket(clause: &str) -> bool {
    let labelled = has_verified(clause) || clause.contains("INFERRED");
    let every = ["Every result", "Every claim", "Each bullet is"]
        .iter()
        .any(|p| clause.contains(p));
    labelled
        && (ESCAPES.iter().any(|e| clause.contains(e))
            || clause.contains("all VERIFIED")
            || clause.contains("all INFERRED")
            || every)
}

/// Byte offset of the first `## ` heading: what sits above it is the
/// document's header, where a document-level default may stand.
fn first_h2(text: &str) -> usize {
    if text.starts_with("## ") {
        return 0;
    }
    text.find("\n## ").map_or(text.len(), |i| i + 1)
}

/// The blankets in `text` past its header, as byte offsets.
fn blankets(text: &str) -> Vec<usize> {
    let from = first_h2(text);
    let mut out: Vec<usize> = BLANKETS
        .iter()
        .flat_map(|b| text.match_indices(b).map(|(i, _)| i))
        .chain(
            clauses(text)
                .into_iter()
                .filter(|(_, c)| is_escape_blanket(c))
                .map(|(i, c)| i + c.len() - c.trim_start().len()),
        )
        .filter(|&i| i >= from)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A section is a mix of claims, so no label may cover a whole one. Every
/// claim carries its own, or the table or list it sits in carries one
/// lead-in label.
#[test]
fn no_research_doc_blankets_a_section_as_verified() {
    let mut hits = Vec::new();
    for (name, text) in research_docs() {
        for offset in blankets(&text) {
            hits.push(format!(
                "{name}:{}: {}",
                line_of(&text, offset),
                excerpt(&text[offset..], 90)
            ));
        }
    }
    assert!(
        hits.is_empty(),
        "a per-section evidence blanket covers claims it should not; \
         label each claim where it is made:\n{}",
        hits.join("\n")
    );
}

/// One claim, one label. A clause opening VERIFIED and closing INFERRED
/// leaves a reader unable to tell which half either word covers.
#[test]
fn no_claim_carries_both_evidence_labels() {
    let mut hits = Vec::new();
    for (name, text) in research_docs() {
        for (offset, clause) in clauses(&text) {
            if has_verified(clause) && clause.contains("INFERRED") {
                hits.push(format!(
                    "{name}:{}: {}",
                    line_of(&text, offset),
                    excerpt(clause, 140)
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "one clause carries both evidence labels; split it so each label \
         states what it covers, and put sequencing and branch conditions \
         under INFERRED:\n{}",
        hits.join("\n")
    );
}

/// The sections of `text` as (heading line, one-based line of the heading,
/// body), a section running to the next heading of any level.
fn sections(text: &str) -> Vec<(&str, usize, String)> {
    let mut out: Vec<(&str, usize, String)> = Vec::new();
    let mut fence = false;
    for (n, line) in text.lines().enumerate() {
        if line.starts_with("```") {
            fence = !fence;
        }
        if !fence && line.starts_with('#') && line.trim_start_matches('#').starts_with(' ') {
            out.push((line, n + 1, String::new()));
        } else if let Some(last) = out.last_mut() {
            last.2.push_str(line);
            last.2.push('\n');
        }
    }
    out
}

/// A label in a heading covers the whole section.
fn labelled_headings(text: &str) -> Vec<(usize, String)> {
    sections(text)
        .into_iter()
        .filter(|(h, ..)| has_verified(h) || h.contains("INFERRED"))
        .map(|(h, n, _)| (n, excerpt(h, 90)))
        .collect()
}

/// A body this long holds several claims; a shorter one is one claim and one
/// label.
const LONE_LABEL_MIN_BODY: usize = 400;

/// Sections that are one table or one decompiled function under a single
/// source-naming lead-in (`VERIFIED (the decompilation): ...`), where the one
/// label is the claim. (file, heading prefix)
const ONE_SOURCE_SECTIONS: [(&str, &str); 5] = [
    ("cod11-events-and-fx.md", "### 5a."),
    ("cod11-gametypes-re-bel.md", "### 7.4 "),
    ("cod2-combat.md", "### 1.5 "),
    ("coduo-movement.md", "### While deployed"),
    ("player-model-anim-system.md", "### 6.9 "),
];

/// Sections whose first body line opens with a bare `VERIFIED` and whose body
/// carries no second label, and is long enough to hold more than one claim.
fn lone_verified_openers(text: &str) -> Vec<(usize, String)> {
    sections(text)
        .into_iter()
        .filter(|(_, _, body)| {
            let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let first = first.trim_start_matches(['`', '*', ' ']);
            let labels = body.matches("VERIFIED").count() + body.matches("INFERRED").count()
                - body.matches("UNVERIFIED").count();
            first.starts_with("VERIFIED") && labels <= 1 && body.len() >= LONE_LABEL_MIN_BODY
        })
        .map(|(h, n, _)| (n, excerpt(h, 90)))
        .collect()
}

/// A section carries its labels in its body, one per claim.
#[test]
fn no_heading_carries_a_label() {
    let mut hits = Vec::new();
    for (name, text) in research_docs() {
        for (line, h) in labelled_headings(&text) {
            hits.push(format!("{name}:{line}: {h}"));
        }
    }
    assert!(
        hits.is_empty(),
        "a heading label covers the whole section; label each claim in the body:\n{}",
        hits.join("\n")
    );
}

#[test]
fn no_section_opens_with_a_lone_verified() {
    let mut hits = Vec::new();
    for (name, text) in research_docs() {
        for (line, h) in lone_verified_openers(&text) {
            if ONE_SOURCE_SECTIONS
                .iter()
                .any(|(f, p)| *f == name && h.starts_with(p))
            {
                continue;
            }
            hits.push(format!("{name}:{line}: {h}"));
        }
    }
    assert!(
        hits.is_empty(),
        "a section opens with VERIFIED and holds no other label, so one word \
         covers every claim; label each claim:\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_check_fires_on_the_shapes_it_claims_to_catch() {
    // The section 19 sentence that cost this repo a review round: opens
    // VERIFIED, closes INFERRED, one claim.
    let both = "VERIFIED, read straight from the binary: right after \
                `RegisterItem(weaponinfo)` (`0x52d74`), it reads two fields off the \
                pointer `BG_GetInfoForWeapon` returns and, for each one that is a \
                non-null, non-empty string, calls `G_SoundAliasIndex` on it — reading \
                control flow, so INFERRED FROM DECOMPILATION for the mechanism.";
    assert_eq!(
        clauses(both)
            .iter()
            .filter(|(_, c)| has_verified(c) && c.contains("INFERRED"))
            .count(),
        1
    );

    // A label per clause, correctly split, is what the docs should read like.
    let split = "INFERRED FROM DECOMPILATION that offset 0x2fc carries the alt-fire \
                 link (not single-stepped); VERIFIED that every `*_semi_mp` bit in \
                 both captures sits beside its base weapon's.";
    assert!(
        clauses(split)
            .iter()
            .all(|(_, c)| !(has_verified(c) && c.contains("INFERRED")))
    );

    // A file name's dot is not a clause boundary, or the split above would
    // hide a real double label rather than pass it through.
    assert_eq!(clauses("a `spawn.rs` b. c").len(), 2);

    // UNVERIFIED is a different label, and pairing it with INFERRED says two
    // compatible things rather than one claim labelled twice.
    assert!(!has_verified(
        "UNVERIFIED, and INFERRED FROM DECOMPILATION."
    ));
    assert!(has_verified("UNVERIFIED there, VERIFIED here."));

    // A blanket past the header is caught; a document-level default above
    // the first section and a lead-in for one list are not.
    let doc = "# T\n\nEverything here is VERIFIED unless marked.\n\n## 1. A\n\n\
               Every number below is VERIFIED (the logs).\n\nThe table, VERIFIED:\n";
    assert_eq!(blankets(doc).len(), 1);
    assert_eq!(
        blankets("## 1. A\n\nOne list, VERIFIED throughout.\n").len(),
        1
    );

    // An escape clause or "all" makes a lead-in a default; "each" does not.
    let lead = "## 1. A\n\nThe stores (VERIFIED unless marked):\n\n- a\n\n\
                They show, all VERIFIED:\n\n- b\n\n\
                Every result in this section is VERIFIED.\n\n\
                Each bullet is VERIFIED unless labelled otherwise.\n\n\
                The stores, each VERIFIED:\n\n- c\n";
    assert_eq!(blankets(lead).len(), 4);

    // A label in a heading, and a lone VERIFIED opener, are caught; a section
    // with a second label is not.
    assert_eq!(labelled_headings("## 3. Name, VERIFIED\n\ntext\n").len(), 1);
    assert_eq!(labelled_headings("## 3. Name\n\nVERIFIED x.\n").len(), 0);
    assert_eq!(
        lone_verified_openers(&format!(
            "## 1. A\n\n`VERIFIED`. Per-row defaults. {}\n",
            "x".repeat(400)
        ))
        .len(),
        1
    );
    assert_eq!(
        lone_verified_openers("## 1. A\n\nVERIFIED x. INFERRED y.\n").len(),
        0
    );
}
