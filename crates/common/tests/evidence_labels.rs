//! The evidence labels in `docs/research/*.md`, checked for the two shapes a
//! grep can see.
//!
//! `AGENTS.md` asks for one label per claim: VERIFIED for what was read out
//! of a binary, an asset or a capture, INFERRED for anything read off control
//! flow, which includes instruction sequencing and branch conditions. Five
//! review rounds across four implementers went on enforcing that by reading,
//! so the two blunt shapes are enforced here instead:
//!
//! - a per-section blanket, spelled `VERIFIED throughout`, which the rule
//!   forbids outright: a section is a mix of claims and a blanket covers the
//!   ones it should not;
//! - one clause carrying both labels, which is how a single claim ends up
//!   opening VERIFIED and closing INFERRED and is really neither. `UNVERIFIED`
//!   is a third label the docs use and does not count as VERIFIED here: it
//!   pairs with INFERRED without contradiction, so a clause carrying those two
//!   is legitimate and is left alone.
//!
//! **What it cannot catch**, and a reader still has to:
//!
//! - a correctly-split pair of sentences with the split in the wrong place: a
//!   VERIFIED half that still narrates sequencing ("followed by", "then") or a
//!   branch condition ("gated on that field being non-null"). That was the
//!   last occurrence of this defect in the repo and no grep finds it;
//! - a claim carrying no label at all;
//! - a blanket in a section heading (`## 16. ..., VERIFIED`), a shape several
//!   pure-data-read sections carry legitimately;
//! - whether a VERIFIED claim is actually true.
//!
//! Passing this proves the docs are free of two mistakes, not that their
//! labels are right.
//!
//! It lives here because the research docs belong to the workspace rather
//! than to any one crate, and this crate's tests need no game data, so it
//! runs in CI.

use std::path::PathBuf;

/// Every `docs/research/*.md`, sorted, read from the repo the crate sits in.
fn research_docs() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/research");
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

/// A section is a mix of claims, so no label may cover a whole one. Every
/// claim carries its own. The phrase match is deliberately left as a
/// substring, so `UNVERIFIED throughout` is caught too: it is the same
/// per-section blanket.
#[test]
fn no_research_doc_blankets_a_section_as_verified() {
    let mut hits = Vec::new();
    for (name, text) in research_docs() {
        for (offset, _) in text.match_indices("VERIFIED throughout") {
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
    assert!(clauses(split)
        .iter()
        .all(|(_, c)| !(has_verified(c) && c.contains("INFERRED"))));

    // A file name's dot is not a clause boundary, or the split above would
    // hide a real double label rather than pass it through.
    assert_eq!(clauses("a `spawn.rs` b. c").len(), 2);

    // UNVERIFIED is a different label, and pairing it with INFERRED says two
    // compatible things rather than one claim labelled twice.
    assert!(!has_verified(
        "UNVERIFIED, and INFERRED FROM DECOMPILATION."
    ));
    assert!(has_verified("UNVERIFIED there, VERIFIED here."));
}
