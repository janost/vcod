//! Interned script strings. The engine interns every script string
//! (`GScr_AllocString`), which is why `Atom` equality is a `u32` compare.
//!
//! Interning has two roles, because retail folds case for one and not the
//! other (`tests/fixtures/semantics/retail-captures.txt`):
//!
//! - `intern_folded` dedups on the case-folded text. Identifiers, field
//!   names, file paths, function names and event names (`notify`/
//!   `waittill`/`endon`) are matched case-insensitively, so two spellings
//!   of one key must share an atom: `# probe_field_case` reads
//!   `level.myField` back through `level.myfield`, and a `waittill` waiting
//!   on one spelling has to see a `notify` fired with the other.
//! - `intern` dedups on the exact text. String values and array keys do not
//!   fold: `# probe_cmp` has `"ABC" == "abc"` false, and
//!   `# probe_arraykey_case` has `a["medFire"]` and `a["medfire"]` as two
//!   entries with a `.size` of 2.
//!
//! Both store the text verbatim, and `resolve` returns that stored spelling
//! rather than a folded one: `Value::String`/`Localized`/`Anim` atoms come
//! from this same table, so folding on storage would lowercase every
//! script-built display string (`"Round " + "won"` -> `"round won"`) and
//! every literal containing a capital, of which the shipped corpus has 2553.
//!
//! `resolve_folded` returns the lowercased form on demand, for matching a
//! resolved name against a fixed lowercase literal, e.g. builtin dispatch
//! (`IPrintLn(...)` must still reach a host matching on `"iprintln"`).
//! `fold_atom` is the same idea one level up: it maps any atom to the atom
//! `intern_folded` would have returned for its text, which is how an event
//! name that arrived as a case-preserving string value (a literal, a
//! concatenation, a host-supplied atom) still matches the other spellings
//! of the same event.
//!
//! Whichever spelling of a folded key is interned first wins for display:
//! `Panzerfaust` and `panzerfaust` share one atom in identifier roles and
//! `resolve` returns whichever the compiler saw first. The shipped corpus
//! has 86 such multi-spelling keys, all weapon or tag names, never display
//! text; the ones used as array keys or string values no longer collapse at
//! all.
//!
//! Runtime-built strings (e.g. `s = s + "x"` in a loop) intern permanently,
//! with no reclamation. Not fixed here: the heap this feeds is dropped
//! whole on map change, and G1 has no GC.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Atom(pub u32);

#[derive(Default)]
pub struct Interner {
    /// Exact-text lookup, for string values and array keys.
    by_exact: HashMap<String, Atom>,
    /// Case-folded lookup, for identifiers, field names, file paths,
    /// function names and event names.
    by_folded: HashMap<String, Atom>,
    /// The spelling first interned for each atom; what `resolve` returns.
    text: Vec<String>,
    /// The case-folded form of the same atom; what `resolve_folded` returns.
    folded: Vec<String>,
    /// The atom that owns this atom's folded key; what `fold_atom` returns.
    canon: Vec<Atom>,
}

impl Interner {
    /// Interns by exact text: two spellings of one word get two atoms.
    pub fn intern(&mut self, s: &str) -> Atom {
        if let Some(&a) = self.by_exact.get(s) {
            return a;
        }
        let folded = s.to_ascii_lowercase();
        let a = Atom(self.text.len() as u32);
        // Claims the folded key too if nothing holds it yet, so `intern` and
        // `intern_folded` can never disagree about which atom a folded key
        // resolves to, whichever role saw the text first.
        let canon = *self.by_folded.entry(folded.clone()).or_insert(a);
        self.text.push(s.to_string());
        self.folded.push(folded);
        self.canon.push(canon);
        self.by_exact.insert(s.to_string(), a);
        a
    }

    /// Interns by case-folded text: every spelling of one word gets the
    /// atom the first spelling seen was given.
    pub fn intern_folded(&mut self, s: &str) -> Atom {
        let folded = s.to_ascii_lowercase();
        if let Some(&a) = self.by_folded.get(&folded) {
            return a;
        }
        let a = Atom(self.text.len() as u32);
        self.text.push(s.to_string());
        self.folded.push(folded.clone());
        self.canon.push(a);
        self.by_folded.insert(folded, a);
        // Two atoms with identical text would make `"x" == "x"` false, so
        // this spelling claims the exact key as well.
        self.by_exact.entry(s.to_string()).or_insert(a);
        a
    }

