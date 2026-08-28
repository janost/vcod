//! Interned script strings. The engine interns every script string
//! (`GScr_AllocString`), which is why `Atom` equality is a `u32` compare.
//!
//! `intern` keys its dedup on the case-folded text — the engine matches
//! identifiers, field names, file paths and event names (`notify`/
//! `waittill`/`endon`) case-insensitively, so two spellings of one key must
//! share an atom or a `waittill` waiting on one spelling never sees a
//! `notify` fired with the other. But it stores the *first* spelling it
//! saw, verbatim, and `resolve` returns that stored spelling rather than a
//! folded one: `Value::String`/`Localized`/`Anim` atoms come from this same
//! table, so folding on storage would lowercase every script-built display
//! string (`"Round " + "won"` -> `"round won"`) and every literal
//! containing a capital, of which the shipped corpus has 2553.
//!
//! `resolve_folded` returns the lowercased form on demand, for the one case
//! that still needs it: matching a resolved name against a fixed lowercase
//! literal, e.g. builtin dispatch (`IPrintLn(...)` must still reach a host
//! matching on `"iprintln"`).
//!
//! Because two spellings collapse onto one atom, whichever was interned
//! first wins for display purposes: `Panzerfaust` and `panzerfaust` share
//! an atom and `resolve` always returns whichever the compiler saw first.
//! The shipped corpus has 86 such pairs, all weapon or tag names, never
//! display text. Storing both spellings and comparing case-insensitively
//! instead would get display exactly right, but only by giving `Value` a
//! custom `PartialEq` that any future `==` on it would silently bypass —
//! worse than a bounded spelling collision.
//!
//! Runtime-built strings (e.g. `s = s + "x"` in a loop) intern permanently,
//! with no reclamation. Not fixed here: the heap this feeds is dropped
//! whole on map change, and G1 has no GC.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Atom(pub u32);

#[derive(Default)]
pub struct Interner {
    by_text: HashMap<String, Atom>,
    /// The spelling first interned for each atom; what `resolve` returns.
    text: Vec<String>,
    /// The case-folded form of the same atom; what `resolve_folded` returns.
    folded: Vec<String>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> Atom {
        let folded = s.to_ascii_lowercase();
        if let Some(&a) = self.by_text.get(&folded) {
            return a;
        }
        let a = Atom(self.text.len() as u32);
        self.text.push(s.to_string());
        self.folded.push(folded.clone());
        self.by_text.insert(folded, a);
        a
    }

    pub fn get(&self, s: &str) -> Option<Atom> {
        self.by_text.get(&s.to_ascii_lowercase()).copied()
    }

    /// The spelling first interned for this atom, verbatim.
    pub fn resolve(&self, a: Atom) -> &str {
        &self.text[a.0 as usize]
    }

    /// The case-folded form of this atom's text, for matching against a
    /// fixed lowercase literal.
    pub fn resolve_folded(&self, a: Atom) -> &str {
        &self.folded[a.0 as usize]
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

    /// gsc identifiers, paths and event names are matched case-insensitively
    /// by the engine, so two spellings of one key still intern to the same
    /// atom. `resolve` no longer folds, though: it returns whichever
    /// spelling was interned first, verbatim.
    #[test]
    fn interning_folds_identity_but_resolve_keeps_the_first_spelling() {
        let mut i = Interner::default();
        assert_eq!(i.intern("PlayerConnect"), i.intern("playerconnect"));
        let main = i.intern("MaIn");
        assert_eq!(i.resolve(main), "MaIn");
    }

    #[test]
    fn resolve_folded_returns_the_lowercase_form_regardless_of_spelling() {
        let mut i = Interner::default();
        let a = i.intern("MaIn");
        assert_eq!(i.resolve_folded(a), "main");
    }

    #[test]
    fn get_does_not_intern() {
        let mut i = Interner::default();
        assert!(i.get("nope").is_none());
        let a = i.intern("yes");
        assert_eq!(i.get("YES"), Some(a));
    }
}
