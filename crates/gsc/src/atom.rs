//! Interned script strings. The engine interns every script string
//! (`GScr_AllocString`), which is why `Atom` equality is a `u32` compare.
//!
//! Two separate tables, not one: the engine matches identifiers, field
//! names and file paths case-insensitively, so those fold. String
//! *content* (`Value::String`/`Localized`/`Anim`) does not fold — folding
//! it would lowercase every script-built message before it reaches a
//! player. A single dedup-on-folded-form table can't serve both: a host
//! resolving a builtin name needs the folded spelling to match its own
//! lowercase literals, while a host reading a string value needs the
//! original spelling back. `Atom` and `StrAtom` are distinct types so a
//! folded identifier atom and a verbatim content atom can't be swapped by
//! mistake and silently resolved against the wrong table.
//!
//! Content is interned permanently, with no reclamation: a script that
//! loops `s = s + "x"` grows the content table without bound for the life
//! of the process. Not fixed here — the heap this feeds is dropped whole on
//! map change, and G1 has no GC.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Atom(pub u32);

/// An interned string-content atom: `Value::String`, `Value::Localized` or
/// `Value::Anim`. Distinct from `Atom` so it never resolves against the
/// folding table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StrAtom(pub u32);

#[derive(Default)]
pub struct Interner {
    by_text: HashMap<String, Atom>,
    text: Vec<String>,
    by_str: HashMap<String, StrAtom>,
    str_text: Vec<String>,
}

impl Interner {
    /// Interns an identifier, field name, function name or file path,
    /// case-folded to match the engine's case-insensitive lookup.
    pub fn intern(&mut self, s: &str) -> Atom {
        let folded = s.to_ascii_lowercase();
        if let Some(&a) = self.by_text.get(&folded) {
            return a;
        }
        let a = Atom(self.text.len() as u32);
        self.text.push(folded.clone());
        self.by_text.insert(folded, a);
        a
    }

    pub fn get(&self, s: &str) -> Option<Atom> {
        self.by_text.get(&s.to_ascii_lowercase()).copied()
    }

    pub fn resolve(&self, a: Atom) -> &str {
        &self.text[a.0 as usize]
    }

    /// Interns string content verbatim: no case folding, no lookup by
    /// folded form.
    pub fn intern_str(&mut self, s: &str) -> StrAtom {
        if let Some(&a) = self.by_str.get(s) {
            return a;
        }
        let a = StrAtom(self.str_text.len() as u32);
        self.str_text.push(s.to_string());
        self.by_str.insert(s.to_string(), a);
        a
    }

    pub fn resolve_str(&self, a: StrAtom) -> &str {
        &self.str_text[a.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_the_same_text_twice_yields_the_same_atom() {
        let mut i = Interner::default();
        let a = i.intern("allies");
        let b = i.intern("allies");
        let c = i.intern("axis");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "allies");
    }

    /// gsc identifiers and script paths are matched case-insensitively by
    /// the engine, so the interner folds case and `resolve` returns the
    /// folded form.
    #[test]
    fn interning_folds_case() {
        let mut i = Interner::default();
        assert_eq!(i.intern("PlayerConnect"), i.intern("playerconnect"));
        let main = i.intern("MaIn");
        assert_eq!(i.resolve(main), "main");
    }

    #[test]
    fn get_does_not_intern() {
        let mut i = Interner::default();
        assert!(i.get("nope").is_none());
        let a = i.intern("yes");
        assert_eq!(i.get("YES"), Some(a));
    }

    /// String content is not case-folded: two spellings differing only in
    /// case are two distinct atoms, and each resolves back to what was
    /// interned, not to a lowercased form.
    #[test]
    fn interning_str_does_not_fold_case() {
        let mut i = Interner::default();
        let a = i.intern_str("Objective Complete");
        let b = i.intern_str("objective complete");
        assert_ne!(a, b);
        assert_eq!(i.resolve_str(a), "Objective Complete");
        assert_eq!(i.resolve_str(b), "objective complete");
    }

    #[test]
    fn interning_the_same_str_content_twice_yields_the_same_atom() {
        let mut i = Interner::default();
        let a = i.intern_str("Round won");
        let b = i.intern_str("Round won");
        assert_eq!(a, b);
    }
}