    /// Looks up an exact spelling without interning it.
    pub fn get(&self, s: &str) -> Option<Atom> {
        self.by_exact.get(s).copied()
    }

    /// Looks up any spelling of a folded key without interning it.
    pub fn get_folded(&self, s: &str) -> Option<Atom> {
        self.by_folded.get(&s.to_ascii_lowercase()).copied()
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

    /// The atom `intern_folded` would return for this atom's text. Every
    /// spelling of one event name folds to the same atom through this, which
    /// is what lets a `notify` reach a `waittill` that spelled the event
    /// differently.
    pub fn fold_atom(&self, a: Atom) -> Atom {
        self.canon[a.0 as usize]
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

    /// Retail folds identifiers and field names but not string values:
    /// `level.myField` and `level.myfield` are one field, while `"ABC"` and
    /// `"abc"` are two distinct strings
    /// (tests/fixtures/semantics/retail-captures.txt).
    #[test]
    fn folding_is_per_role_not_global() {
        let mut i = Interner::default();
        assert_eq!(i.intern_folded("myField"), i.intern_folded("myfield"));
        assert_ne!(i.intern("ABC"), i.intern("abc"));
        assert_eq!(i.intern("abc"), i.intern("abc"));
        // A case-preserving intern still returns exactly what went in.
        let a = i.intern("MedFire");
        assert_eq!(i.resolve(a), "MedFire");
    }

    /// gsc identifiers, paths and event names are matched case-insensitively
    /// by the engine, so two spellings of one key still intern to the same
    /// atom in those roles. `resolve` does not fold, though: it returns
    /// whichever spelling was interned first, verbatim.
    #[test]
    fn interning_folded_keeps_the_first_spelling() {
        let mut i = Interner::default();
        assert_eq!(
            i.intern_folded("PlayerConnect"),
            i.intern_folded("playerconnect")
        );
        let main = i.intern_folded("MaIn");
        assert_eq!(i.resolve(main), "MaIn");
    }

    #[test]
    fn resolve_folded_returns_the_lowercase_form_regardless_of_spelling() {
        let mut i = Interner::default();
        let a = i.intern("MaIn");
        assert_eq!(i.resolve_folded(a), "main");
    }

    /// The bridge the notify machinery runs on: an event name that reached
    /// the VM as a case-preserving string value still folds onto the atom
    /// every other spelling of it shares.
    #[test]
    fn fold_atom_maps_every_spelling_onto_one_atom() {
        let mut i = Interner::default();
        let explode = i.intern("Explode");
        let lower = i.intern("explode");
        assert_ne!(explode, lower);
        assert_eq!(i.fold_atom(explode), i.fold_atom(lower));
        assert_eq!(i.fold_atom(lower), i.intern_folded("EXPLODE"));
        // Folding is idempotent, so a doubly-folded atom is still the same.
        let once = i.fold_atom(explode);
        assert_eq!(i.fold_atom(once), once);
    }

    /// Whichever role interns a spelling first owns the folded key, so the
    /// two interns can never hand out different atoms for one identifier.
    #[test]
    fn an_exact_intern_claims_the_folded_key_it_is_the_first_spelling_of() {
        let mut i = Interner::default();
        let literal = i.intern("Panzerfaust");
        assert_eq!(i.intern_folded("panzerfaust"), literal);
        assert_eq!(i.resolve(literal), "Panzerfaust");
    }

    /// The mirror: a folded intern claims its own exact spelling, so a later
    /// string literal with that exact text is the same atom rather than a
    /// second atom holding identical text (which would make `==` on two
    /// equal strings false).
    #[test]
    fn a_folded_intern_claims_its_own_exact_spelling() {
        let mut i = Interner::default();
        let field = i.intern_folded("medFire");
        assert_eq!(i.intern("medFire"), field);
        assert_ne!(i.intern("medfire"), field);
    }

    #[test]
    fn get_does_not_intern() {
        let mut i = Interner::default();
        assert!(i.get("nope").is_none());
        let a = i.intern("yes");
        assert_eq!(i.get("yes"), Some(a));
        assert_eq!(i.get("YES"), None);
        assert_eq!(i.get_folded("YES"), Some(a));
    }
}
